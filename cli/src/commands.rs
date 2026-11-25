//! The parts of each command that are plain data in, data out: building an
//! adapter from resolved credentials, the `providers` and `verify` reports,
//! and turning failures into errors that say what to do next.

use crate::credentials::{login_command, resolve, ResolveError, Resolved, Source, Sources};
use crate::output::CliError;
use nimbus_cloud::{
    providers::{DigitalOcean, Hetzner, Hostinger, Linode, Ovh, OvhRegion, Scaleway, Vultr},
    Capability, Caveats, CloudProvider, CredentialCheck, ProviderInfo, PROVIDERS,
};
use serde::Serialize;
use std::sync::Arc;

fn provider_ids() -> String {
    let ids: Vec<&str> = PROVIDERS.iter().map(|p| p.id).collect();
    ids.join(", ")
}

/// Looks up the `--provider` value.
pub fn require_provider(id: Option<&str>) -> Result<&'static ProviderInfo, CliError> {
    let Some(id) = id else {
        return Err(CliError::new(
            "missing_provider",
            format!("--provider is required ({})", provider_ids()),
            "Pass --provider <id> or set NIMBUS_PROVIDER. Run `nimbus providers` to see what each one needs.",
        ));
    };
    nimbus_cloud::provider_info(id).ok_or_else(|| {
        CliError::new(
            "unknown_provider",
            format!("unknown provider '{id}' (expected {})", provider_ids()),
            "Run `nimbus providers` for the supported providers.",
        )
    })
}

/// Explains a failed credential lookup: which provider, and the exact
/// command that fixes it.
pub fn resolve_error(info: &ProviderInfo, error: ResolveError) -> CliError {
    let login = login_command(info);
    let required: Vec<&str> = info
        .credential_fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.env)
        .collect();
    match error {
        ResolveError::Missing { notes } => {
            let mut message = format!("no credentials found for {} ({})", info.name, info.id);
            for note in notes {
                message.push_str(&format!("; {note}"));
            }
            CliError::new(
                "missing_credentials",
                message,
                format!(
                    "Run `{login}` in a terminal to store them (the secret is typed with echo off and never shown), or set {}.",
                    required.join(", ")
                ),
            )
        }
        ResolveError::Unsupported { source } => CliError::new(
            "unsupported_credential",
            format!(
                "the credential for {} in {} is of a kind this version of nimbus cannot use",
                info.id,
                source.describe()
            ),
            format!("Upgrade nimbus, or run `{login}` to replace the stored credential."),
        ),
        ResolveError::Incomplete { source, missing } => CliError::new(
            "incomplete_credential",
            format!(
                "the credential for {} in {} has no value for {}",
                info.id,
                source.describe(),
                missing.join(", ")
            ),
            format!("Run `{login}` to store it again."),
        ),
    }
    .for_provider(info.id)
}

/// Builds the adapter for a provider from resolved credentials.
pub fn build_provider(
    info: &ProviderInfo,
    creds: &Resolved,
    base_url: Option<String>,
) -> Result<Arc<dyn CloudProvider>, CliError> {
    /// Applies `--base-url` when given. Every adapter has the same builder
    /// method but they share no trait for it.
    macro_rules! with_base {
        ($provider:expr) => {{
            let provider = $provider;
            Arc::new(match base_url {
                Some(base) => provider.with_base_url(base),
                None => provider,
            }) as Arc<dyn CloudProvider>
        }};
    }
    Ok(match info.id {
        "hetzner" => with_base!(Hetzner::new(creds.get("token"))),
        "vultr" => with_base!(Vultr::new(creds.get("api_key"))),
        "ovh" => with_base!(Ovh::new(
            OvhRegion::Eu,
            creds.get("application_key"),
            creds.get("application_secret"),
            creds.get("consumer_key"),
            creds.get("project_id"),
        )),
        "digitalocean" => with_base!(DigitalOcean::new(creds.get("token"))),
        "scaleway" => with_base!(Scaleway::new(
            creds.get("secret_key"),
            creds.get("project_id"),
            creds.get("zone"),
        )),
        "linode" => with_base!(Linode::new(creds.get("token"))),
        "hostinger" => with_base!(Hostinger::new(creds.get("token"))),
        other => {
            return Err(CliError::new(
                "unknown_provider",
                format!("provider '{other}' is in the catalog but has no adapter in this build"),
                "This is a bug in nimbus; please report it.",
            ))
        }
    })
}

