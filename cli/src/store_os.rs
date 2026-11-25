//! The operating system's credential store: Keychain on macOS, the Secret
//! Service (GNOME Keyring, KWallet, ...) on Linux, Credential Manager on
//! Windows. One item per provider, holding that provider's
//! [`StoredCredential`] as JSON.
//!
//! Tests never construct this type — they would be writing to the keychain
//! of whoever runs them. Everything above it is tested through the
//! [`CredentialStore`] trait with an in-memory store instead.

use crate::credentials::{CredentialStore, Source, StoreError, StoredCredential};
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

/// The service name every item is filed under.
const SERVICE: &str = "nimbus-cli";

/// How long to wait for the store before giving up. A store can block
/// indefinitely — an unlock prompt nobody can see, a D-Bus service that never
/// answers — and a command that hangs tells its caller nothing. Long enough
/// for a person to answer a prompt that is actually on their screen.
const TIMEOUT: Duration = Duration::from_secs(60);

pub struct OsStore;

fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else {
        "Secret Service keyring"
    }
}

/// Describes a keyring failure without its payload. Two of the variants
/// carry the bytes that were read from the store — the credential itself —
/// so the error is never formatted with `Debug` or passed along whole.
fn describe(error: keyring::Error) -> StoreError {
    use keyring::Error as E;
    let reason = match error {
        E::NoStorageAccess(platform) => format!("it cannot be accessed ({platform})"),
        E::PlatformFailure(platform) => format!("it reported a failure ({platform})"),
        E::NoDefaultStore => "none is available on this machine".to_owned(),
        E::NotSupportedByStore(why) => format!("it does not support this ({why})"),
        E::Invalid(what, why) => format!("it rejected `{what}` ({why})"),
        E::TooLong(what, limit) => format!("`{what}` exceeds its limit of {limit}"),
        E::BadStoreFormat(why) => format!("it is not in the expected format ({why})"),
        E::BadEncoding(_) | E::BadDataFormat(_, _) => {
            "the stored item is not in the expected format".to_owned()
        }
        E::Ambiguous(_) => "more than one matching item is stored".to_owned(),
        E::NoEntry => "no item is stored".to_owned(),
        _ => "it failed for an unrecognised reason".to_owned(),
    };
    StoreError(format!("{}: {reason}", store_name()))
}

/// Runs keyring calls with a time limit, and stops trying once one has gone
/// unanswered.
struct Gate {
    timeout: Duration,
    /// Set by the first call that timed out. Later calls fail with the same
    /// message at once: `nimbus providers` asks the store about every
    /// provider, and waiting out the limit for each would turn one dead
    /// store into minutes of silence.
    gave_up: OnceLock<StoreError>,
}

static GATE: Gate = Gate {
    timeout: TIMEOUT,
    gave_up: OnceLock::new(),
};

impl Gate {
    /// Runs `call` on its own thread and stops waiting after the limit. The
    /// thread is left behind in that case; nothing else will be asked of the
    /// store in this process.
    fn run<T: Send + 'static>(
        &self,
        call: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        if let Some(error) = self.gave_up.get() {
            return Err(error.clone());
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // The receiver is gone only if we already timed out.
            let _ = tx.send(call());
        });
        rx.recv_timeout(self.timeout).unwrap_or_else(|_| {
            let error = StoreError(format!(
                "{}: no answer after {} seconds — it may be waiting for an unlock prompt nobody can see, or there is no desktop session. Set NIMBUS_CREDENTIAL_STORE=file to use the credentials file instead",
                store_name(),
                self.timeout.as_secs()
            ));
            Err(self.gave_up.get_or_init(|| error).clone())
        })
    }
}

fn with_timeout<T: Send + 'static>(
    call: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    GATE.run(call)
}

fn entry(provider: &str) -> Result<keyring::Entry, StoreError> {
    keyring::Entry::new(SERVICE, provider).map_err(describe)
}

impl CredentialStore for OsStore {
    fn source(&self) -> Source {
        Source::CredentialStore
    }

    fn location(&self) -> String {
        format!("{} (service \"{SERVICE}\")", store_name())
    }

    fn get(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError> {
        let provider = provider.to_owned();
        with_timeout(move || {
            let json = match entry(&provider)?.get_password() {
                Ok(json) => json,
                Err(keyring::Error::NoEntry) => return Ok(None),
                Err(e) => return Err(describe(e)),
            };
            // A serde error can quote the input, which here is the credential.
            serde_json::from_str(&json).map(Some).map_err(|_| {
                StoreError(format!(
                    "{}: the item for {provider} is not in the expected format; run `nimbus login --provider {provider}` again",
                    store_name()
                ))
            })
        })
    }

    fn set(&self, provider: &str, credential: &StoredCredential) -> Result<(), StoreError> {
        let provider = provider.to_owned();
        let json = serde_json::to_string(credential).map_err(|_| {
            StoreError(format!(
                "{}: the credential could not be encoded",
                store_name()
            ))
        })?;
        with_timeout(move || entry(&provider)?.set_password(&json).map_err(describe))
    }

    fn delete(&self, provider: &str) -> Result<bool, StoreError> {
        let provider = provider.to_owned();
        with_timeout(move || match entry(&provider)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(describe(e)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_that_carry_the_stored_bytes_are_described_without_them() {
        let secret = b"fake-stored-secret".to_vec();
        let described = describe(keyring::Error::BadEncoding(secret));
        assert!(!described.0.contains("fake-stored-secret"), "{described}");
        assert!(described.0.contains("not in the expected format"));
    }

    fn gate(timeout: Duration) -> Gate {
        Gate {
            timeout,
            gave_up: OnceLock::new(),
        }
    }

    #[test]
    fn a_call_that_answers_is_passed_through() {
        let gate = gate(Duration::from_secs(30));
        assert_eq!(gate.run(|| Ok(7)), Ok(7));
        assert_eq!(
            gate.run(|| Err::<(), _>(StoreError("nope".into()))),
            Err(StoreError("nope".into())),
        );
        assert_eq!(gate.run(|| Ok(8)), Ok(8), "an error is not a timeout");
    }

    #[test]
    fn a_store_that_never_answers_is_asked_only_once() {
        let gate = gate(Duration::from_millis(20));
        let (release, hold) = mpsc::channel::<()>();
        let first = gate.run(move || {
            // Blocks until the test ends, like an unlock prompt nobody sees.
            let _ = hold.recv();
            Ok(())
        });
        let error = first.expect_err("timed out");
        assert!(error.0.contains("no answer"), "{error}");
        assert!(error.0.contains("NIMBUS_CREDENTIAL_STORE=file"), "{error}");

        let started = std::time::Instant::now();
        let second = gate.run(|| -> Result<(), StoreError> {
            panic!("the store must not be asked again");
        });
        assert_eq!(second, Err(error));
        assert!(started.elapsed() < Duration::from_millis(20));
        drop(release);
    }
}
