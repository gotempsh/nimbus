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
    tracing::info!("  Hetzner:      http://{addr}/v1     (HCLOUD_TOKEN: any non-empty string except bad-token)");
    tracing::info!("  Vultr:        http://{addr}/v2     (VULTR_API_KEY: any non-empty string except bad-token)");
    tracing::info!("  OVH:          http://{addr}/1.0    (OVH_* creds: any non-empty strings; bad-token rejected)");
    tracing::info!(
        "  DigitalOcean: http://{addr}/do/v2  (DIGITALOCEAN_TOKEN: any non-empty string)"
    );
    tracing::info!("  Linode:       http://{addr}/v4     (LINODE_TOKEN: any non-empty string except bad-token)");
    tracing::info!("  Scaleway:     http://{addr}        (SCW_* creds: any non-empty strings; bad-token rejected)");
    axum::serve(listener, nimbus_mock::router())
        .await
        .expect("server");
}
