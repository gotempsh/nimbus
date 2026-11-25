//! Static facts about each supported provider: the credential fields it
//! needs and the caveats a caller has to know before offering it to someone.
//!
//! This is data, not documentation, so that a program (or an agent driving
//! the CLI) can act on it — e.g. refuse to "try out" a provider whose create
//! is a purchase that cannot be undone. `lib/tests/catalog.rs` checks every
//! caveat here against what the adapter actually does.

use serde::Serialize;

/// One value a provider needs in order to authenticate or address an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CredentialField {
    /// Stable key for this field, e.g. `"token"`.
    pub name: &'static str,
    /// Environment variable the CLI reads it from, e.g. `"HCLOUD_TOKEN"`.
    pub env: &'static str,
    /// Human-readable name, suitable for a prompt.
    pub label: &'static str,
    /// Whether the value must be kept secret. Non-secret fields (a project
    /// id, a zone) identify what to act on rather than prove who is acting.
    pub secret: bool,
    /// Optional fields fall back to `default`.
    pub required: bool,
    pub default: Option<&'static str>,
}

/// What a caller must know before offering a provider to a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Caveats {
    /// `delete_instance` works. When `false` an instance can only be removed
    /// outside the API (the provider's dashboard).
    pub can_delete_instances: bool,
    /// `instance_types` carries real prices. When `false`, `monthly_price`
    /// is `0.0` and must not be shown as a cost.
    pub reports_prices: bool,
    /// `instance_types` carries real `vcpus` / `memory_gb` / `disk_gb`. When
    /// `false` they are `0`.
    pub reports_instance_specs: bool,
    /// `create_instance` buys something that cannot be cancelled or refunded
    /// through the API — never call it speculatively or as a test.
    pub create_is_irreversible_purchase: bool,
    /// `CreateInstance.user_data` is delivered to the instance. When `false`
    /// `create_instance` refuses a request that sets it.
    pub supports_user_data: bool,
    /// Anything else worth knowing that the flags above don't capture.
    pub notes: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderInfo {
    /// Same value as [`CloudProvider::id`](crate::CloudProvider::id).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Other ids accepted for this provider.
    pub aliases: &'static [&'static str],
    pub credential_fields: &'static [CredentialField],
    pub caveats: Caveats,
}

const fn secret(name: &'static str, env: &'static str, label: &'static str) -> CredentialField {
    CredentialField {
        name,
        env,
        label,
        secret: true,
        required: true,
        default: None,
    }
}

const fn plain(name: &'static str, env: &'static str, label: &'static str) -> CredentialField {
    CredentialField {
        name,
        env,
        label,
        secret: false,
        required: true,
        default: None,
    }
}

/// The common case: a metered provider with a full API.
const NO_CAVEATS: Caveats = Caveats {
    can_delete_instances: true,
    reports_prices: true,
    reports_instance_specs: true,
    create_is_irreversible_purchase: false,
    supports_user_data: true,
    notes: &[],
};

