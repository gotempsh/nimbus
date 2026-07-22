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

use axum::Router;

/// Router serving all provider mocks on distinct path prefixes so one bound
/// port covers everything: `/v1` Hetzner, `/v2` Vultr, `/1.0` OVH,
/// `/do/v2` DigitalOcean (Vultr owns the bare `/v2`), `/v4` Linode, and
/// Scaleway's full `/instance/v1`···`/iam`···`/vpc` paths.
pub fn router() -> Router {
    Router::new()
        .merge(hetzner::router())
        .merge(vultr::router())
        .merge(ovh::router())
        .merge(digitalocean::router())
        .merge(linode::router())
        .merge(scaleway::router())
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
