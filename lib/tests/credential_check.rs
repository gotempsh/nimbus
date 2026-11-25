//! `check_credential` against nimbus-mock: what each adapter reports for a
//! working credential, a rejected one, and one that authenticates but cannot
//! create instances — plus the providers where "can it create?" has no
//! honest answer and must come back `Unknown`.
//!
//! Every check must be read-only: the mock logs each request it serves, and
//! each test asserts nothing but GETs were sent.

use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Hostinger, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    Capability, CloudProvider, CredentialCheck, Error,
};

fn hetzner(base: &str, token: &str) -> Hetzner {
    Hetzner::new(token).with_base_url(format!("{base}/v1"))
}
fn vultr(base: &str, token: &str) -> Vultr {
    Vultr::new(token).with_base_url(format!("{base}/v2"))
}
fn ovh(base: &str, consumer_key: &str, project: &str) -> Ovh {
    Ovh::new(
        OvhRegion::Eu,
        "app-key",
        "app-secret",
        consumer_key,
        project,
    )
    .with_base_url(format!("{base}/1.0"))
}
fn digitalocean(base: &str, token: &str) -> DigitalOcean {
    DigitalOcean::new(token).with_base_url(format!("{base}/do/v2"))
}
fn linode(base: &str, token: &str) -> Linode {
    Linode::new(token).with_base_url(format!("{base}/v4"))
}
fn scaleway(base: &str, secret_key: &str) -> Scaleway {
    Scaleway::new(secret_key, "test-project", "fr-par-1").with_base_url(base)
}
fn hostinger(base: &str, token: &str) -> Hostinger {
    Hostinger::new(token).with_base_url(format!("{base}/api"))
}

/// One adapter per provider, all holding the same credential value.
fn all(base: &str, credential: &str) -> Vec<Box<dyn CloudProvider>> {
    vec![
        Box::new(hetzner(base, credential)),
        Box::new(vultr(base, credential)),
        Box::new(ovh(base, credential, "test-project")),
        Box::new(digitalocean(base, credential)),
        Box::new(linode(base, credential)),
        Box::new(scaleway(base, credential)),
        Box::new(hostinger(base, credential)),
    ]
}

/// Runs the check and asserts it sent nothing but GETs.
async fn check(base: &str, provider: &dyn CloudProvider) -> CredentialCheck {
    let before = requests(base).await.len();
    let result = provider
        .check_credential()
        .await
        .unwrap_or_else(|e| panic!("{}: check_credential: {e}", provider.id()));
    let sent = requests(base).await.split_off(before);
    assert!(!sent.is_empty(), "{}: check made no request", provider.id());
    assert_eq!(
        sent.len(),
        result.checks.len(),
        "{}: `checks` must list every request made: {sent:?} vs {:?}",
        provider.id(),
        result.checks
    );
    for (method, path) in &sent {
        assert_eq!(
            method,
            "GET",
            "{}: a credential check must never write, but sent {method} {path}",
            provider.id()
        );
    }
    assert!(!result.detail.is_empty(), "{}: empty detail", provider.id());
    result
}

