//! The fallback credential store: one JSON file in the config directory,
//! readable and writable by its owner only.
//!
//! This is for machines with no OS credential store — a headless Linux
//! server has no Secret Service to talk to. The values are not encrypted;
//! the file's permissions are the protection, so they are set when the file
//! is created (never widened-then-narrowed) and checked again on every read.

use crate::credentials::{CredentialStore, Source, StoreError, StoredCredential};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "credentials.json";
const FORMAT_VERSION: u32 = 1;

/// The file's contents. Entries are kept as raw JSON so that rewriting the
/// file for one provider cannot damage another provider's entry, even one of
/// a kind this version does not understand.
#[derive(Serialize, Deserialize)]
struct Contents {
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, serde_json::Value>,
}

impl Default for Contents {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            providers: BTreeMap::new(),
        }
    }
}

pub struct FileStore {
    path: PathBuf,
}

/// The directory nimbus keeps its files in: `$NIMBUS_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/nimbus`, else `~/.config/nimbus` (`%APPDATA%\nimbus`
/// where there is no home directory variable).
pub fn config_dir(env: &dyn Fn(&str) -> Option<String>) -> Result<PathBuf, StoreError> {
    if let Some(dir) = env("NIMBUS_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = env("XDG_CONFIG_HOME").map(PathBuf::from) {
        // The XDG spec says to ignore a relative value.
        if xdg.is_absolute() {
            return Ok(xdg.join("nimbus"));
        }
    }
    if let Some(home) = env("HOME") {
        return Ok(PathBuf::from(home).join(".config").join("nimbus"));
    }
    if let Some(appdata) = env("APPDATA") {
        return Ok(PathBuf::from(appdata).join("nimbus"));
    }
    Err(StoreError(
        "cannot locate a config directory: none of NIMBUS_CONFIG_DIR, XDG_CONFIG_HOME or HOME is set"
            .to_owned(),
    ))
}

impl FileStore {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join(FILE_NAME),
        }
    }

    fn fail(&self, what: &str, e: impl std::fmt::Display) -> StoreError {
        StoreError(format!("{what} {}: {e}", self.path.display()))
    }

    fn read(&self) -> Result<Option<Contents>, StoreError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(self.fail("cannot read", e)),
        };
        self.check_permissions()?;
        // A parse error's message can quote the text around the fault, which
        // here would be a credential — report where, not what.
        let contents: Contents = serde_json::from_str(&text).map_err(|e| {
            self.fail(
                "cannot parse",
                format!(
                    "not valid JSON at line {} column {}; fix it or delete it and run `nimbus login` again",
                    e.line(),
                    e.column()
                ),
            )
        })?;
        if contents.version != FORMAT_VERSION {
            return Err(self.fail(
                "cannot use",
                format!(
                    "it has format version {}, this nimbus understands version {FORMAT_VERSION}; upgrade nimbus",
                    contents.version
                ),
            ));
        }
        Ok(Some(contents))
    }

    #[cfg(unix)]
    fn check_permissions(&self) -> Result<(), StoreError> {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&self.path)
            .map_err(|e| self.fail("cannot inspect", e))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            return Err(self.fail(
                "refusing to use",
                format!(
                    "its permissions are {mode:03o}, so other users can read the credentials in it; run `chmod 600 {}`",
                    self.path.display()
                ),
            ));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn check_permissions(&self) -> Result<(), StoreError> {
        // No mode bits to check; the per-user profile directory's ACL is
        // what restricts the file there.
        Ok(())
    }

    /// Writes the whole file atomically: a new owner-only file beside it,
    /// then a rename over the old one. The credentials never exist in a file
    /// with wider permissions, not even briefly.
    fn write(&self, contents: &Contents) -> Result<(), StoreError> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| self.fail("cannot write", "the path has no parent directory"))?;
        create_private_dir(dir).map_err(|e| self.fail("cannot create the directory for", e))?;

        let json = serde_json::to_string_pretty(contents)
            .map_err(|_| self.fail("cannot write", "the credentials could not be encoded"))?;
        let tmp = dir.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
        // A leftover from an interrupted run would make `create_new` fail.
        let _ = fs::remove_file(&tmp);
        let result = (|| {
            let mut file = private_file_options().open(&tmp)?;
            file.write_all(json.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(|e| self.fail("cannot write", e))
    }
}

#[cfg(unix)]
fn private_file_options() -> fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    options
}

#[cfg(not(unix))]
fn private_file_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    options
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

impl CredentialStore for FileStore {
    fn source(&self) -> Source {
        Source::File
    }

    fn location(&self) -> String {
        self.path.display().to_string()
    }

    fn get(&self, provider: &str) -> Result<Option<StoredCredential>, StoreError> {
        let Some(contents) = self.read()? else {
            return Ok(None);
        };
        let Some(entry) = contents.providers.get(provider) else {
            return Ok(None);
        };
        // As in `read`: say that the entry is unusable, not what is in it.
        serde_json::from_value(entry.clone()).map(Some).map_err(|_| {
            self.fail(
                "cannot use",
                format!("the entry for {provider} is malformed; run `nimbus login --provider {provider}` again"),
            )
        })
    }

    fn set(&self, provider: &str, credential: &StoredCredential) -> Result<(), StoreError> {
        let mut contents = self.read()?.unwrap_or_default();
        let entry = serde_json::to_value(credential)
            .map_err(|_| self.fail("cannot write", "the credential could not be encoded"))?;
        contents.providers.insert(provider.to_owned(), entry);
        self.write(&contents)
    }

