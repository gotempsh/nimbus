//! In-memory mock servers for Hetzner, Vultr, and OVH — enough of each REST
//! surface for `nimbus-cloud`'s adapters to run their full instance/
//! volume/network CRUD flow against, offline and for free. Not a faithful
//! emulation of either API — just the fields and status codes the nimbus
//! clients actually read.

mod digitalocean;
mod hetzner;
mod hostinger;
mod linode;
mod ovh;
mod scaleway;
mod vultr;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::Engine;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Path prefix of the mock's own inspection endpoints. Not part of any
/// provider's API, and exempt from the auth gate.
const INSPECT_PREFIX: &str = "/_mock";

/// What each provider mock received as first-boot user data, keyed by
/// (provider id, instance id). Lets a test assert that a script actually
/// reached the provider, in the wire format that provider's API expects.
#[derive(Clone, Default)]
pub(crate) struct Recorder(Arc<Mutex<HashMap<(String, String), Value>>>);

impl Recorder {
    /// `received` is the value exactly as it arrived on the wire; `script`
    /// is the text after undoing the provider's encoding.
    pub(crate) fn user_data(
        &self,
        provider: &str,
        instance_id: impl ToString,
        received: &str,
        script: &str,
        encoding: &str,
    ) {
        let instance_id = instance_id.to_string();
        let record = json!({
            "provider": provider,
            "instance_id": instance_id,
            "encoding": encoding,
            "received": received,
            "script": script,
        });
        self.0
            .lock()
            .unwrap()
            .insert((provider.to_owned(), instance_id), record);
    }

    /// For APIs that carry user data as plain text.
    pub(crate) fn plain_user_data(&self, provider: &str, instance_id: impl ToString, text: &str) {
        self.user_data(provider, instance_id, text, text, "plain");
    }
}

/// Decodes user data for the APIs that only accept it base64-encoded
/// (Vultr, Linode). `None` means the client sent something else — the real
/// APIs reject that, so the mocks do too.
pub(crate) fn decode_base64_user_data(received: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(received)
        .ok()?;
    String::from_utf8(bytes).ok()
}

/// `GET /_mock/{provider}/instances/{id}/user-data` — 404 when that instance
/// was created without user data (or doesn't exist).
async fn inspect_user_data(
    State(recorder): State<Recorder>,
    Path((provider, id)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    recorder
        .0
        .lock()
        .unwrap()
        .get(&(provider, id))
        .cloned()
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Router serving all provider mocks on distinct path prefixes so one bound
/// port covers everything: `/v1` Hetzner, `/v2` Vultr, `/1.0` OVH,
/// `/do/v2` DigitalOcean (Vultr owns the bare `/v2`), `/v4` Linode,
/// Scaleway's full `/instance/v1`···`/iam`···`/vpc` paths, and Hostinger's
/// full `/api/vps/v1`···`/api/billing/v1` paths.
///
/// Unhappy paths: every route rejects a missing credential or the sentinel
/// token `bad-token` with 401 (each provider's real auth header is checked),
/// and each create-server handler rejects unknown instance types with the
/// provider's real error status and body shape.
///
/// Inspection: `GET /_mock/{provider}/instances/{id}/user-data` returns the
/// user data that instance was created with — as received on the wire
/// (`received`) and decoded back to text (`script`) — so tests can assert a
/// first-boot script arrived. No credential needed.
pub fn router() -> Router {
    let recorder = Recorder::default();
    Router::new()
        .merge(hetzner::router(recorder.clone()))
        .merge(vultr::router(recorder.clone()))
        .merge(ovh::router(recorder.clone()))
        .merge(digitalocean::router(recorder.clone()))
        .merge(linode::router(recorder.clone()))
        .merge(scaleway::router(recorder.clone()))
        .merge(hostinger::router())
        .merge(
            Router::new()
                .route(
                    "/_mock/{provider}/instances/{id}/user-data",
                    get(inspect_user_data),
                )
                .with_state(recorder),
        )
        .layer(axum::middleware::from_fn(auth_gate))
}

fn token_ok(token: Option<&str>) -> bool {
    matches!(token, Some(t) if !t.is_empty() && t != "bad-token")
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

fn header<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Checks the auth header each real API uses: bearer tokens for
/// Hetzner/Vultr/DO/Linode/Hostinger, `X-Ovh-Consumer` for OVH (signatures
/// are NOT verified), `X-Auth-Token` for Scaleway.
async fn auth_gate(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    let headers = req.headers();
    let ok = if path.starts_with(INSPECT_PREFIX) {
        true
    } else if path.starts_with("/1.0") {
        token_ok(header(headers, "x-ovh-consumer"))
    } else if path.starts_with("/instance") || path.starts_with("/iam") || path.starts_with("/vpc")
    {
        token_ok(header(headers, "x-auth-token"))
    } else {
        token_ok(bearer(headers))
    };
    if !ok {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": "unauthorized" })),
        )
            .into_response();
    }
    next.run(req).await
}

/// Binds an ephemeral local port and serves `router()` in the background.
/// Returns the bare `http://127.0.0.1:<port>` root — each adapter's `base`
/// includes its own version segment (`/v1`, `/v2`, `/1.0`), so callers pass
/// `with_base_url(format!("{root}/v1"))` etc.
pub async fn spawn() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock listener");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router())
            .await
            .expect("mock server crashed");
    });
    format!("http://{addr}")
}
