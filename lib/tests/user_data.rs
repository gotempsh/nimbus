//! First-boot user data: every adapter either delivers the script to its
//! provider in the wire format that provider's API expects, or refuses the
//! create outright. Silently dropping it is the failure these tests exist to
//! prevent — the server would come up without what the script installs and
//! nothing would say why.
//!
//! nimbus-mock records what each create call carried and serves it back on
//! `GET /_mock/{provider}/instances/{id}/user-data`; for the providers that
//! only accept base64 it rejects anything else, as the real APIs do.

use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Hostinger, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    CloudProvider, CreateInstance, Error, InstanceStatus,
};
use serde_json::Value;

const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest test@example.com";

/// Multi-line, with a shebang, quotes, `=` padding look-alikes and non-ASCII
/// text — the things an encoding bug would mangle.
const SCRIPT: &str = "#!/bin/sh\nset -eu\n# première étape — \"quoted\" & 'single'\nexport JOIN_CODE=fake-single-use-code==\ncurl -fsSL https://install.example.invalid/setup.sh | sh\n";

fn create_req(region: &str, instance_type: &str, image: &str, user_data: bool) -> CreateInstance {
    CreateInstance {
        name: "nimbus-userdata".into(),
        region: region.into(),
        instance_type: instance_type.into(),
        image: image.into(),
        ssh_public_key: SSH_KEY.into(),
        network_id: None,
        user_data: user_data.then(|| SCRIPT.to_owned()),
    }
}

/// What the mock recorded for an instance, or `None` if it received no user
/// data for it.
async fn recorded(base: &str, provider: &str, instance_id: &str) -> Option<Value> {
    let resp = reqwest::get(format!(
        "{base}/_mock/{provider}/instances/{instance_id}/user-data"
    ))
    .await
    .expect("inspect request");
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return None;
    }
    assert!(resp.status().is_success(), "inspect: {}", resp.status());
    Some(resp.json().await.expect("inspect json"))
}

/// Creates one instance with the script and one without, and checks the mock
/// saw exactly that.
async fn assert_delivers(
    base: &str,
    provider: &dyn CloudProvider,
    (region, instance_type, image): (&str, &str, &str),
    encoding: &str,
) {
    let id = provider.id();
    let with = provider
        .create_instance(create_req(region, instance_type, image, true))
        .await
        .unwrap_or_else(|e| panic!("{id}: create with user_data: {e}"));
    let record = recorded(base, id, &with.id)
        .await
        .unwrap_or_else(|| panic!("{id}: user data never reached the provider"));
    assert_eq!(
        record["script"], SCRIPT,
        "{id}: script arrived altered: {record}"
    );
    assert_eq!(record["encoding"], encoding, "{id}: wrong wire encoding");
    let received = record["received"].as_str().expect("received is a string");
    match encoding {
        "plain" => assert_eq!(received, SCRIPT, "{id}: must be sent as plain text"),
        "base64" => assert_ne!(received, SCRIPT, "{id}: must not be sent as plain text"),
        other => panic!("unexpected encoding {other}"),
    }

    let without = provider
        .create_instance(create_req(region, instance_type, image, false))
        .await
        .unwrap_or_else(|e| panic!("{id}: create without user_data: {e}"));
    assert!(
        recorded(base, id, &without.id).await.is_none(),
        "{id}: sent user data that was never asked for"
    );
}

#[tokio::test]
async fn hetzner_sends_user_data_plain() {
    let base = nimbus_mock::spawn().await;
    let p = Hetzner::new("mock-token").with_base_url(format!("{base}/v1"));
    assert_delivers(&base, &p, ("fsn1", "cx22", "ubuntu-24.04"), "plain").await;
}

#[tokio::test]
async fn vultr_sends_user_data_base64() {
    let base = nimbus_mock::spawn().await;
    let p = Vultr::new("mock-token").with_base_url(format!("{base}/v2"));
    assert_delivers(&base, &p, ("ewr", "vc2-1c-1gb", "2284"), "base64").await;
}

