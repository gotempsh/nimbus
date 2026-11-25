//! The provider catalog (`nimbus_cloud::PROVIDERS`) is what a caller reads
//! to decide whether a provider is safe to offer. These tests hold every
//! caveat in it to what the adapter actually does against nimbus-mock, so
//! the data cannot drift from the code.

use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Hostinger, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    CloudProvider, CreateInstance, Error, ProviderInfo, PROVIDERS,
};

const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest test@example.com";

/// An adapter pointed at the mock, plus a (region, type, image) it accepts.
struct Target {
    provider: Box<dyn CloudProvider>,
    region: &'static str,
    instance_type: &'static str,
    image: &'static str,
}

fn target(id: &str, base: &str) -> Target {
    let (provider, region, instance_type, image): (Box<dyn CloudProvider>, _, _, _) = match id {
        "hetzner" => (
            Box::new(Hetzner::new("t").with_base_url(format!("{base}/v1"))),
            "fsn1",
            "cx22",
            "ubuntu-24.04",
        ),
        "vultr" => (
            Box::new(Vultr::new("t").with_base_url(format!("{base}/v2"))),
            "ewr",
            "vc2-1c-1gb",
            "2284",
        ),
        "ovh" => (
            Box::new(
                Ovh::new(OvhRegion::Eu, "ak", "as", "ck", "proj")
                    .with_base_url(format!("{base}/1.0")),
            ),
            "GRA",
            "d2-2",
            "img-uuid",
        ),
        "digitalocean" => (
            Box::new(DigitalOcean::new("t").with_base_url(format!("{base}/do/v2"))),
            "nyc3",
            "s-1vcpu-1gb",
            "ubuntu-24-04-x64",
        ),
        "scaleway" => (
            Box::new(Scaleway::new("t", "proj", "fr-par-1").with_base_url(base)),
            "fr-par-1",
            "DEV1-S",
            "img-uuid",
        ),
        "linode" => (
            Box::new(Linode::new("t").with_base_url(format!("{base}/v4"))),
            "us-east",
            "g6-nanode-1",
            "linode/ubuntu24.04",
        ),
        "hostinger" => (
            Box::new(Hostinger::new("t").with_base_url(format!("{base}/api"))),
            "19",
            "hostingercom-vps-kvm2-usd-1m",
            "1130",
        ),
        other => panic!("catalog lists provider {other} but this test has no adapter for it"),
    };
    Target {
        provider,
        region,
        instance_type,
        image,
    }
}

fn create_req(t: &Target, user_data: Option<&str>) -> CreateInstance {
    CreateInstance {
        name: "nimbus-catalog".into(),
        region: t.region.into(),
        instance_type: t.instance_type.into(),
        image: t.image.into(),
        ssh_public_key: SSH_KEY.into(),
        network_id: None,
        user_data: user_data.map(str::to_owned),
    }
}

#[tokio::test]
async fn every_adapter_is_in_the_catalog_under_its_own_id() {
    let base = nimbus_mock::spawn().await;
    assert_eq!(PROVIDERS.len(), 7, "a provider was added or removed");
    for info in PROVIDERS {
        let t = target(info.id, &base);
        assert_eq!(t.provider.id(), info.id);
        let via_trait: &ProviderInfo = t.provider.info().expect("adapter has catalog info");
        assert_eq!(via_trait, info);
    }
}

#[tokio::test]
async fn price_and_spec_caveats_match_what_instance_types_returns() {
    let base = nimbus_mock::spawn().await;
    for info in PROVIDERS {
        let t = target(info.id, &base);
        let types = t
            .provider
            .instance_types(t.region)
            .await
            .expect("instance_types");
        assert!(!types.is_empty(), "{}: no instance types", info.id);
        for ty in &types {
            assert_eq!(
                ty.monthly_price > 0.0,
                info.caveats.reports_prices,
                "{}: reports_prices is {} but {} has monthly_price {}",
                info.id,
                info.caveats.reports_prices,
                ty.id,
                ty.monthly_price
            );
            assert_eq!(
                ty.vcpus > 0 && ty.memory_gb > 0.0 && ty.disk_gb > 0,
                info.caveats.reports_instance_specs,
                "{}: reports_instance_specs is {} but {} has {} vcpus / {} GB / {} GB",
                info.id,
                info.caveats.reports_instance_specs,
                ty.id,
                ty.vcpus,
                ty.memory_gb,
                ty.disk_gb
            );
        }
    }
}

#[tokio::test]
async fn delete_caveat_matches_what_delete_instance_does() {
    let base = nimbus_mock::spawn().await;
    for info in PROVIDERS {
        let t = target(info.id, &base);
        let instance = t
            .provider
            .create_instance(create_req(&t, None))
            .await
            .unwrap_or_else(|e| panic!("{}: create: {e}", info.id));
        let deleted = t.provider.delete_instance(&instance.id).await;
        if info.caveats.can_delete_instances {
            deleted.unwrap_or_else(|e| panic!("{}: delete: {e}", info.id));
        } else {
            match deleted {
                Err(Error::InvalidRequest(message)) => {
                    assert!(
                        message.contains(info.id),
                        "must name the provider: {message}"
                    )
                }
                other => panic!("{}: delete should be refused: {other:?}", info.id),
            }
            // ...and the instance is still there: nothing was half-deleted.
            t.provider
                .get_instance(&instance.id)
                .await
                .expect("instance still exists");
        }
    }
}

#[tokio::test]
async fn user_data_caveat_matches_what_create_instance_does() {
    let base = nimbus_mock::spawn().await;
    for info in PROVIDERS {
        let t = target(info.id, &base);
        let created = t
            .provider
            .create_instance(create_req(&t, Some("#!/bin/sh\ntrue\n")))
            .await;
        if info.caveats.supports_user_data {
            created.unwrap_or_else(|e| panic!("{}: create with user_data: {e}", info.id));
        } else {
            assert!(
                matches!(created, Err(Error::InvalidRequest(_))),
                "{}: create with user_data must be refused: {created:?}",
                info.id
            );
        }
    }
}

/// The two caveats the README has always stated in prose, pinned as data.
#[test]
fn documented_caveats_are_encoded() {
    let hostinger = nimbus_cloud::provider_info("hostinger").expect("hostinger");
    assert!(hostinger.caveats.create_is_irreversible_purchase);
    assert!(!hostinger.caveats.can_delete_instances);
    assert!(!hostinger.caveats.supports_user_data);

    let ovh = nimbus_cloud::provider_info("ovh").expect("ovh");
    assert!(!ovh.caveats.reports_prices);

    for info in PROVIDERS {
        if !matches!(info.id, "hostinger") {
            assert!(
                !info.caveats.create_is_irreversible_purchase,
                "{}: only Hostinger's create is a purchase",
                info.id
            );
            assert!(info.caveats.can_delete_instances, "{}", info.id);
        }
    }
}

/// The catalog serializes as-is into the CLI's `providers` output.
#[test]
fn catalog_serializes_to_json() {
    let json = serde_json::to_value(PROVIDERS).expect("serialize");
    let scaleway = json
        .as_array()
        .expect("array")
        .iter()
        .find(|p| p["id"] == "scaleway")
        .expect("scaleway");
    let zone = &scaleway["credential_fields"][2];
    assert_eq!(zone["env"], "SCW_DEFAULT_ZONE");
    assert_eq!(zone["secret"], false);
    assert_eq!(zone["required"], false);
    assert_eq!(zone["default"], "fr-par-1");
    assert_eq!(scaleway["caveats"]["can_delete_instances"], true);
}
