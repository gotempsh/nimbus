//! In-memory mock servers for Hetzner, Vultr, and OVH — enough of each REST
//! surface for `nimbus-cloud`'s adapters to run their full instance/
//! volume/network CRUD flow against, offline and for free. Not a faithful
//! emulation of either API — just the fields and status codes the nimbus
//! clients actually read.

mod hetzner;
mod ovh;
mod vultr;

use axum::Router;

/// Router serving all three mocks on distinct path prefixes
/// (`/v1` Hetzner, `/v2` Vultr, `/1.0` OVH) so one bound port covers all
/// three providers — point each adapter's `with_base_url` at
/// `http://127.0.0.1:<port>` plus the provider's own path prefix removed
/// (each adapter already appends its own version prefix).
pub fn router() -> Router {
    Router::new().merge(hetzner::router()).merge(vultr::router()).merge(ovh::router())
}

/// Binds an ephemeral local port and serves `router()` in the background.
/// Returns the bare `http://127.0.0.1:<port>` root — each adapter's `base`
/// includes its own version segment (`/v1`, `/v2`, `/1.0`), so callers pass
/// `with_base_url(format!("{root}/v1"))` etc. See `mock/README.md`.
pub async fn spawn() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind mock listener");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, router()).await.expect("mock server crashed");
    });
    format!("http://{addr}")
}
