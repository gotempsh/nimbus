//! Where provider credentials come from, and how they are kept.
//!
//! A command that needs credentials looks in three places, in this order:
//!
//! 1. the provider's environment variables (the original behaviour);
//! 2. the operating system's credential store (macOS Keychain, Linux Secret
//!    Service), written by `nimbus login`;
//! 3. an owner-only file in the config directory, for machines that have no
//!    credential store.
//!
//! Nothing in this module prints a credential value, and the types that
//! carry one cannot be formatted by accident: [`Secret`] has a redacting
//! `Debug` and no `Display`.

use nimbus_cloud::ProviderInfo;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A credential value. Deliberately awkward to print: `Debug` is redacted and
/// there is no `Display`, so the only way to the text is [`Secret::expose`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// What a store holds for one provider.
///
/// Tagged by `kind` so the stored form does not assume a credential is one
/// static string forever. Today there is one kind; a provider that offers
/// browser approval would add another (an access token with a refresh token
/// and an expiry) next to it without disturbing what is already stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredCredential {
    /// Long-lived values a person supplied, keyed by the field names in the
    /// provider catalog (`token`, or OVH's four values, ...).
    Static { fields: BTreeMap<String, Secret> },
    /// A kind this version does not know — written by a newer nimbus.
    #[serde(other)]
    Unsupported,
}

/// Where a resolved credential came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Environment,
    CredentialStore,
    File,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::Environment => "environment variables",
            Source::CredentialStore => "the operating system's credential store",
            Source::File => "the credentials file",
        }
    }
}

/// Why a store could not do what was asked. Carries a description only —
/// never the stored value, even when the underlying error has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A place credentials are kept between runs. Small on purpose: the real
/// implementations talk to the OS keychain and the filesystem, and tests use
/// an in-memory one.
pub trait CredentialStore {
    /// How a credential read from here is reported.
    fn source(&self) -> Source;
    /// Where this store keeps things, for telling a person — a keychain
    /// name or a file path.
    fn location(&self) -> String;
    fn get(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError>;
    fn set(&self, provider: &str, credential: &StoredCredential) -> Result<(), StoreError>;
    /// Returns whether there was something to remove.
    fn delete(&self, provider: &str) -> Result<bool, StoreError>;
}

/// Everything a command may draw credentials from.
pub struct Sources<'a> {
    /// Looks up an environment variable; unset and empty are both `None`.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Persistent stores in priority order (OS credential store, then file).
    pub stores: Vec<&'a dyn CredentialStore>,
}

/// Credentials ready to hand to an adapter.
pub struct Resolved {
    /// Field name → value. `Secret` even for the non-secret fields: no
    /// command has a reason to print any of them.
    pub values: BTreeMap<&'static str, Secret>,
    pub source: Source,
}

impl Resolved {
    /// The value of a field. Only called for fields `resolve` has filled in.
    pub fn get(&self, field: &str) -> &str {
        self.values
            .get(field)
            .map(Secret::expose)
            .unwrap_or_default()
    }

    /// The values that must never reach output.
    pub fn secrets(&self, info: &ProviderInfo) -> Vec<String> {
        info.credential_fields
            .iter()
            .filter(|f| f.secret)
            .filter_map(|f| self.values.get(f.name))
            .map(|s| s.expose().to_owned())
            .collect()
    }
}

/// Why no usable credential was found.
#[derive(Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// Nothing is configured anywhere. `notes` explains any store that
    /// could not be consulted, so a locked keychain is not mistaken for an
    /// empty one.
    Missing { notes: Vec<String> },
    /// A store holds a credential kind this version cannot use.
    Unsupported { source: Source },
    /// A stored credential lacks fields the provider needs.
    Incomplete {
        source: Source,
        missing: Vec<&'static str>,
    },
}