#[tokio::test]
async fn ovh_sends_user_data_plain() {
    let base = nimbus_mock::spawn().await;
    let p = Ovh::new(OvhRegion::Eu, "ak", "as", "ck", "proj").with_base_url(format!("{base}/1.0"));
    assert_delivers(&base, &p, ("GRA", "d2-2", "img-uuid"), "plain").await;
}

#[tokio::test]
async fn digitalocean_sends_user_data_plain() {
    let base = nimbus_mock::spawn().await;
    let p = DigitalOcean::new("mock-token").with_base_url(format!("{base}/do/v2"));
    assert_delivers(
        &base,
        &p,
        ("nyc3", "s-1vcpu-1gb", "ubuntu-24-04-x64"),
        "plain",
    )
    .await;
}

#[tokio::test]
async fn linode_sends_user_data_base64() {
    let base = nimbus_mock::spawn().await;
    let p = Linode::new("mock-token").with_base_url(format!("{base}/v4"));
    assert_delivers(
        &base,
        &p,
        ("us-east", "g6-nanode-1", "linode/ubuntu24.04"),
        "base64",
    )
    .await;
}

/// Scaleway's create-server has no user-data field; the adapter has to set
/// the `cloud-init` key on the stopped server and only then boot it. The
/// mock records the script only if it arrived before that first boot.
#[tokio::test]
async fn scaleway_sets_user_data_before_first_boot() {
    let base = nimbus_mock::spawn().await;
    let p = Scaleway::new("mock-secret", "proj", "fr-par-1").with_base_url(base.clone());
    let target = ("fr-par-1", "DEV1-S", "img-uuid");
    assert_delivers(&base, &p, target, "plain").await;

    let booted = p
        .create_instance(create_req(target.0, target.1, target.2, true))
        .await
        .expect("create");
    assert_eq!(
        booted.status,
        InstanceStatus::Running,
        "server must still be powered on after its user data is set"
    );
}

/// Hostinger's purchase API takes no user data, and its create is a real,
/// non-refundable purchase — so the adapter must refuse before sending
/// anything rather than buy a server that will boot without the script.
#[tokio::test]
async fn hostinger_refuses_user_data_before_purchasing() {
    let base = nimbus_mock::spawn().await;
    let p = Hostinger::new("mock-token").with_base_url(format!("{base}/api"));
    let err = p
        .create_instance(create_req(
            "19",
            "hostingercom-vps-kvm2-usd-1m",
            "1130",
            true,
        ))
        .await
        .expect_err("hostinger must reject user_data");
    match &err {
        Error::InvalidRequest(message) => {
            assert!(
                message.contains("hostinger"),
                "must name the provider: {message}"
            );
            assert!(
                message.contains("user_data"),
                "must name the option: {message}"
            );
        }
        other => panic!("expected Error::InvalidRequest, got: {other:?}"),
    }
    assert!(
        p.list_instances().await.expect("list").is_empty(),
        "nothing may be purchased when user_data is refused"
    );

    // The same request without user data is accepted.
    p.create_instance(create_req(
        "19",
        "hostingercom-vps-kvm2-usd-1m",
        "1130",
        false,
    ))
    .await
    .expect("create without user_data");
}

/// The base64-only mocks reject plain text, so an adapter regressing to
/// sending the raw script fails its test above instead of passing unnoticed.
#[tokio::test]
async fn base64_only_mocks_reject_plain_user_data() {
    let base = nimbus_mock::spawn().await;
    let client = reqwest::Client::new();

    let vultr = client
        .post(format!("{base}/v2/instances"))
        .bearer_auth("mock-token")
        .json(&serde_json::json!({
            "region": "ewr", "plan": "vc2-1c-1gb", "label": "x", "user_data": SCRIPT,
        }))
        .send()
        .await
        .expect("vultr request");
    assert_eq!(vultr.status(), reqwest::StatusCode::BAD_REQUEST);

    let linode = client
        .post(format!("{base}/v4/linode/instances"))
        .bearer_auth("mock-token")
        .json(&serde_json::json!({
            "region": "us-east", "type": "g6-nanode-1", "label": "x",
            "metadata": { "user_data": SCRIPT },
        }))
        .send()
        .await
        .expect("linode request");
    assert_eq!(linode.status(), reqwest::StatusCode::BAD_REQUEST);
}