/// One credential field as `providers` shows it: its names, never its value.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FieldReport {
    pub name: &'static str,
    pub env: &'static str,
    pub label: &'static str,
    pub secret: bool,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<&'static str>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Availability {
    /// Whether a command run now would find usable credentials.
    pub available: bool,
    /// `environment`, `credential_store` or `file`; `null` when unavailable.
    pub source: Option<Source>,
    /// Why not, when a credential exists but cannot be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// One entry of the `providers` output.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ProviderReport {
    pub id: &'static str,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub credential_fields: Vec<FieldReport>,
    pub credentials: Availability,
    pub caveats: Caveats,
    /// The command a person runs to connect this provider.
    pub login_command: String,
}

/// Describes every supported provider. Works with nothing configured, and
/// reads credential values only to learn whether they exist.
pub fn providers_report(sources: &Sources) -> Vec<ProviderReport> {
    PROVIDERS
        .iter()
        .map(|info| {
            let credentials = match resolve(info, sources) {
                Ok(resolved) => Availability {
                    available: true,
                    source: Some(resolved.source),
                    problem: None,
                },
                Err(ResolveError::Missing { .. }) => Availability {
                    available: false,
                    source: None,
                    problem: None,
                },
                Err(other) => Availability {
                    available: false,
                    source: None,
                    problem: Some(resolve_error(info, other).message),
                },
            };
            ProviderReport {
                id: info.id,
                name: info.name,
                aliases: info.aliases,
                credential_fields: info
                    .credential_fields
                    .iter()
                    .map(|f| FieldReport {
                        name: f.name,
                        env: f.env,
                        label: f.label,
                        secret: f.secret,
                        required: f.required,
                        default: f.default,
                    })
                    .collect(),
                credentials,
                caveats: info.caveats,
                login_command: login_command(info),
            }
        })
        .collect()
}

/// The `verify` output: the SDK's findings plus where the credential came
/// from and what to do next.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct VerifyReport {
    pub provider: &'static str,
    /// Where the credential that was checked came from.
    pub credential_source: Source,
    pub authenticated: bool,
    /// `yes`, `no` or `unknown`.
    pub can_create_instances: Capability,
    /// The read-only requests the answer rests on.
    pub checks: Vec<String>,
    pub detail: String,
    /// What to do next.
    pub hint: String,
}

impl VerifyReport {
    pub fn new(info: &ProviderInfo, source: Source, check: CredentialCheck) -> Self {
        let login = login_command(info);
        let replace = match source {
            Source::Environment => format!(
                "Correct the provider's environment variables (they take precedence over stored credentials), or unset them and run `{login}`."
            ),
            _ => format!("Run `{login}` to store a working credential."),
        };
        let hint = match (check.authenticated, check.can_create_instances) {
            (false, _) => format!("The credential was rejected. {replace}"),
            (true, Capability::No) => format!(
                "The credential works but cannot create instances; see `detail` for what to change. {replace}"
            ),
            (true, Capability::Yes) => {
                "The credential is ready to create instances.".to_owned()
            }
            (true, Capability::Unknown) => "The credential works. Whether it may create instances cannot be checked in advance for this provider (see `detail`), so creating one is the only test: if that is refused for lack of permission, the credential is read-only.".to_owned(),
        };
        Self {
            provider: info.id,
            credential_source: source,
            authenticated: check.authenticated,
            can_create_instances: check.can_create_instances,
            checks: check.checks,
            detail: check.detail,
            hint,
        }
    }

    /// Whether the credential can be used to create an instance, as far as
    /// can be told: `false` only when it was rejected or is known unable.
    pub fn usable(&self) -> bool {
        self.authenticated && self.can_create_instances != Capability::No
    }
}

