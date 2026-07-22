use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8090);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    tracing::info!("nimbus-mock listening on http://{addr}");
    tracing::info!("  Hetzner: http://{addr}/v1  (HCLOUD_TOKEN can be any non-empty string)");
    tracing::info!("  Vultr:   http://{addr}/v2  (VULTR_API_KEY can be any non-empty string)");
    tracing::info!("  OVH:     http://{addr}/1.0 (OVH_* creds can be any non-empty strings)");
    axum::serve(listener, nimbus_mock::router())
        .await
        .expect("server");
}
