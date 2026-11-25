//! Everything the CLI writes goes through here.
//!
//! Two guarantees are enforced in this one place rather than left to each
//! command: results are JSON on stdout and failures are JSON on stderr, and
//! no credential value appears in either. The second is a backstop — nothing
//! is supposed to put a secret into a message in the first place — but a
//! provider that echoed a token back in an error body, or a future change
//! that formatted the wrong field, would otherwise leak it to whatever is
//! reading the output.

use serde::Serialize;
use std::sync::Mutex;

const REDACTED: &str = "[redacted]";

/// Shorter values are not registered: a real credential is far longer, and
/// blanking every occurrence of a three-letter string would shred the output.
const MIN_SECRET_LEN: usize = 8;

/// Credential values seen by this process, to be blanked out of all output.
static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Marks a value as never to be printed.
pub fn register_secret(value: &str) {
    if value.len() < MIN_SECRET_LEN {
        return;
    }
    let mut secrets = SECRETS.lock().unwrap_or_else(|e| e.into_inner());
    if !secrets.iter().any(|s| s == value) {
        secrets.push(value.to_owned());
    }
}

/// Replaces every occurrence of each secret — and of its JSON-escaped form,
/// since most output is JSON — with a placeholder.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        out = out.replace(secret, REDACTED);
        if let Ok(quoted) = serde_json::to_string(secret) {
            let escaped = &quoted[1..quoted.len() - 1];
            if escaped != secret {
                out = out.replace(escaped, REDACTED);
            }
        }
    }
    out
}

fn redact_registered(text: &str) -> String {
    let secrets = SECRETS.lock().unwrap_or_else(|e| e.into_inner());
    redact(text, &secrets)
}

/// A failure, in the shape it is reported: what went wrong and what to do
/// about it. The person on the other end of an agent cannot debug, so `hint`
/// is not optional decoration — every error the CLI raises itself carries
/// one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliError {
    /// Stable machine-readable category, e.g. `missing_credentials`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub message: String,
    /// The next thing to do.
    pub hint: String,
}

impl CliError {
    pub fn new(kind: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            kind,
            provider: None,
            message: message.into(),
            hint: hint.into(),
        }
    }

    pub fn for_provider(mut self, provider: &str) -> Self {
        self.provider = Some(provider.to_owned());
        self
    }
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: &'a CliError,
}

fn to_json<T: Serialize>(value: &T) -> String {
    // Serializing the CLI's own plain data types cannot fail; if it somehow
    // did, saying so beats printing nothing.
    serde_json::to_string_pretty(value).unwrap_or_else(|e| {
        format!("{{\n  \"error\": {{\n    \"kind\": \"internal\",\n    \"message\": \"could not encode output: {e}\"\n  }}\n}}")
    })
}

/// Prints a result as pretty JSON on stdout.
pub fn print_json<T: Serialize>(value: &T) {
    println!("{}", redact_registered(&to_json(value)));
}

/// Prints a plain confirmation line on stdout (`deleted <id>` and friends,
/// kept as they always were).
pub fn print_line(line: &str) {
    println!("{}", redact_registered(line));
}

/// Prints a failure as JSON on stderr: `{"error": {kind, provider?, message, hint}}`.
pub fn print_error(error: &CliError) {
    eprintln!("{}", redact_registered(&to_json(&ErrorEnvelope { error })));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_owned()).collect()
    }

    #[test]
    fn every_occurrence_is_blanked() {
        let out = redact(
            "token fake-secret-1 rejected; retry with fake-secret-1 or fake-secret-2",
            &secrets(&["fake-secret-1", "fake-secret-2"]),
        );
        assert_eq!(
            out,
            "token [redacted] rejected; retry with [redacted] or [redacted]"
        );
    }

    #[test]
    fn the_json_escaped_form_is_blanked_too() {
        let secret = "fake\"quoted\\secret";
        let json = serde_json::to_string(&serde_json::json!({ "message": secret })).expect("json");
        assert!(
            !json.contains(secret),
            "the raw form is not what JSON holds"
        );
        let out = redact(&json, &secrets(&[secret]));
        assert_eq!(out, r#"{"message":"[redacted]"}"#);
    }

    #[test]
    fn text_without_secrets_is_untouched() {
        let text = r#"{"id": "fsn1", "name": "Falkenstein"}"#;
        assert_eq!(redact(text, &secrets(&["fake-secret-1"])), text);
        assert_eq!(redact(text, &[]), text);
    }

    #[test]
    fn short_values_are_not_registered() {
        register_secret("abc");
        register_secret("fake-registered-secret");
        let out = redact_registered("abc and fake-registered-secret");
        assert_eq!(out, "abc and [redacted]");
    }

    #[test]
    fn errors_serialize_with_kind_message_and_hint() {
        let error = CliError::new("missing_credentials", "none found", "run the login command")
            .for_provider("hetzner");
        let json: serde_json::Value =
            serde_json::from_str(&to_json(&ErrorEnvelope { error: &error })).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "error": {
                "kind": "missing_credentials",
                "provider": "hetzner",
                "message": "none found",
                "hint": "run the login command",
            }})
        );
    }
}
