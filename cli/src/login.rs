//! `nimbus login` and `nimbus logout`: getting a credential from a person
//! into a store, and out again, without it ever being a command-line
//! argument (visible in the process list and shell history) or appearing in
//! output.
//!
//! A credential arrives one of three ways: typed at a prompt with echo off,
//! read from a file, or read from standard input. There is deliberately no
//! flag that takes the value itself.

use crate::credentials::{
    login_command, CredentialStore, Secret, Source, StoreError, StoredCredential,
};
use crate::output::CliError;
use nimbus_cloud::{CredentialField, ProviderInfo};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io;

/// Which persistent store(s) to use. `NIMBUS_CREDENTIAL_STORE` sets it for
/// every command; `login --store` overrides it for that login.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum StoreMode {
    /// The OS credential store, falling back to the file if there is none.
    Auto,
    /// The OS credential store only.
    Os,
    /// The owner-only credentials file only; the OS store is never touched.
    File,
}

impl StoreMode {
    pub fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "auto" => Ok(Self::Auto),
            "os" => Ok(Self::Os),
            "file" => Ok(Self::File),
            other => Err(CliError::new(
                "invalid_configuration",
                format!("NIMBUS_CREDENTIAL_STORE is `{other}`"),
                "Set NIMBUS_CREDENTIAL_STORE to `auto`, `os` or `file`, or unset it.",
            )),
        }
    }
}

/// Asks a person for values. The terminal implementation turns echo off for
/// secrets; tests script the answers and record which kind of prompt each
/// field got.
pub trait Prompter {
    /// Reads a value without showing it as it is typed.
    fn secret(&mut self, label: &str) -> io::Result<String>;
    /// Reads a value that is fine to show (a project id, a zone).
    fn visible(&mut self, label: &str, default: Option<&str>) -> io::Result<String>;
}

/// How many times an empty answer to a required field is re-asked.
const ATTEMPTS: usize = 3;

fn prompt_label(info: &ProviderInfo, field: &CredentialField) -> String {
    format!("{} {} ({})", info.name, field.label, field.env)
}

/// Prompts for each of a provider's fields in turn.
pub fn collect_interactive(
    info: &ProviderInfo,
    prompter: &mut dyn Prompter,
) -> Result<BTreeMap<String, Secret>, CliError> {
    let mut fields = BTreeMap::new();
    for field in info.credential_fields {
        let label = prompt_label(info, field);
        let mut value = String::new();
        for _ in 0..ATTEMPTS {
            let answer = if field.secret {
                prompter.secret(&label)
            } else {
                prompter.visible(&label, field.default)
            }
            .map_err(|e| {
                CliError::new(
                    "prompt_failed",
                    format!("could not read {} from the terminal: {e}", field.env),
                    format!(
                        "Run `{}` in an interactive terminal, or supply the credential with --from-file <path> or --stdin.",
                        login_command(info)
                    ),
                )
                .for_provider(info.id)
            })?;
            value = answer.trim().to_owned();
            if !value.is_empty() || !field.required {
                break;
            }
        }
        if value.is_empty() {
            if field.required {
                return Err(CliError::new(
                    "invalid_input",
                    format!("no value was entered for {}", field.env),
                    format!("Run `{}` again and enter it.", login_command(info)),
                )
                .for_provider(info.id));
            }
            // An optional field left blank is not stored: its default (or an
            // environment override) applies when the credential is used.
            continue;
        }
        fields.insert(field.name.to_owned(), Secret::new(value));
    }
    Ok(fields)
}

/// Describes the accepted file / stdin format for a provider.
fn input_format_hint(info: &ProviderInfo) -> String {
    let lines: Vec<String> = info
        .credential_fields
        .iter()
        .map(|f| {
            if f.required {
                format!("{}=<value>", f.env)
            } else {
                format!("{}=<value> (optional)", f.env)
            }
        })
        .collect();
    let single = single_required_field(info)
        .map(|_| " — or just the value on its own, with nothing else".to_owned())
        .unwrap_or_default();
    format!(
        "Provide one `NAME=value` line per field: {}{single}.",
        lines.join(", ")
    )
}