/// Every provider this crate ships an adapter for.
pub const PROVIDERS: &[ProviderInfo] = &[
    ProviderInfo {
        id: "hetzner",
        name: "Hetzner Cloud",
        aliases: &[],
        credential_fields: &[secret("token", "HCLOUD_TOKEN", "API token")],
        caveats: NO_CAVEATS,
    },
    ProviderInfo {
        id: "vultr",
        name: "Vultr",
        aliases: &[],
        credential_fields: &[secret("api_key", "VULTR_API_KEY", "API key")],
        caveats: NO_CAVEATS,
    },
    ProviderInfo {
        id: "ovh",
        name: "OVHcloud",
        aliases: &[],
        credential_fields: &[
            plain("application_key", "OVH_APPLICATION_KEY", "Application key"),
            secret(
                "application_secret",
                "OVH_APPLICATION_SECRET",
                "Application secret",
            ),
            secret("consumer_key", "OVH_CONSUMER_KEY", "Consumer key"),
            plain("project_id", "OVH_PROJECT_ID", "Public Cloud project id"),
        ],
        caveats: Caveats {
            reports_prices: false,
            notes: &[
                "monthly_price is 0.0 for every instance type: the adapter does not read OVHcloud's price catalog, so take prices from OVHcloud's own price list before quoting a cost.",
                "Instances are reached as the image's default user (`ubuntu` on Ubuntu images), not root.",
            ],
            ..NO_CAVEATS
        },
    },
    ProviderInfo {
        id: "digitalocean",
        name: "DigitalOcean",
        aliases: &["do"],
        credential_fields: &[secret(
            "token",
            "DIGITALOCEAN_TOKEN",
            "Personal access token",
        )],
        caveats: NO_CAVEATS,
    },
    ProviderInfo {
        id: "scaleway",
        name: "Scaleway",
        aliases: &[],
        credential_fields: &[
            secret("secret_key", "SCW_SECRET_KEY", "API secret key"),
            plain("project_id", "SCW_DEFAULT_PROJECT_ID", "Project id"),
            CredentialField {
                name: "zone",
                env: "SCW_DEFAULT_ZONE",
                label: "Zone",
                secret: false,
                required: false,
                default: Some("fr-par-1"),
            },
        ],
        caveats: Caveats {
            notes: &[
                "A client is bound to one zone: instance types, images and instances are those of that zone only.",
            ],
            ..NO_CAVEATS
        },
    },
    ProviderInfo {
        id: "linode",
        name: "Linode (Akamai)",
        aliases: &[],
        credential_fields: &[secret("token", "LINODE_TOKEN", "Personal access token")],
        caveats: Caveats {
            notes: &[
                "A private network (VPC) cannot be attached at create time; create_instance refuses a request that asks for one.",
            ],
            ..NO_CAVEATS
        },
    },
    ProviderInfo {
        id: "hostinger",
        name: "Hostinger VPS",
        aliases: &[],
        credential_fields: &[secret("token", "HOSTINGER_API_TOKEN", "API token")],
        caveats: Caveats {
            can_delete_instances: false,
            create_is_irreversible_purchase: true,
            supports_user_data: false,
            notes: &[
                "create_instance purchases a subscription, charged to the account's default payment method. There is no API to delete the server or cancel the subscription; that is done in Hostinger's billing dashboard.",
                "vcpus, memory_gb and disk_gb are not read from Hostinger's API (its catalog does not expose them before purchase) but from a table of the published plan tiers built into nimbus; a plan that table does not know reports 0 for all three.",
                "No block storage and no private networks.",
                "Regions and images are numeric ids (data center id, OS template id).",
            ],
            ..NO_CAVEATS
        },
    },
];

/// Looks a provider up by id or alias.
pub fn provider_info(id: &str) -> Option<&'static ProviderInfo> {
    PROVIDERS
        .iter()
        .find(|p| p.id == id || p.aliases.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_aliases_and_env_vars_are_unique() {
        let mut ids = HashSet::new();
        let mut envs = HashSet::new();
        for p in PROVIDERS {
            assert!(ids.insert(p.id), "duplicate provider id {}", p.id);
            for alias in p.aliases {
                assert!(ids.insert(alias), "alias {alias} collides");
            }
            let mut names = HashSet::new();
            for f in p.credential_fields {
                assert!(envs.insert(f.env), "env var {} used twice", f.env);
                assert!(names.insert(f.name), "{}: field {} twice", p.id, f.name);
            }
        }
    }

    #[test]
    fn every_provider_needs_a_secret_and_optional_fields_have_defaults() {
        for p in PROVIDERS {
            assert!(
                p.credential_fields.iter().any(|f| f.secret && f.required),
                "{}: no required secret field",
                p.id
            );
            for f in p.credential_fields {
                assert_eq!(
                    f.required,
                    f.default.is_none(),
                    "{}: field {} must be required or have a default",
                    p.id,
                    f.name
                );
                assert!(!(f.secret && !f.required), "secrets have no defaults");
            }
        }
    }

    #[test]
    fn an_irreversible_purchase_is_one_that_cannot_be_deleted() {
        for p in PROVIDERS {
            if p.caveats.create_is_irreversible_purchase {
                assert!(!p.caveats.can_delete_instances, "{}", p.id);
            }
        }
    }

    #[test]
    fn lookup_accepts_ids_and_aliases() {
        assert_eq!(provider_info("do").map(|p| p.id), Some("digitalocean"));
        assert_eq!(provider_info("hetzner").map(|p| p.id), Some("hetzner"));
        assert!(provider_info("nope").is_none());
    }
}