async fn requests(base: &str) -> Vec<(String, String)> {
    let log: serde_json::Value = reqwest::get(format!("{base}/_mock/requests"))
        .await
        .expect("request log")
        .json()
        .await
        .expect("request log json");
    log.as_array()
        .expect("array")
        .iter()
        .map(|r| {
            (
                r["method"].as_str().unwrap_or_default().to_owned(),
                r["path"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

// ---------- authenticated ----------

#[tokio::test]
async fn working_credentials_authenticate_everywhere() {
    let base = nimbus_mock::spawn().await;
    for provider in all(&base, "mock-credential") {
        let result = check(&base, provider.as_ref()).await;
        assert!(result.authenticated, "{}: {result:?}", provider.id());
        assert_ne!(
            result.can_create_instances,
            Capability::No,
            "{}: {result:?}",
            provider.id()
        );
        provider.verify().await.expect("verify");
    }
}

/// Where the API exposes the credential's permissions, a working credential
/// is positively confirmed as able to create.
#[tokio::test]
async fn providers_with_introspection_confirm_create_access() {
    let base = nimbus_mock::spawn().await;
    let confirmed: Vec<Box<dyn CloudProvider>> = vec![
        Box::new(vultr(&base, "mock-credential")),
        Box::new(ovh(&base, "mock-credential", "test-project")),
        Box::new(linode(&base, "mock-credential")),
        Box::new(hostinger(&base, "mock-credential")),
    ];
    for provider in confirmed {
        let result = check(&base, provider.as_ref()).await;
        assert_eq!(
            result.can_create_instances,
            Capability::Yes,
            "{}: {result:?}",
            provider.id()
        );
    }
}

// ---------- unknown write access ----------

/// Hetzner, DigitalOcean and Scaleway cannot report what a credential may
/// do. The honest answer is `Unknown` — not a guess in either direction —
/// and the detail has to say that it could not be determined.
#[tokio::test]
async fn providers_without_introspection_report_unknown() {
    let base = nimbus_mock::spawn().await;
    let unknowable: Vec<Box<dyn CloudProvider>> = vec![
        Box::new(hetzner(&base, "mock-credential")),
        Box::new(digitalocean(&base, "mock-credential")),
        Box::new(scaleway(&base, "mock-credential")),
    ];
    for provider in unknowable {
        let result = check(&base, provider.as_ref()).await;
        assert!(result.authenticated, "{}: {result:?}", provider.id());
        assert_eq!(
            result.can_create_instances,
            Capability::Unknown,
            "{}: {result:?}",
            provider.id()
        );
        assert!(
            result.detail.contains("cannot be confirmed"),
            "{}: detail must say what could not be determined: {}",
            provider.id(),
            result.detail
        );
    }
}

// ---------- rejected ----------

/// A rejected credential is a verdict (`Ok`, `authenticated: false`), not an
/// error, and `verify()` turns it into `Error::Auth`.
#[tokio::test]
async fn rejected_credentials_are_reported_not_raised() {
    let base = nimbus_mock::spawn().await;
    for provider in all(&base, "bad-token") {
        let result = check(&base, provider.as_ref()).await;
        assert!(!result.authenticated, "{}: {result:?}", provider.id());
        assert_eq!(
            result.can_create_instances,
            Capability::No,
            "{}: a rejected credential cannot create anything",
            provider.id()
        );
        assert!(
            !result.detail.contains("bad-token"),
            "{}: detail leaked the credential: {}",
            provider.id(),
            result.detail
        );
        match provider.verify().await {
            Err(Error::Auth { provider: p }) => assert_eq!(p, provider.id()),
            other => panic!(
                "{}: verify() should be Error::Auth: {other:?}",
                provider.id()
            ),
        }
    }
}

/// Listing regions is not evidence: Scaleway's is a static table and Vultr
/// serves its regions without a credential. `verify()` used to pass a
/// rejected Scaleway key because of that.
#[tokio::test]
async fn verify_does_not_rely_on_listing_regions() {
    let base = nimbus_mock::spawn().await;
    let scaleway = scaleway(&base, "bad-token");
    scaleway
        .regions()
        .await
        .expect("regions needs no credential on Scaleway");
    assert!(matches!(scaleway.verify().await, Err(Error::Auth { .. })));
    for provider in all(&base, "bad-token") {
        let result = check(&base, provider.as_ref()).await;
        assert!(
            result.checks.iter().all(|c| !c.contains("region")),
            "{}: must not use a regions endpoint as evidence: {:?}",
            provider.id(),
            result.checks
        );
    }
}

// ---------- authenticates but cannot create ----------

#[tokio::test]
async fn credentials_that_cannot_create_are_reported_as_such() {
    let base = nimbus_mock::spawn().await;
    let read_only = "read-only-token";
    let cases: Vec<(Box<dyn CloudProvider>, &str)> = vec![
        (Box::new(vultr(&base, read_only)), "provisioning"),
        (
            Box::new(ovh(&base, read_only, "test-project")),
            "access rules",
        ),
        (Box::new(linode(&base, read_only)), "linodes:read_write"),
        (Box::new(linode(&base, "restricted-token")), "add_linodes"),
        (Box::new(digitalocean(&base, read_only)), "locked"),
        (Box::new(hostinger(&base, read_only)), "payment method"),
        (
            Box::new(scaleway(&base, "no-access-token")),
            "refused access",
        ),
        (
            Box::new(ovh(&base, "mock-credential", "missing-project")),
            "not found",
        ),
        (
            Box::new(ovh(&base, "mock-credential", "suspended-project")),
            "suspended",
        ),
    ];
    for (provider, expected_reason) in cases {
        let result = check(&base, provider.as_ref()).await;
        assert!(
            result.authenticated,
            "{}: the credential itself is valid: {result:?}",
            provider.id()
        );
        assert_eq!(
            result.can_create_instances,
            Capability::No,
            "{}: {result:?}",
            provider.id()
        );
        assert!(
            result.detail.contains(expected_reason),
            "{}: detail should explain why ({expected_reason}): {}",
            provider.id(),
            result.detail
        );
        // Authenticated, so verify() still passes: it answers a narrower question.
        provider.verify().await.expect("verify");
    }
}

// ---------- the check itself cannot run ----------

#[tokio::test]
async fn unreachable_provider_is_an_error_not_a_verdict() {
    for provider in all("http://127.0.0.1:1", "mock-credential") {
        match provider.check_credential().await {
            Err(Error::Transport(_)) => {}
            other => panic!("{}: expected Error::Transport: {other:?}", provider.id()),
        }
    }
}

/// The report serializes to the stable, machine-readable shape the CLI
/// prints: capability is a lowercase string, never a bare boolean.
#[tokio::test]
async fn report_serializes_to_stable_json() {
    let base = nimbus_mock::spawn().await;
    let result = check(&base, &hetzner(&base, "mock-credential")).await;
    let json = serde_json::to_value(&result).expect("serialize");
    assert_eq!(json["authenticated"], true);
    assert_eq!(json["can_create_instances"], "unknown");
    assert_eq!(json["checks"], serde_json::json!(["GET /servers"]));
    assert!(json["detail"].is_string());
    for (capability, text) in [
        (Capability::Yes, "yes"),
        (Capability::No, "no"),
        (Capability::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(capability).expect("serialize"), text);
    }
}