/// Turns an SDK error into one that names the provider and the next step.
pub fn api_error(info: &ProviderInfo, source: Source, error: nimbus_cloud::Error) -> CliError {
    use nimbus_cloud::Error as E;
    let id = info.id;
    let message = error.to_string();
    let (kind, hint) = match &error {
        E::Auth { .. } => (
            "authentication_rejected",
            match source {
                Source::Environment => format!(
                    "The provider rejected the credential from the environment. Correct it, or unset it and run `{}`. `nimbus --provider {id} verify` shows what the credential can do.",
                    login_command(info)
                ),
                _ => format!(
                    "The provider rejected the stored credential. Run `{}` to replace it. `nimbus --provider {id} verify` shows what a credential can do.",
                    login_command(info)
                ),
            },
        ),
        E::NotFound { resource, .. } => (
            "not_found",
            format!("Check the id: `nimbus --provider {id} {resource} list` shows what exists."),
        ),
        E::Api { status, .. } => (
            "provider_error",
            match status {
                401 | 403 => format!(
                    "The provider refused this for lack of permission. Run `nimbus --provider {id} verify` to see what the credential can do."
                ),
                404 => "The provider does not know that resource or path. Check the ids used, and --base-url if it is set.".to_owned(),
                429 => "The provider is rate-limiting requests. Wait a minute and run the same command again.".to_owned(),
                400..=499 => format!(
                    "The provider rejected the request; its message above says which value. Valid choices: `nimbus --provider {id} regions`, `sizes <region>`, `images <region>`."
                ),
                _ => "The provider's API failed on its side. Wait and run the same command again; nothing needs changing here.".to_owned(),
            },
        ),
        E::Transport(_) => (
            "network_error",
            "The provider's API could not be reached. Check the network connection (and --base-url if it is set), then run the same command again.".to_owned(),
        ),
        E::InvalidRequest(_) => (
            "unsupported_request",
            "This provider does not support what was asked. `nimbus providers` lists each provider's caveats.".to_owned(),
        ),
    };
    CliError::new(kind, message, hint).for_provider(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::testing::{env_of, MemoryStore};
    use crate::credentials::{CredentialStore, StoredCredential};
    use nimbus_cloud::provider_info;

    fn info(id: &str) -> &'static ProviderInfo {
        provider_info(id).expect("known provider")
    }

    fn report_for<'r>(reports: &'r [ProviderReport], id: &str) -> &'r ProviderReport {
        reports
            .iter()
            .find(|r| r.id == id)
            .expect("provider listed")
    }

    #[test]
    fn providers_lists_everything_with_no_credentials_at_all() {
        let env = env_of(&[]);
        let reports = providers_report(&Sources {
            env: &env,
            stores: vec![],
        });
        let ids: Vec<&str> = reports.iter().map(|r| r.id).collect();
        assert_eq!(
            ids,
            vec![
                "hetzner",
                "vultr",
                "ovh",
                "digitalocean",
                "scaleway",
                "linode",
                "hostinger"
            ]
        );
        for report in &reports {
            assert_eq!(
                report.credentials,
                Availability {
                    available: false,
                    source: None,
                    problem: None
                },
                "{}",
                report.id
            );
            assert_eq!(
                report.login_command,
                format!("nimbus login --provider {}", report.id)
            );
            assert!(!report.credential_fields.is_empty());
        }
    }

    #[test]
    fn providers_reports_where_each_credential_comes_from() {
        let env = env_of(&[("HCLOUD_TOKEN", "fake-env-token")]);
        let os =
            MemoryStore::new(Source::CredentialStore).with("vultr", &[("api_key", "fake-os-key")]);
        let file = MemoryStore::new(Source::File)
            .with("linode", &[("token", "fake-file-token")])
            .with("scaleway", &[("secret_key", "fake-incomplete")]);
        file.entries
            .borrow_mut()
            .insert("ovh".into(), StoredCredential::Unsupported);
        let reports = providers_report(&Sources {
            env: &env,
            stores: vec![&os as &dyn CredentialStore, &file],
        });
        let source = |id: &str| report_for(&reports, id).credentials.source;
        assert_eq!(source("hetzner"), Some(Source::Environment));
        assert_eq!(source("vultr"), Some(Source::CredentialStore));
        assert_eq!(source("linode"), Some(Source::File));
        assert_eq!(source("digitalocean"), None);

        let scaleway = &report_for(&reports, "scaleway").credentials;
        assert!(!scaleway.available);
        assert!(scaleway
            .problem
            .as_deref()
            .is_some_and(|p| p.contains("SCW_DEFAULT_PROJECT_ID")));
        let ovh = &report_for(&reports, "ovh").credentials;
        assert!(!ovh.available);
        assert!(ovh.problem.is_some());
    }

    #[test]
    fn providers_output_never_contains_a_credential_value() {
        let env = env_of(&[
            ("HCLOUD_TOKEN", "fake-env-token"),
            ("OVH_APPLICATION_KEY", "fake-ovh-ak"),
            ("OVH_APPLICATION_SECRET", "fake-ovh-as"),
            ("OVH_CONSUMER_KEY", "fake-ovh-ck"),
            ("OVH_PROJECT_ID", "fake-ovh-project"),
        ]);
        let os =
            MemoryStore::new(Source::CredentialStore).with("vultr", &[("api_key", "fake-os-key")]);
        let reports = providers_report(&Sources {
            env: &env,
            stores: vec![&os as &dyn CredentialStore],
        });
        let json = serde_json::to_string(&reports).expect("json");
        for value in [
            "fake-env-token",
            "fake-ovh-ak",
            "fake-ovh-as",
            "fake-ovh-ck",
            "fake-ovh-project",
            "fake-os-key",
        ] {
            assert!(!json.contains(value), "leaked {value}: {json}");
        }
    }

    #[test]
    fn providers_encodes_the_caveats_an_agent_needs() {
        let env = env_of(&[]);
        let reports = providers_report(&Sources {
            env: &env,
            stores: vec![],
        });
        let json = serde_json::to_value(&reports).expect("json");
        let by_id = |id: &str| {
            json.as_array()
                .expect("array")
                .iter()
                .find(|p| p["id"] == id)
                .expect("listed")
                .clone()
        };
        let hostinger = by_id("hostinger");
        assert_eq!(hostinger["caveats"]["can_delete_instances"], false);
        assert_eq!(
            hostinger["caveats"]["create_is_irreversible_purchase"],
            true
        );
        assert_eq!(hostinger["caveats"]["supports_user_data"], false);
        assert_eq!(hostinger["credentials"]["source"], serde_json::Value::Null);
        let ovh = by_id("ovh");
        assert_eq!(ovh["caveats"]["reports_prices"], false);
        assert_eq!(ovh["credential_fields"][1]["env"], "OVH_APPLICATION_SECRET");
        assert_eq!(ovh["credential_fields"][1]["secret"], true);
        assert_eq!(ovh["credential_fields"][3]["secret"], false);
        let hetzner = by_id("hetzner");
        assert_eq!(hetzner["caveats"]["can_delete_instances"], true);
        assert_eq!(hetzner["caveats"]["reports_prices"], true);
        assert_eq!(hetzner["caveats"]["create_is_irreversible_purchase"], false);
        assert_eq!(by_id("digitalocean")["aliases"], serde_json::json!(["do"]));
    }

    #[test]
    fn a_missing_credential_names_the_provider_and_the_exact_login_command() {
        let err = resolve_error(info("hetzner"), ResolveError::Missing { notes: vec![] });
        assert_eq!(err.kind, "missing_credentials");
        assert_eq!(err.provider.as_deref(), Some("hetzner"));
        assert!(err.message.contains("Hetzner Cloud"), "{}", err.message);
        assert!(
            err.hint.contains("`nimbus login --provider hetzner`"),
            "{}",
            err.hint
        );
        assert!(err.hint.contains("HCLOUD_TOKEN"), "{}", err.hint);

        let err = resolve_error(
            info("scaleway"),
            ResolveError::Missing {
                notes: vec!["the keychain could not be read: locked".into()],
            },
        );
        assert!(err.message.contains("locked"), "{}", err.message);
        assert!(err.hint.contains("`nimbus login --provider scaleway`"));
        assert!(err.hint.contains("SCW_SECRET_KEY, SCW_DEFAULT_PROJECT_ID"));
        assert!(
            !err.hint.contains("SCW_DEFAULT_ZONE"),
            "optional fields are not demanded"
        );
    }

    #[test]
    fn provider_lookup_errors_say_what_is_valid() {
        let err = require_provider(None).expect_err("required");
        assert_eq!(err.kind, "missing_provider");
        assert!(err.message.contains("--provider is required"));
        let err = require_provider(Some("aws")).expect_err("unknown");
        assert_eq!(err.kind, "unknown_provider");
        assert!(err.message.contains("unknown provider 'aws'"));
        assert!(err.message.contains("hostinger"));
        assert_eq!(
            require_provider(Some("do")).expect("alias").id,
            "digitalocean"
        );
    }

    fn check(authenticated: bool, capability: Capability) -> CredentialCheck {
        CredentialCheck {
            authenticated,
            can_create_instances: capability,
            checks: vec!["GET /account".into()],
            detail: "detail".into(),
        }
    }

    #[test]
    fn verify_report_shape_and_hints() {
        let report = VerifyReport::new(
            info("hetzner"),
            Source::CredentialStore,
            check(true, Capability::Unknown),
        );
        assert!(report.usable());
        assert_eq!(
            serde_json::to_value(&report).expect("json"),
            serde_json::json!({
                "provider": "hetzner",
                "credential_source": "credential_store",
                "authenticated": true,
                "can_create_instances": "unknown",
                "checks": ["GET /account"],
                "detail": "detail",
                "hint": report.hint,
            })
        );
        assert!(report.hint.contains("cannot be checked in advance"));

        let rejected = VerifyReport::new(info("vultr"), Source::File, check(false, Capability::No));
        assert!(!rejected.usable());
        assert!(rejected.hint.contains("`nimbus login --provider vultr`"));

        let from_env = VerifyReport::new(
            info("vultr"),
            Source::Environment,
            check(false, Capability::No),
        );
        assert!(from_env.hint.contains("environment variables"));

        let read_only = VerifyReport::new(info("vultr"), Source::File, check(true, Capability::No));
        assert!(!read_only.usable(), "known unable to create is not usable");
        assert!(
            VerifyReport::new(info("vultr"), Source::File, check(true, Capability::Yes)).usable()
        );
    }

    #[test]
    fn every_sdk_error_gets_a_kind_and_a_next_step() {
        use nimbus_cloud::Error as E;
        let hetzner = info("hetzner");
        let cases = [
            (
                E::Auth {
                    provider: "hetzner",
                },
                "authentication_rejected",
                "nimbus login --provider hetzner",
            ),
            (
                E::NotFound {
                    provider: "hetzner",
                    resource: "instance",
                    id: "42".into(),
                },
                "not_found",
                "instance list",
            ),
            (
                E::Api {
                    provider: "hetzner",
                    status: 403,
                    message: "no".into(),
                },
                "provider_error",
                "verify",
            ),
            (
                E::Api {
                    provider: "hetzner",
                    status: 422,
                    message: "bad type".into(),
                },
                "provider_error",
                "sizes <region>",
            ),
            (
                E::Api {
                    provider: "hetzner",
                    status: 429,
                    message: "slow".into(),
                },
                "provider_error",
                "Wait",
            ),
            (
                E::Api {
                    provider: "hetzner",
                    status: 503,
                    message: "down".into(),
                },
                "provider_error",
                "Wait",
            ),
            (
                E::InvalidRequest("hetzner: nope".into()),
                "unsupported_request",
                "nimbus providers",
            ),
        ];
        for (error, kind, hint) in cases {
            let err = api_error(hetzner, Source::File, error);
            assert_eq!(err.kind, kind);
            assert_eq!(err.provider.as_deref(), Some("hetzner"));
            assert!(err.hint.contains(hint), "{kind}: {}", err.hint);
            assert!(!err.message.is_empty());
        }
        let from_env = api_error(
            hetzner,
            Source::Environment,
            E::Auth {
                provider: "hetzner",
            },
        );
        assert!(from_env.hint.contains("from the environment"));
    }
}