/// The only required field, if the provider has exactly one — in which case
/// a bare value is unambiguous.
fn single_required_field(info: &ProviderInfo) -> Option<&'static CredentialField> {
    let mut required = info.credential_fields.iter().filter(|f| f.required);
    match (required.next(), required.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// Parses a credential supplied through a file or standard input.
///
/// Either `NAME=value` lines, using the provider's environment variable
/// names (blank lines and `#` comments allowed), or — for a provider with a
/// single required field — the bare value. The names are matched literally,
/// so a token that happens to contain `=` is still read as a bare value.
pub fn parse_input(info: &ProviderInfo, input: &str) -> Result<BTreeMap<String, Secret>, CliError> {
    let invalid = |message: String| {
        CliError::new("invalid_input", message, input_format_hint(info)).for_provider(info.id)
    };
    let lines: Vec<&str> = input
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return Err(invalid("the credential input is empty".to_owned()));
    }

    let assignment = |line: &str| -> Option<(&'static CredentialField, String)> {
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        info.credential_fields.iter().find_map(|field| {
            let value = line
                .strip_prefix(field.env)?
                .trim_start()
                .strip_prefix('=')?;
            Some((field, unquote(value.trim()).to_owned()))
        })
    };

    let mut fields = BTreeMap::new();
    if lines.iter().all(|l| assignment(l).is_some()) {
        for (field, value) in lines.iter().filter_map(|l| assignment(l)) {
            if !value.is_empty() {
                fields.insert(field.name.to_owned(), Secret::new(value));
            }
        }
    } else if let (Some(field), [only]) = (single_required_field(info), lines.as_slice()) {
        fields.insert(field.name.to_owned(), Secret::new(*only));
    } else {
        // Deliberately does not quote the offending line: it may be a secret.
        return Err(invalid(
            "the credential input is not in the expected format".to_owned(),
        ));
    }

    let missing: Vec<&str> = info
        .credential_fields
        .iter()
        .filter(|f| f.required && !fields.contains_key(f.name))
        .map(|f| f.env)
        .collect();
    if !missing.is_empty() {
        return Err(invalid(format!(
            "the credential input has no value for {}",
            missing.join(", ")
        )));
    }
    Ok(fields)
}

/// Strips one pair of matching quotes, as a shell or dotenv file would.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// What `login` prints. No field of it holds a credential value.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct LoginReport {
    pub provider: &'static str,
    pub stored: bool,
    /// `credential_store` or `file`.
    pub store: Source,
    /// The keychain or file path the credential now lives in.
    pub location: String,
    /// Environment-variable names of the fields that were stored.
    pub fields: Vec<&'static str>,
    /// Why the OS credential store was not used, when the file was the
    /// fallback rather than the choice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The command to run next.
    pub next: String,
}

/// The stores a login or logout may touch, as selected by [`StoreMode`].
/// Either may be absent: `--store file` never constructs the OS store.
pub struct Stores<'a> {
    pub os: Option<&'a dyn CredentialStore>,
    pub file: Option<&'a dyn CredentialStore>,
}

impl<'a> Stores<'a> {
    /// In the order credentials are read from them.
    pub fn in_read_order(&self) -> Vec<&'a dyn CredentialStore> {
        self.os.into_iter().chain(self.file).collect()
    }
}