/// Finds credentials for a provider: environment first, then each store.
///
/// The environment is used only when it has every required field — a lone
/// `SCW_DEFAULT_ZONE` is not a credential. With a stored credential, optional
/// fields (a zone) may still be overridden from the environment.
pub fn resolve(info: &ProviderInfo, sources: &Sources) -> Result<Resolved, ResolveError> {
    let env = sources.env;
    let required = || info.credential_fields.iter().filter(|f| f.required);
    let optional = || info.credential_fields.iter().filter(|f| !f.required);

    if required().all(|f| env(f.env).is_some()) {
        let mut values = BTreeMap::new();
        for field in info.credential_fields {
            if let Some(value) = env(field.env).or_else(|| field.default.map(str::to_owned)) {
                values.insert(field.name, Secret::new(value));
            }
        }
        return Ok(Resolved {
            values,
            source: Source::Environment,
        });
    }

    let mut notes = Vec::new();
    for store in &sources.stores {
        let stored = match store.get(info.id) {
            Ok(Some(stored)) => stored,
            Ok(None) => continue,
            Err(e) => {
                notes.push(format!("{} could not be read: {e}", store.location()));
                continue;
            }
        };
        let source = store.source();
        let StoredCredential::Static { fields } = stored else {
            return Err(ResolveError::Unsupported { source });
        };
        let missing: Vec<&'static str> = required()
            .filter(|f| fields.get(f.name).is_none_or(|v| v.expose().is_empty()))
            .map(|f| f.env)
            .collect();
        if !missing.is_empty() {
            return Err(ResolveError::Incomplete { source, missing });
        }
        let mut values = BTreeMap::new();
        for field in required() {
            if let Some(value) = fields.get(field.name) {
                values.insert(field.name, value.clone());
            }
        }
        for field in optional() {
            let value = env(field.env)
                .map(Secret::new)
                .or_else(|| fields.get(field.name).cloned())
                .or_else(|| field.default.map(Secret::new));
            if let Some(value) = value {
                values.insert(field.name, value);
            }
        }
        return Ok(Resolved { values, source });
    }

    Err(ResolveError::Missing { notes })
}

