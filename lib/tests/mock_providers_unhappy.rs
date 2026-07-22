//! Unhappy paths for every provider against nimbus-mock: rejected
//! credentials (401 → Error::Auth), unknown resource ids (404), validation
//! failures on create (provider-shaped 4xx error bodies surfaced in
//! Error::Api), and an unreachable host (Error::Transport).

use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    CloudProvider, CreateInstance, Error,
};

const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest test@example.com";

fn create_req(region: &str, image: &str) -> CreateInstance {
    CreateInstance {
        name: "nimbus-unhappy".into(),
        region: region.into(),
        instance_type: "no-such-type".into(),
        image: image.into(),
        ssh_public_key: SSH_KEY.into(),
        network_id: None,
        user_data: None,
    }
}

fn assert_auth(err: Error, provider: &str) {
    assert!(
        matches!(err, Error::Auth { .. }),
        "{provider}: expected Error::Auth, got: {err:?}"
    );
}

fn assert_not_found(err: Error, provider: &str) {
    match err {
        Error::NotFound { .. } => {}
        Error::Api { status: 404, .. } => {}
        other => panic!("{provider}: expected 404/NotFound, got: {other:?}"),
    }
}

fn assert_validation(err: Error, provider: &str) {
    match err {
        Error::Api {
            status, message, ..
        } => {
            assert!(
                (400..500).contains(&status),
                "{provider}: expected 4xx, got {status}"
            );
            assert!(
                !message.is_empty(),
                "{provider}: validation error must carry the provider's message"
            );
        }
        other => panic!("{provider}: expected Error::Api, got: {other:?}"),
    }
}

// ---------- rejected credentials ----------

#[tokio::test]
async fn hetzner_bad_token() {
    let base = nimbus_mock::spawn().await;
    let p = Hetzner::new("bad-token").with_base_url(format!("{base}/v1"));
    assert_auth(p.regions().await.unwrap_err(), "hetzner");
    assert_auth(p.verify().await.unwrap_err(), "hetzner");
}

#[tokio::test]
async fn vultr_bad_token() {
    let base = nimbus_mock::spawn().await;
    let p = Vultr::new("bad-token").with_base_url(format!("{base}/v2"));
    assert_auth(p.regions().await.unwrap_err(), "vultr");
}

#[tokio::test]
async fn ovh_bad_token() {
    let base = nimbus_mock::spawn().await;
    let p = Ovh::new(OvhRegion::Eu, "ak", "as", "bad-token", "proj")
        .with_base_url(format!("{base}/1.0"));
    assert_auth(p.regions().await.unwrap_err(), "ovh");
}

#[tokio::test]
async fn digitalocean_bad_token() {
    let base = nimbus_mock::spawn().await;
    let p = DigitalOcean::new("bad-token").with_base_url(format!("{base}/do/v2"));
    assert_auth(p.regions().await.unwrap_err(), "digitalocean");
}

#[tokio::test]
async fn linode_bad_token() {
    let base = nimbus_mock::spawn().await;
    let p = Linode::new("bad-token").with_base_url(format!("{base}/v4"));
    assert_auth(p.regions().await.unwrap_err(), "linode");
}

#[tokio::test]
async fn scaleway_bad_token() {
    let base = nimbus_mock::spawn().await;
    // regions() is a static catalog for Scaleway — hit an authenticated
    // endpoint, like the fleet bridge's verify() does.
    let p = Scaleway::new("bad-token", "proj", "fr-par-1").with_base_url(base);
    assert_auth(p.instance_types("fr-par-1").await.unwrap_err(), "scaleway");
}

// ---------- unknown resource ids ----------

#[tokio::test]
async fn get_unknown_instance_is_404() {
    let base = nimbus_mock::spawn().await;
    let zero_uuid = "00000000-0000-0000-0000-000000000000";

    let hetzner = Hetzner::new("t").with_base_url(format!("{base}/v1"));
    assert_not_found(hetzner.get_instance("424242").await.unwrap_err(), "hetzner");

    let vultr = Vultr::new("t").with_base_url(format!("{base}/v2"));
    assert_not_found(vultr.get_instance(zero_uuid).await.unwrap_err(), "vultr");

    let ovh =
        Ovh::new(OvhRegion::Eu, "ak", "as", "ck", "proj").with_base_url(format!("{base}/1.0"));
    assert_not_found(ovh.get_instance("424242").await.unwrap_err(), "ovh");

    let digitalocean = DigitalOcean::new("t").with_base_url(format!("{base}/do/v2"));
    assert_not_found(
        digitalocean.get_instance("424242").await.unwrap_err(),
        "digitalocean",
    );

    let linode = Linode::new("t").with_base_url(format!("{base}/v4"));
    assert_not_found(linode.get_instance("424242").await.unwrap_err(), "linode");

    let scaleway = Scaleway::new("t", "proj", "fr-par-1").with_base_url(base);
    assert_not_found(
        scaleway.get_instance(zero_uuid).await.unwrap_err(),
        "scaleway",
    );
}

// ---------- validation failure on create ----------

#[tokio::test]
async fn create_with_invalid_type_surfaces_provider_error() {
    let base = nimbus_mock::spawn().await;

    let hetzner = Hetzner::new("t").with_base_url(format!("{base}/v1"));
    assert_validation(
        hetzner
            .create_instance(create_req("fsn1", "ubuntu-24.04"))
            .await
            .unwrap_err(),
        "hetzner",
    );

    let vultr = Vultr::new("t").with_base_url(format!("{base}/v2"));
    assert_validation(
        vultr
            .create_instance(create_req("ewr", "2284"))
            .await
            .unwrap_err(),
        "vultr",
    );

    let ovh =
        Ovh::new(OvhRegion::Eu, "ak", "as", "ck", "proj").with_base_url(format!("{base}/1.0"));
    assert_validation(
        ovh.create_instance(create_req("GRA", "img-uuid"))
            .await
            .unwrap_err(),
        "ovh",
    );

    let digitalocean = DigitalOcean::new("t").with_base_url(format!("{base}/do/v2"));
    assert_validation(
        digitalocean
            .create_instance(create_req("nyc3", "ubuntu-24-04-x64"))
            .await
            .unwrap_err(),
        "digitalocean",
    );

    let linode = Linode::new("t").with_base_url(format!("{base}/v4"));
    assert_validation(
        linode
            .create_instance(create_req("us-east", "linode/ubuntu24.04"))
            .await
            .unwrap_err(),
        "linode",
    );

    let scaleway = Scaleway::new("t", "proj", "fr-par-1").with_base_url(base);
    assert_validation(
        scaleway
            .create_instance(create_req("fr-par-1", "img-uuid"))
            .await
            .unwrap_err(),
        "scaleway",
    );
}

// ---------- unreachable host ----------

#[tokio::test]
async fn unreachable_host_is_transport_error() {
    // Nothing listens on port 1; the transport path is shared code in every
    // adapter, so one provider covers it.
    let p = Hetzner::new("t").with_base_url("http://127.0.0.1:1/v1");
    match p.regions().await.unwrap_err() {
        Error::Transport(_) => {}
        other => panic!("expected Error::Transport, got: {other:?}"),
    }
}
