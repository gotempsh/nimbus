//! In-memory mock servers for Hetzner, Vultr, and OVH — enough of each REST
//! surface for `nimbus-cloud`'s adapters to run their full instance/
//! volume/network CRUD flow against, offline and for free. Not a faithful
//! emulation of either API — just the fields and status codes the nimbus
//! clients actually read.

mod digitalocean;
mod hetzner;
mod linode;
mod ovh;
mod scaleway;
mod vultr;

use axum::{
    body::Body,
    http::{HeaderMap, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::json;

/// Router serving all provider mocks on distinct path prefixes so one bound
/// port covers everything: `/v1` Hetzner, `/v2` Vultr, `/1.0` OVH,
/// `/do/v2` DigitalOcean (Vultr owns the bare `/v2`), `/v4` Linode, and
/// Scaleway's full `/instance/v1`···`/iam`···`/vpc` paths.
///
/// Unhappy paths: every route rejects a missing credential or the sentinel
/// token `bad-token` with 401 (each provider's real auth header is checked),
/// and each create-server handler rejects unknown instance types with the
/// provider's real error status and body shape.
pub fn router() -> Router {
    Router::new()
        .merge(hetzner::router())
        .merge(vultr::router())
        .merge(ovh::router())
        .merge(digitalocean::router())
        .merge(linode::router())
        .merge(scaleway::router())
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
/// Hetzner/Vultr/DO/Linode, `X-Ovh-Consumer` for OVH (signatures are NOT
/// verified), `X-Auth-Token` for Scaleway.
async fn auth_gate(req: Request<Body>, next: Next) -> Response {
    let path = req.uri().path();
    let headers = req.headers();
    let ok = if path.starts_with("/1.0") {
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
/// `with_base_url(format!("{root}/v1"))` etc. See `mock/README.md`.
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