    fn delete(&self, provider: &str) -> Result<bool, StoreError> {
        let Some(mut contents) = self.read()? else {
            return Ok(false);
        };
        if contents.providers.remove(provider).is_none() {
            return Ok(false);
        }
        if contents.providers.is_empty() {
            fs::remove_file(&self.path).map_err(|e| self.fail("cannot remove", e))?;
        } else {
            self.write(&contents)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::testing::{env_of, static_credential};

    fn store() -> (tempfile::TempDir, FileStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FileStore::in_dir(&dir.path().join("nimbus"));
        (dir, store)
    }

    #[test]
    fn round_trips_and_deletes() {
        let (_dir, store) = store();
        assert_eq!(store.get("hetzner"), Ok(None), "no file yet");
        assert_eq!(store.delete("hetzner"), Ok(false));

        let hetzner = static_credential(&[("token", "fake-hetzner-token")]);
        let ovh = static_credential(&[("consumer_key", "fake-ck"), ("project_id", "fake-project")]);
        store.set("hetzner", &hetzner).expect("set");
        store.set("ovh", &ovh).expect("set");
        assert_eq!(store.get("hetzner"), Ok(Some(hetzner)));
        assert_eq!(store.get("ovh"), Ok(Some(ovh.clone())));
        assert_eq!(store.get("vultr"), Ok(None));

        assert_eq!(store.delete("hetzner"), Ok(true));
        assert_eq!(store.get("hetzner"), Ok(None));
        assert_eq!(store.get("ovh"), Ok(Some(ovh)), "other entries survive");

        assert_eq!(store.delete("ovh"), Ok(true));
        assert!(
            !store.path.exists(),
            "an empty file is removed, not left behind"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_and_directory_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        store
            .set("hetzner", &static_credential(&[("token", "fake-token")]))
            .expect("set");
        let mode = |p: &Path| fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(&store.path), 0o600);
        assert_eq!(mode(store.path.parent().expect("parent")), 0o700);

        // Rewriting keeps it that way, and leaves no temporary file behind.
        store
            .set("vultr", &static_credential(&[("api_key", "fake-key")]))
            .expect("set");
        assert_eq!(mode(&store.path), 0o600);
        let names: Vec<_> = fs::read_dir(store.path.parent().expect("parent"))
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from(FILE_NAME)]);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_read_is_refused_with_the_fix() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        store
            .set("hetzner", &static_credential(&[("token", "fake-token")]))
            .expect("set");
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o644)).expect("chmod");

        let err = store.get("hetzner").expect_err("too open");
        assert!(err.0.contains("644"), "{err}");
        assert!(err.0.contains("chmod 600"), "{err}");
        assert!(!err.0.contains("fake-token"), "{err}");
        assert!(store
            .set("vultr", &static_credential(&[("api_key", "k")]))
            .is_err());
    }

    #[test]
    fn an_entry_of_an_unknown_kind_survives_a_rewrite() {
        let (_dir, store) = store();
        store
            .set("hetzner", &static_credential(&[("token", "fake-token")]))
            .expect("set");
        // Simulate a newer nimbus having stored a different kind for Vultr.
        let mut contents = store.read().expect("read").expect("exists");
        let newer = serde_json::json!({ "kind": "oauth", "access_token": "fake-access" });
        contents.providers.insert("vultr".into(), newer.clone());
        store.write(&contents).expect("write");

        assert_eq!(store.get("vultr"), Ok(Some(StoredCredential::Unsupported)));
        store
            .set("linode", &static_credential(&[("token", "fake-linode")]))
            .expect("set");
        let contents = store.read().expect("read").expect("exists");
        assert_eq!(contents.providers.get("vultr"), Some(&newer));
    }

    #[test]
    fn a_corrupt_file_is_reported_without_quoting_it() {
        let (_dir, store) = store();
        store
            .set(
                "hetzner",
                &static_credential(&[("token", "fake-token-value")]),
            )
            .expect("set");
        let text = fs::read_to_string(&store.path).expect("read");
        let broken = text.replacen("\"fake-token-value\"", "\"fake-token-value", 1);
        let mut file = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&store.path)
            .expect("open");
        file.write_all(broken.as_bytes()).expect("write");
        drop(file);

        let err = store.get("hetzner").expect_err("corrupt");
        assert!(err.0.contains("not valid JSON"), "{err}");
        assert!(!err.0.contains("fake-token-value"), "{err}");
    }

    #[test]
    fn config_dir_resolution_order() {
        let dir =
            |vars: &[(&str, &str)]| config_dir(&env_of(vars)).map(|p| p.display().to_string());
        assert_eq!(
            dir(&[
                ("NIMBUS_CONFIG_DIR", "/a"),
                ("XDG_CONFIG_HOME", "/b"),
                ("HOME", "/c")
            ]),
            Ok("/a".to_owned())
        );
        assert_eq!(
            dir(&[("XDG_CONFIG_HOME", "/b"), ("HOME", "/c")]),
            Ok(PathBuf::from("/b").join("nimbus").display().to_string())
        );
        assert_eq!(
            dir(&[("XDG_CONFIG_HOME", "relative"), ("HOME", "/c")]),
            Ok(PathBuf::from("/c")
                .join(".config")
                .join("nimbus")
                .display()
                .to_string())
        );
        assert!(dir(&[]).is_err());
    }
}