/// The exact command that stores credentials for a provider.
pub fn login_command(info: &ProviderInfo) -> String {
    format!("nimbus login --provider {}", info.id)
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use std::cell::RefCell;

    /// In-memory [`CredentialStore`] — what tests use instead of the real
    /// keychain. `failing` makes every call fail, like a machine with no
    /// credential store.
    pub struct MemoryStore {
        pub source: Source,
        pub entries: RefCell<BTreeMap<String, StoredCredential>>,
        pub failing: Option<&'static str>,
    }

    impl MemoryStore {
        pub fn new(source: Source) -> Self {
            Self {
                source,
                entries: RefCell::new(BTreeMap::new()),
                failing: None,
            }
        }

        pub fn failing(source: Source, reason: &'static str) -> Self {
            Self {
                failing: Some(reason),
                ..Self::new(source)
            }
        }

        pub fn with(self, provider: &str, fields: &[(&str, &str)]) -> Self {
            self.entries
                .borrow_mut()
                .insert(provider.to_owned(), static_credential(fields));
            self
        }

        pub fn has(&self, provider: &str) -> bool {
            self.entries.borrow().contains_key(provider)
        }

        fn check(&self) -> Result<(), StoreError> {
            match self.failing {
                Some(reason) => Err(StoreError(reason.to_owned())),
                None => Ok(()),
            }
        }
    }

    impl CredentialStore for MemoryStore {
        fn source(&self) -> Source {
            self.source
        }
        fn location(&self) -> String {
            format!("memory store ({:?})", self.source)
        }
        fn get(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError> {
            self.check()?;
            Ok(self.entries.borrow().get(provider).cloned())
        }
        fn set(&self, provider: &str, credential: &StoredCredential) -> Result<(), StoreError> {
            self.check()?;
            self.entries
                .borrow_mut()
                .insert(provider.to_owned(), credential.clone());
            Ok(())
        }
        fn delete(&self, provider: &str) -> Result<bool, StoreError> {
            self.check()?;
            Ok(self.entries.borrow_mut().remove(provider).is_some())
        }
    }

    pub fn static_credential(fields: &[(&str, &str)]) -> StoredCredential {
        StoredCredential::Static {
            fields: fields
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Secret::new(*v)))
                .collect(),
        }
    }

    /// An environment lookup over a fixed list, with the same "empty means
    /// unset" rule as the real one.
    pub fn env_of(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| vars.get(key).filter(|v| !v.is_empty()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use nimbus_cloud::provider_info;

    fn info(id: &str) -> &'static ProviderInfo {
        provider_info(id).expect("known provider")
    }

    fn resolve_with(
        id: &str,
        env: &[(&str, &str)],
        stores: &[&MemoryStore],
    ) -> Result<Resolved, ResolveError> {
        let env = env_of(env);
        let sources = Sources {
            env: &env,
            stores: stores.iter().map(|s| *s as &dyn CredentialStore).collect(),
        };
        resolve(info(id), &sources)
    }

    #[test]
    fn environment_wins_over_both_stores() {
        let os = MemoryStore::new(Source::CredentialStore).with("hetzner", &[("token", "from-os")]);
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "from-file")]);
        let r = resolve_with("hetzner", &[("HCLOUD_TOKEN", "from-env")], &[&os, &file])
            .expect("resolved");
        assert_eq!(r.source, Source::Environment);
        assert_eq!(r.get("token"), "from-env");
    }

    #[test]
    fn credential_store_wins_over_file() {
        let os = MemoryStore::new(Source::CredentialStore).with("hetzner", &[("token", "from-os")]);
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "from-file")]);
        let r = resolve_with("hetzner", &[], &[&os, &file]).expect("resolved");
        assert_eq!(r.source, Source::CredentialStore);
        assert_eq!(r.get("token"), "from-os");
    }

    #[test]
    fn file_is_used_last() {
        let os = MemoryStore::new(Source::CredentialStore);
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "from-file")]);
        let r = resolve_with("hetzner", &[], &[&os, &file]).expect("resolved");
        assert_eq!(r.source, Source::File);
        assert_eq!(r.get("token"), "from-file");
    }

    #[test]
    fn an_unreadable_credential_store_falls_through_to_the_file() {
        let os = MemoryStore::failing(Source::CredentialStore, "no secret service");
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "from-file")]);
        let r = resolve_with("hetzner", &[], &[&os, &file]).expect("resolved");
        assert_eq!(r.source, Source::File);
    }

    #[test]
    fn an_empty_environment_variable_counts_as_unset() {
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "from-file")]);
        let r = resolve_with("hetzner", &[("HCLOUD_TOKEN", "")], &[&file]).expect("resolved");
        assert_eq!(r.source, Source::File);
    }

    #[test]
    fn a_partial_environment_is_not_a_credential() {
        // Three of OVH's four values: not enough, so the stored one is used
        // whole rather than mixed with them.
        let os = MemoryStore::new(Source::CredentialStore).with(
            "ovh",
            &[
                ("application_key", "ak-stored"),
                ("application_secret", "as-stored"),
                ("consumer_key", "ck-stored"),
                ("project_id", "project-stored"),
            ],
        );
        let env = [
            ("OVH_APPLICATION_KEY", "ak-env"),
            ("OVH_APPLICATION_SECRET", "as-env"),
            ("OVH_CONSUMER_KEY", "ck-env"),
        ];
        let r = resolve_with("ovh", &env, &[&os]).expect("resolved");
        assert_eq!(r.source, Source::CredentialStore);
        assert_eq!(r.get("application_secret"), "as-stored");
        assert_eq!(r.get("project_id"), "project-stored");

        assert_eq!(
            resolve_with("ovh", &env, &[]).err(),
            Some(ResolveError::Missing { notes: vec![] })
        );
    }

    #[test]
    fn optional_fields_default_and_can_be_overridden_from_the_environment() {
        let env = [("SCW_SECRET_KEY", "sk"), ("SCW_DEFAULT_PROJECT_ID", "proj")];
        let r = resolve_with("scaleway", &env, &[]).expect("resolved");
        assert_eq!(r.get("zone"), "fr-par-1", "default zone");

        let stored = MemoryStore::new(Source::File).with(
            "scaleway",
            &[
                ("secret_key", "sk"),
                ("project_id", "proj"),
                ("zone", "nl-ams-1"),
            ],
        );
        let r = resolve_with("scaleway", &[], &[&stored]).expect("resolved");
        assert_eq!(r.get("zone"), "nl-ams-1", "stored zone");

        let r = resolve_with("scaleway", &[("SCW_DEFAULT_ZONE", "pl-waw-2")], &[&stored])
            .expect("resolved");
        assert_eq!(r.source, Source::File);
        assert_eq!(
            r.get("zone"),
            "pl-waw-2",
            "environment overrides the stored zone"
        );
        assert_eq!(r.get("secret_key"), "sk");
    }

    #[test]
    fn nothing_configured_is_missing_and_says_which_stores_failed() {
        let os = MemoryStore::failing(Source::CredentialStore, "keychain is locked");
        let file = MemoryStore::new(Source::File);
        match resolve_with("vultr", &[], &[&os, &file]).err() {
            Some(ResolveError::Missing { notes }) => {
                assert_eq!(notes.len(), 1);
                assert!(notes[0].contains("keychain is locked"), "{notes:?}");
            }
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn a_stored_credential_missing_a_required_field_is_incomplete() {
        let file = MemoryStore::new(Source::File).with("scaleway", &[("secret_key", "sk")]);
        assert_eq!(
            resolve_with("scaleway", &[], &[&file]).err(),
            Some(ResolveError::Incomplete {
                source: Source::File,
                missing: vec!["SCW_DEFAULT_PROJECT_ID"],
            })
        );
    }

    #[test]
    fn a_credential_kind_from_a_newer_version_is_reported_not_misread() {
        let file = MemoryStore::new(Source::File);
        file.entries
            .borrow_mut()
            .insert("hetzner".into(), StoredCredential::Unsupported);
        assert_eq!(
            resolve_with("hetzner", &[], &[&file]).err(),
            Some(ResolveError::Unsupported {
                source: Source::File
            })
        );
    }

    #[test]
    fn only_secret_fields_are_listed_for_redaction() {
        let env = [
            ("OVH_APPLICATION_KEY", "ak-value"),
            ("OVH_APPLICATION_SECRET", "as-value"),
            ("OVH_CONSUMER_KEY", "ck-value"),
            ("OVH_PROJECT_ID", "project-value"),
        ];
        let r = resolve_with("ovh", &env, &[]).expect("resolved");
        assert_eq!(r.secrets(info("ovh")), vec!["as-value", "ck-value"]);
    }

    #[test]
    fn stored_form_is_tagged_so_other_kinds_can_be_added() {
        let stored = static_credential(&[("token", "fake-token-value")]);
        let json = serde_json::to_value(&stored).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({ "kind": "static", "fields": { "token": "fake-token-value" } })
        );
        assert_eq!(
            serde_json::from_value::<StoredCredential>(json).expect("round trip"),
            stored
        );

        // What a later version might write for a browser-approved login.
        let oauth = serde_json::json!({
            "kind": "oauth",
            "access_token": "fake-access",
            "refresh_token": "fake-refresh",
            "expires_at": "2030-01-01T00:00:00Z",
        });
        assert_eq!(
            serde_json::from_value::<StoredCredential>(oauth).expect("parses"),
            StoredCredential::Unsupported
        );
    }

    #[test]
    fn debug_output_never_shows_a_value() {
        let stored = static_credential(&[("token", "fake-token-value")]);
        let debug = format!("{stored:?} {:?}", Secret::new("fake-token-value"));
        assert!(!debug.contains("fake-token-value"), "{debug}");
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn login_command_is_exact() {
        assert_eq!(
            login_command(info("do")),
            "nimbus login --provider digitalocean"
        );
    }
}