/// Saves a credential: in the OS store when there is one, otherwise in the
/// file — and says which. A copy left in the other store is removed so the
/// two cannot disagree.
pub fn store_credential(
    info: &'static ProviderInfo,
    fields: BTreeMap<String, Secret>,
    stores: &Stores,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<LoginReport, CliError> {
    let stored_fields: Vec<&'static str> = info
        .credential_fields
        .iter()
        .filter(|f| fields.contains_key(f.name))
        .map(|f| f.env)
        .collect();
    let credential = StoredCredential::Static { fields };
    let store_failed = |e: StoreError, hint: &str| {
        CliError::new("credential_store_failed", e.to_string(), hint).for_provider(info.id)
    };

    let mut fallback_reason = None;
    let mut used: Option<&dyn CredentialStore> = None;
    if let Some(os) = stores.os {
        match os.set(info.id, &credential) {
            Ok(()) => used = Some(os),
            Err(e) if stores.file.is_some() => fallback_reason = Some(e.to_string()),
            Err(e) => {
                return Err(store_failed(
                    e,
                    "Unlock or start the system's credential store, or run the login with `--store file` to keep the credential in an owner-only file instead.",
                ))
            }
        }
    }
    let used = match (used, stores.file) {
        (Some(used), _) => used,
        (None, Some(file)) => {
            file.set(info.id, &credential).map_err(|e| {
                store_failed(
                    e,
                    "Check that the config directory is writable, or point NIMBUS_CONFIG_DIR at one that is.",
                )
            })?;
            file
        }
        (None, None) => {
            return Err(CliError::new(
                "credential_store_failed",
                "no credential store is enabled",
                "Unset NIMBUS_CREDENTIAL_STORE or set it to `auto`.",
            )
            .for_provider(info.id))
        }
    };

    let mut warnings = Vec::new();
    for other in stores.in_read_order() {
        // An OS store that just refused the write is not asked again: the
        // fallback reason already says it is unusable.
        if other.source() == used.source() || fallback_reason.is_some() {
            continue;
        }
        // Best effort: a stale copy is untidy, not a reason to fail a login
        // that succeeded.
        if let Err(e) = other.delete(info.id) {
            warnings.push(format!(
                "an older credential for {} may remain in {}: {e}",
                info.id,
                other.location()
            ));
        }
    }
    let required: Vec<&str> = info
        .credential_fields
        .iter()
        .filter(|f| f.required)
        .map(|f| f.env)
        .collect();
    if required.iter().all(|name| env(name).is_some()) {
        warnings.push(format!(
            "{} set in the environment, which takes precedence over the stored credential; unset to use what was just stored",
            required.join(", ")
        ));
    }

    Ok(LoginReport {
        provider: info.id,
        stored: true,
        store: used.source(),
        location: used.location(),
        fields: stored_fields,
        fallback_reason,
        warnings,
        next: format!("nimbus --provider {} verify", info.id),
    })
}

/// What `logout` prints.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct LogoutReport {
    pub provider: &'static str,
    /// The stores a credential was removed from; empty if none was stored.
    pub removed: Vec<Source>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Removes a provider's credential from every enabled store. Having nothing
/// to remove is not an error — the outcome asked for already holds.
pub fn remove_credential(
    info: &'static ProviderInfo,
    stores: &Stores,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<LogoutReport, CliError> {
    let mut removed = Vec::new();
    let mut failures = Vec::new();
    for store in stores.in_read_order() {
        match store.delete(info.id) {
            Ok(true) => removed.push(store.source()),
            Ok(false) => {}
            Err(e) => failures.push((store, e)),
        }
    }

    let mut warnings = Vec::new();
    for (store, e) in failures {
        // A file that will not open still holds whatever is in it: that has
        // to stop the logout from looking complete. An OS store that is
        // simply absent holds nothing.
        if store.source() == Source::File {
            return Err(CliError::new(
                "credential_store_failed",
                e.to_string(),
                "Fix or delete the credentials file, then run the logout again.",
            )
            .for_provider(info.id));
        }
        warnings.push(format!(
            "{} could not be checked, so a credential may remain there: {e}",
            store.location()
        ));
    }
    let set: Vec<&str> = info
        .credential_fields
        .iter()
        .filter(|f| env(f.env).is_some())
        .map(|f| f.env)
        .collect();
    if !set.is_empty() {
        warnings.push(format!(
            "{} still set in the environment; logout does not change the environment",
            set.join(", ")
        ));
    }
    Ok(LogoutReport {
        provider: info.id,
        removed,
        warnings,
    })
}

/// Prompts on the controlling terminal. Secrets are read with echo off.
pub struct TerminalPrompter;

impl Prompter for TerminalPrompter {
    fn secret(&mut self, label: &str) -> io::Result<String> {
        rpassword::prompt_password(format!("{label} (input hidden): "))
    }

    fn visible(&mut self, label: &str, default: Option<&str>) -> io::Result<String> {
        use std::io::Write;
        let mut err = io::stderr();
        match default {
            Some(default) => write!(err, "{label} [{default}]: ")?,
            None => write!(err, "{label}: ")?,
        }
        err.flush()?;
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        Ok(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::testing::{env_of, MemoryStore};
    use nimbus_cloud::provider_info;

    fn info(id: &str) -> &'static ProviderInfo {
        provider_info(id).expect("known provider")
    }

    fn values(fields: &BTreeMap<String, Secret>) -> Vec<(&str, &str)> {
        fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.expose()))
            .collect()
    }

    /// Answers prompts from a script and records how each was asked.
    struct Scripted {
        answers: Vec<&'static str>,
        asked: Vec<(String, bool)>,
    }

    impl Scripted {
        fn new(answers: &[&'static str]) -> Self {
            Self {
                answers: answers.iter().rev().copied().collect(),
                asked: Vec::new(),
            }
        }
        fn next(&mut self, label: &str, hidden: bool) -> io::Result<String> {
            self.asked.push((label.to_owned(), hidden));
            self.answers
                .pop()
                .map(str::to_owned)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no more answers"))
        }
    }

    impl Prompter for Scripted {
        fn secret(&mut self, label: &str) -> io::Result<String> {
            self.next(label, true)
        }
        fn visible(&mut self, label: &str, _default: Option<&str>) -> io::Result<String> {
            self.next(label, false)
        }
    }

    // ---------- interactive ----------

    #[test]
    fn a_single_token_is_prompted_hidden() {
        let mut prompter = Scripted::new(&["  fake-hetzner-token\n"]);
        let fields = collect_interactive(info("hetzner"), &mut prompter).expect("collected");
        assert_eq!(values(&fields), vec![("token", "fake-hetzner-token")]);
        assert_eq!(
            prompter.asked,
            vec![("Hetzner Cloud API token (HCLOUD_TOKEN)".to_owned(), true)]
        );
    }

    #[test]
    fn ovh_prompts_for_all_four_values_hiding_only_the_secret_ones() {
        let mut prompter = Scripted::new(&["fake-ak", "fake-as", "fake-ck", "fake-project"]);
        let fields = collect_interactive(info("ovh"), &mut prompter).expect("collected");
        assert_eq!(
            values(&fields),
            vec![
                ("application_key", "fake-ak"),
                ("application_secret", "fake-as"),
                ("consumer_key", "fake-ck"),
                ("project_id", "fake-project"),
            ]
        );
        let hidden: Vec<bool> = prompter.asked.iter().map(|(_, hidden)| *hidden).collect();
        assert_eq!(hidden, vec![false, true, true, false]);
        assert!(prompter.asked[1].0.contains("OVH_APPLICATION_SECRET"));
    }

    #[test]
    fn scaleway_zone_may_be_left_blank_and_is_then_not_stored() {
        let mut prompter = Scripted::new(&["fake-secret-key", "fake-project", "\n"]);
        let fields = collect_interactive(info("scaleway"), &mut prompter).expect("collected");
        assert_eq!(
            values(&fields),
            vec![
                ("project_id", "fake-project"),
                ("secret_key", "fake-secret-key")
            ]
        );
        let hidden: Vec<bool> = prompter.asked.iter().map(|(_, hidden)| *hidden).collect();
        assert_eq!(hidden, vec![true, false, false]);

        let mut prompter = Scripted::new(&["fake-secret-key", "fake-project", "nl-ams-1"]);
        let fields = collect_interactive(info("scaleway"), &mut prompter).expect("collected");
        assert_eq!(fields.get("zone").map(Secret::expose), Some("nl-ams-1"));
    }

    #[test]
    fn an_empty_required_answer_is_asked_again_then_refused() {
        let mut prompter = Scripted::new(&["", "  ", "fake-token"]);
        let fields = collect_interactive(info("vultr"), &mut prompter).expect("collected");
        assert_eq!(values(&fields), vec![("api_key", "fake-token")]);
        assert_eq!(prompter.asked.len(), 3);

        let mut prompter = Scripted::new(&["", "", ""]);
        let err = collect_interactive(info("vultr"), &mut prompter).expect_err("refused");
        assert_eq!(err.kind, "invalid_input");
        assert!(err.message.contains("VULTR_API_KEY"));
        assert!(err.hint.contains("nimbus login --provider vultr"));
    }

    #[test]
    fn a_prompt_that_cannot_be_read_points_at_the_alternatives() {
        let mut prompter = Scripted::new(&[]);
        let err = collect_interactive(info("linode"), &mut prompter).expect_err("no terminal");
        assert_eq!(err.kind, "prompt_failed");
        assert!(err.hint.contains("--from-file"));
        assert!(err.hint.contains("--stdin"));
    }

    // ---------- file / stdin ----------

    #[test]
    fn a_bare_value_is_accepted_for_a_single_field_provider() {
        let fields = parse_input(info("hetzner"), "fake-hetzner-token\n").expect("parsed");
        assert_eq!(values(&fields), vec![("token", "fake-hetzner-token")]);
        // Windows line endings, a BOM, padding that looks like an assignment.
        let fields = parse_input(info("hetzner"), "\u{feff}fake+token/with==\r\n").expect("parsed");
        assert_eq!(values(&fields), vec![("token", "fake+token/with==")]);
    }

    #[test]
    fn assignments_use_the_environment_variable_names() {
        let input = "# from the provider's console\nexport HCLOUD_TOKEN=\"fake-hetzner-token\"\n\n";
        let fields = parse_input(info("hetzner"), input).expect("parsed");
        assert_eq!(values(&fields), vec![("token", "fake-hetzner-token")]);

        let input = "OVH_APPLICATION_KEY=fake-ak\nOVH_APPLICATION_SECRET = fake-as\nOVH_CONSUMER_KEY='fake-ck'\nOVH_PROJECT_ID=fake-project\n";
        let fields = parse_input(info("ovh"), input).expect("parsed");
        assert_eq!(
            values(&fields),
            vec![
                ("application_key", "fake-ak"),
                ("application_secret", "fake-as"),
                ("consumer_key", "fake-ck"),
                ("project_id", "fake-project"),
            ]
        );
    }

    #[test]
    fn optional_fields_may_be_given_or_omitted() {
        let fields = parse_input(
            info("scaleway"),
            "SCW_SECRET_KEY=fake-sk\nSCW_DEFAULT_PROJECT_ID=fake-project\n",
        )
        .expect("parsed");
        assert!(!fields.contains_key("zone"));
        let fields = parse_input(
            info("scaleway"),
            "SCW_SECRET_KEY=fake-sk\nSCW_DEFAULT_PROJECT_ID=fake-project\nSCW_DEFAULT_ZONE=nl-ams-1\n",
        )
        .expect("parsed");
        assert_eq!(fields.get("zone").map(Secret::expose), Some("nl-ams-1"));
    }

    #[test]
    fn bad_input_is_refused_without_echoing_it() {
        let secret = "fake-secret-that-must-not-be-echoed";
        let cases = [
            ("hetzner", "".to_owned(), "empty"),
            (
                "hetzner",
                format!("{secret}\nsecond line\n"),
                "expected format",
            ),
            ("ovh", secret.to_owned(), "expected format"),
            (
                "ovh",
                format!("OVH_APPLICATION_SECRET={secret}\n"),
                "no value for OVH_APPLICATION_KEY, OVH_CONSUMER_KEY, OVH_PROJECT_ID",
            ),
            ("ovh", format!("NOT_A_FIELD={secret}\n"), "expected format"),
            (
                "vultr",
                "VULTR_API_KEY=\n".to_owned(),
                "no value for VULTR_API_KEY",
            ),
        ];
        for (provider, input, expected) in cases {
            let err = parse_input(info(provider), &input).expect_err("refused");
            assert_eq!(err.kind, "invalid_input");
            assert!(
                err.message.contains(expected),
                "{provider}: {}",
                err.message
            );
            let shown = format!("{} {}", err.message, err.hint);
            assert!(!shown.contains(secret), "{provider}: leaked input: {shown}");
            assert!(
                err.hint.contains("=<value>"),
                "hint shows the format: {}",
                err.hint
            );
        }
    }

    // ---------- storing ----------

    fn fields_of(token: &str) -> BTreeMap<String, Secret> {
        BTreeMap::from([("token".to_owned(), Secret::new(token))])
    }

    #[test]
    fn the_os_store_is_used_when_it_works_and_a_stale_file_copy_is_removed() {
        let os = MemoryStore::new(Source::CredentialStore);
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "fake-old")]);
        let stores = Stores {
            os: Some(&os),
            file: Some(&file),
        };
        let report = store_credential(
            info("hetzner"),
            fields_of("fake-new"),
            &stores,
            &env_of(&[]),
        )
        .expect("stored");
        assert_eq!(report.store, Source::CredentialStore);
        assert_eq!(report.fields, vec!["HCLOUD_TOKEN"]);
        assert_eq!(report.fallback_reason, None);
        assert_eq!(report.next, "nimbus --provider hetzner verify");
        assert!(os.has("hetzner"));
        assert!(!file.has("hetzner"), "the stale copy must not linger");
    }

    #[test]
    fn with_no_os_store_the_file_is_used_and_the_report_says_so() {
        let os = MemoryStore::failing(Source::CredentialStore, "no secret service on this machine");
        let file = MemoryStore::new(Source::File);
        let stores = Stores {
            os: Some(&os),
            file: Some(&file),
        };
        let report = store_credential(
            info("hetzner"),
            fields_of("fake-new"),
            &stores,
            &env_of(&[]),
        )
        .expect("stored");
        assert_eq!(report.store, Source::File);
        assert_eq!(
            report.fallback_reason.as_deref(),
            Some("no secret service on this machine")
        );
        assert!(file.has("hetzner"));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn forcing_the_os_store_fails_rather_than_falling_back() {
        let os = MemoryStore::failing(Source::CredentialStore, "keychain is locked");
        let stores = Stores {
            os: Some(&os),
            file: None,
        };
        let err = store_credential(
            info("hetzner"),
            fields_of("fake-new"),
            &stores,
            &env_of(&[]),
        )
        .expect_err("no fallback");
        assert_eq!(err.kind, "credential_store_failed");
        assert!(err.message.contains("keychain is locked"));
        assert!(err.hint.contains("--store file"));
    }

    #[test]
    fn file_mode_never_touches_the_os_store() {
        let file = MemoryStore::new(Source::File);
        let stores = Stores {
            os: None,
            file: Some(&file),
        };
        let report = store_credential(
            info("hetzner"),
            fields_of("fake-new"),
            &stores,
            &env_of(&[]),
        )
        .expect("stored");
        assert_eq!(report.store, Source::File);
        assert_eq!(
            report.fallback_reason, None,
            "the file was the choice, not a fallback"
        );
    }

    #[test]
    fn login_warns_when_the_environment_will_shadow_what_was_stored() {
        let file = MemoryStore::new(Source::File);
        let stores = Stores {
            os: None,
            file: Some(&file),
        };
        let env = env_of(&[("HCLOUD_TOKEN", "fake-env-token")]);
        let report = store_credential(info("hetzner"), fields_of("fake-new"), &stores, &env)
            .expect("stored");
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("takes precedence"));
    }

    #[test]
    fn the_login_report_holds_no_credential_value() {
        let file = MemoryStore::new(Source::File);
        let stores = Stores {
            os: None,
            file: Some(&file),
        };
        let report = store_credential(
            info("hetzner"),
            fields_of("fake-secret-value-123"),
            &stores,
            &env_of(&[("HCLOUD_TOKEN", "fake-env-secret-456")]),
        )
        .expect("stored");
        let json = serde_json::to_string(&report).expect("json");
        assert!(!json.contains("fake-secret-value-123"), "{json}");
        assert!(!json.contains("fake-env-secret-456"), "{json}");
        assert!(json.contains("\"store\":\"file\""), "{json}");
    }

    // ---------- logout ----------

    #[test]
    fn logout_removes_the_credential_from_every_store() {
        let os = MemoryStore::new(Source::CredentialStore).with("ovh", &[("consumer_key", "x")]);
        let file = MemoryStore::new(Source::File).with("ovh", &[("consumer_key", "y")]);
        let stores = Stores {
            os: Some(&os),
            file: Some(&file),
        };
        let report = remove_credential(info("ovh"), &stores, &env_of(&[])).expect("removed");
        assert_eq!(report.removed, vec![Source::CredentialStore, Source::File]);
        assert!(report.warnings.is_empty());
        assert!(!os.has("ovh") && !file.has("ovh"));

        // Again: nothing left, still a success.
        let report = remove_credential(info("ovh"), &stores, &env_of(&[])).expect("idempotent");
        assert!(report.removed.is_empty());
    }

    #[test]
    fn logout_reports_what_it_could_not_clear() {
        let os = MemoryStore::failing(Source::CredentialStore, "no secret service");
        let file = MemoryStore::new(Source::File).with("hetzner", &[("token", "x")]);
        let stores = Stores {
            os: Some(&os),
            file: Some(&file),
        };
        let env = env_of(&[("HCLOUD_TOKEN", "fake-env-token")]);
        let report = remove_credential(info("hetzner"), &stores, &env).expect("removed");
        assert_eq!(report.removed, vec![Source::File]);
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
        assert!(report.warnings[0].contains("no secret service"));
        assert!(report.warnings[1].contains("HCLOUD_TOKEN still set"));
        assert!(!report.warnings[1].contains("fake-env-token"));

        let broken = MemoryStore::failing(Source::File, "permissions are 644");
        let stores = Stores {
            os: None,
            file: Some(&broken),
        };
        let err = remove_credential(info("hetzner"), &stores, &env_of(&[])).expect_err("must fail");
        assert_eq!(err.kind, "credential_store_failed");
    }

    #[test]
    fn store_mode_parses_its_three_values() {
        assert_eq!(StoreMode::parse("auto"), Ok(StoreMode::Auto));
        assert_eq!(StoreMode::parse("os"), Ok(StoreMode::Os));
        assert_eq!(StoreMode::parse("file"), Ok(StoreMode::File));
        assert_eq!(
            StoreMode::parse("keychain").expect_err("unknown").kind,
            "invalid_configuration"
        );
    }
}
