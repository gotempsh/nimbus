//! End-to-end CRUD flow for each provider against nimbus-mock — proves the
//! adapters parse real-shaped responses correctly without touching a live
//! cloud account or spending money.

use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    CloudProvider, CreateInstance, CreateNetwork, CreateVolume,
};

const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest test@example.com";

async fn exercise(provider: &dyn CloudProvider, region: &str, instance_type: &str, image: &str) {
    let regions = provider.regions().await.expect("regions");
    assert!(
        !regions.is_empty(),
        "{}: expected at least one region",
        provider.id()
    );

    let types = provider
        .instance_types(region)
        .await
        .expect("instance_types");
    assert!(
        !types.is_empty(),
        "{}: expected at least one instance type",
        provider.id()
    );
    assert!(
        !types[0].currency.is_empty(),
        "{}: instance type must carry a currency",
        provider.id()
    );

    let images = provider.images(region).await.expect("images");
    assert!(
        !images.is_empty(),
        "{}: expected at least one image",
        provider.id()
    );

    provider.verify().await.expect("verify");

    let instance = provider
        .create_instance(CreateInstance {
            name: "nimbus-test".into(),
            region: region.into(),
            instance_type: instance_type.into(),
            image: image.into(),
            ssh_public_key: SSH_KEY.into(),
            network_id: None,
            user_data: None,
        })
        .await
        .expect("create_instance");

    let fetched = provider
        .get_instance(&instance.id)
        .await
        .expect("get_instance");
    assert_eq!(fetched.id, instance.id);

    let listed = provider.list_instances().await.expect("list_instances");
    assert!(listed.iter().any(|i| i.id == instance.id));

    let volume = provider
        .create_volume(CreateVolume {
            name: "nimbus-vol".into(),
            region: region.into(),
            size_gb: 10,
            instance_id: None,
        })
        .await
        .expect("create_volume");
    provider
        .attach_volume(&volume.id, &instance.id)
        .await
        .expect("attach_volume");
    let volumes = provider.list_volumes().await.expect("list_volumes");
    assert!(volumes.iter().any(|v| v.id == volume.id));
    provider
        .detach_volume(&volume.id)
        .await
        .expect("detach_volume");
    provider
        .delete_volume(&volume.id)
        .await
        .expect("delete_volume");

    let network = provider
        .create_network(CreateNetwork {
            name: "nimbus-net".into(),
            region: region.into(),
            ip_range: "10.0.0.0/16".into(),
        })
        .await
        .expect("create_network");
    let networks = provider.list_networks().await.expect("list_networks");
    assert!(networks.iter().any(|n| n.id == network.id));
    provider
        .delete_network(&network.id)
        .await
        .expect("delete_network");

    provider
        .delete_instance(&instance.id)
        .await
        .expect("delete_instance");
}

#[tokio::test]
async fn hetzner_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = Hetzner::new("mock-token").with_base_url(format!("{base}/v1"));
    exercise(&provider, "fsn1", "cx22", "ubuntu-24.04").await;
}

#[tokio::test]
async fn vultr_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = Vultr::new("mock-token").with_base_url(format!("{base}/v2"));
    exercise(&provider, "ewr", "vc2-1c-1gb", "ubuntu-24.04").await;
}

#[tokio::test]
async fn ovh_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = Ovh::new(
        OvhRegion::Eu,
        "app-key",
        "app-secret",
        "consumer-key",
        "test-project",
    )
    .with_base_url(format!("{base}/1.0"));
    exercise(&provider, "GRA", "d2-2", "ubuntu-24.04").await;
}

#[tokio::test]
async fn digitalocean_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = DigitalOcean::new("mock-token").with_base_url(format!("{base}/do/v2"));
    exercise(&provider, "nyc3", "s-1vcpu-1gb", "ubuntu-24-04-x64").await;
}

#[tokio::test]
async fn linode_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = Linode::new("mock-token").with_base_url(format!("{base}/v4"));
    exercise(&provider, "us-east", "g6-nanode-1", "linode/ubuntu24.04").await;
}

#[tokio::test]
async fn scaleway_full_flow() {
    let base = nimbus_mock::spawn().await;
    let provider = Scaleway::new("mock-secret", "test-project", "fr-par-1").with_base_url(base);
    exercise(
        &provider,
        "fr-par-1",
        "DEV1-S",
        "11111111-2222-3333-4444-555555555555",
    )
    .await;
}
