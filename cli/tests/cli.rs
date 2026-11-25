//! End-to-end tests: the real `nimbus` binary against nimbus-mock.
//!
//! Every invocation runs in a sandbox — a cleared environment, a throwaway
//! config directory, and `NIMBUS_CREDENTIAL_STORE=file` so the operating
//! system's keychain is never read or written. Nothing here can see, use or
//! disturb a credential belonging to whoever runs the tests.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Distinctive fake secrets, so a leak anywhere in the output is findable.
const ENV_SECRET: &str = "fake-env-secret-Zq7Lm2Xv9Rt4Kp8W";
const STORED_SECRET: &str = "fake-stored-secret-Hd5Ny3Bc6Ju1Fs0E";

const SSH_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAItest test@example.com";
const SCRIPT: &str = "#!/bin/sh\nset -eu\n# première étape\nexport JOIN_CODE=fake-single-use-code==\ncurl -fsSL https://install.example.invalid/setup.sh | sh\n";

struct Sandbox {
    dir: tempfile::TempDir,
    /// Root of the mock server.
    base: String,
}

struct Ran {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Ran {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {:?}", self.stdout))
    }

    /// The `error` object every failure prints on stderr.
    fn error(&self) -> Value {
        let json: Value = serde_json::from_str(&self.stderr)
            .unwrap_or_else(|e| panic!("stderr is not JSON ({e}): {:?}", self.stderr));
        assert!(json["error"].is_object(), "no error object: {json}");
        assert!(
            json["error"]["hint"]
                .as_str()
                .is_some_and(|h| !h.is_empty()),
            "every failure must say what to do next: {json}"
        );
        json["error"].clone()
    }

    fn ok(self) -> Self {
        assert_eq!(self.code, 0, "expected success; stderr: {}", self.stderr);
        assert!(self.stderr.is_empty(), "unexpected stderr: {}", self.stderr);
        self
    }

    fn failed(self) -> Self {
        assert_eq!(self.code, 1, "expected exit 1; stdout: {}", self.stdout);
        self
    }
}

impl Sandbox {
    async fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
            base: nimbus_mock::spawn().await,
        }
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn credentials_file(&self) -> PathBuf {
        self.config_dir().join("credentials.json")
    }

    /// Each adapter's API root on the mock.
    fn base_url(&self, provider: &str) -> String {
        let suffix = match provider {
            "hetzner" => "/v1",
            "vultr" => "/v2",
            "ovh" => "/1.0",
            "digitalocean" => "/do/v2",
            "linode" => "/v4",
            "scaleway" => "",
            "hostinger" => "/api",
            other => panic!("unknown provider {other}"),
        };
        format!("{}{suffix}", self.base)
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nimbus"));
        command
            .args(args)
            .env_clear()
            .env("NIMBUS_CREDENTIAL_STORE", "file")
            .env("NIMBUS_CONFIG_DIR", self.config_dir())
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Ran {
        ran(self.command(args, env).output().expect("run nimbus"))
    }

    fn run_with_stdin(&self, args: &[&str], env: &[(&str, &str)], input: &str) -> Ran {
        let mut child = self
            .command(args, env)
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn nimbus");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
        ran(child.wait_with_output().expect("wait"))
    }

    /// Runs a provider command against the mock.
    fn api(&self, provider: &str, args: &[&str], env: &[(&str, &str)]) -> Ran {
        let base = self.base_url(provider);
        let mut all = vec!["--provider", provider, "--base-url", &base];
        all.extend_from_slice(args);
        self.run(&all, env)
    }

    fn write(&self, name: &str, contents: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, contents).expect("write file");
        path.display().to_string()
    }

    async fn mock_get(&self, path: &str) -> Option<Value> {
        let resp = reqwest::get(format!("{}{path}", self.base))
            .await
            .expect("mock request");
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return None;
        }
        Some(resp.json().await.expect("mock json"))
    }

    /// The write requests the mock has served so far.
    async fn writes(&self) -> Vec<String> {
        let log = self.mock_get("/_mock/requests").await.expect("request log");
        log.as_array()
            .expect("array")
            .iter()
            .filter(|r| r["method"] != "GET")
            .map(|r| {
                format!(
                    "{} {}",
                    r["method"].as_str().unwrap_or(""),
                    r["path"].as_str().unwrap_or("")
                )
            })
            .collect()
    }
}

fn ran(output: Output) -> Ran {
    Ran {
        code: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

/// A (region, type, image) each mock accepts, and its credential env vars.
fn target(provider: &str) -> (&'static str, &'static str, &'static str) {
    match provider {
        "hetzner" => ("fsn1", "cx22", "ubuntu-24.04"),
        "vultr" => ("ewr", "vc2-1c-1gb", "2284"),
        "ovh" => ("GRA", "d2-2", "img-uuid"),
        "digitalocean" => ("nyc3", "s-1vcpu-1gb", "ubuntu-24-04-x64"),
        "linode" => ("us-east", "g6-nanode-1", "linode/ubuntu24.04"),
        "scaleway" => ("fr-par-1", "DEV1-S", "img-uuid"),
        "hostinger" => ("19", "hostingercom-vps-kvm2-usd-1m", "1130"),
        other => panic!("unknown provider {other}"),
    }
}

fn env_for(provider: &str, secret: &'static str) -> Vec<(&'static str, &'static str)> {
    match provider {
        "hetzner" => vec![("HCLOUD_TOKEN", secret)],
        "vultr" => vec![("VULTR_API_KEY", secret)],
        "ovh" => vec![
            ("OVH_APPLICATION_KEY", "fake-app-key"),
            ("OVH_APPLICATION_SECRET", "fake-app-secret-value"),
            ("OVH_CONSUMER_KEY", secret),
            ("OVH_PROJECT_ID", "test-project"),
        ],
        "digitalocean" => vec![("DIGITALOCEAN_TOKEN", secret)],
        "linode" => vec![("LINODE_TOKEN", secret)],
        "scaleway" => vec![
            ("SCW_SECRET_KEY", secret),
            ("SCW_DEFAULT_PROJECT_ID", "test-project"),
        ],
        "hostinger" => vec![("HOSTINGER_API_TOKEN", secret)],
        other => panic!("unknown provider {other}"),
    }
}

const ALL: [&str; 7] = [
    "hetzner",
    "vultr",
    "ovh",
    "digitalocean",
    "scaleway",
    "linode",
    "hostinger",
];

fn find<'j>(providers: &'j Value, id: &str) -> &'j Value {
    providers
        .as_array()
        .expect("providers is an array")
        .iter()
        .find(|p| p["id"] == id)
        .unwrap_or_else(|| panic!("{id} not listed"))
}

// ---------------------------------------------------------------- providers

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn providers_works_with_nothing_configured() {
    let sandbox = Sandbox::new().await;
    let out = sandbox.run(&["providers"], &[]).ok();
    let providers = out.json();
    assert_eq!(providers.as_array().map(Vec::len), Some(7));
    for id in ALL {
        let p = find(&providers, id);
        assert_eq!(p["credentials"]["available"], false, "{id}");
        assert_eq!(p["credentials"]["source"], Value::Null, "{id}");
        assert_eq!(p["login_command"], format!("nimbus login --provider {id}"));
        assert!(p["name"].is_string() && p["credential_fields"].is_array());
        for caveat in [
            "can_delete_instances",
            "reports_prices",
            "create_is_irreversible_purchase",
            "supports_user_data",
        ] {
            assert!(p["caveats"][caveat].is_boolean(), "{id}: {caveat}");
        }
    }
    let hostinger = find(&providers, "hostinger");
    assert_eq!(hostinger["caveats"]["can_delete_instances"], false);
    assert_eq!(
        hostinger["caveats"]["create_is_irreversible_purchase"],
        true
    );
    assert_eq!(find(&providers, "ovh")["caveats"]["reports_prices"], false);
    assert!(
        !sandbox.config_dir().exists(),
        "listing providers must not create anything"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn providers_reports_each_credential_source_and_no_values() {
    let sandbox = Sandbox::new().await;
    sandbox
        .run_with_stdin(
            &["login", "--provider", "vultr", "--stdin"],
            &[],
            STORED_SECRET,
        )
        .ok();
    let out = sandbox
        .run(&["providers"], &[("HCLOUD_TOKEN", ENV_SECRET)])
        .ok();
    let providers = out.json();
    assert_eq!(
        find(&providers, "hetzner")["credentials"]["source"],
        "environment"
    );
    assert_eq!(
        find(&providers, "hetzner")["credentials"]["available"],
        true
    );
    assert_eq!(find(&providers, "vultr")["credentials"]["source"], "file");
    assert_eq!(
        find(&providers, "linode")["credentials"]["available"],
        false
    );
    assert!(!out.stdout.contains(ENV_SECRET) && !out.stdout.contains(STORED_SECRET));
}

// ------------------------------------------------------------ login / logout

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_stores_a_credential_that_later_commands_use_until_logout() {
    let sandbox = Sandbox::new().await;

    // Nothing stored: the error names the provider and the exact command.
    let missing = sandbox.api("hetzner", &["regions"], &[]).failed();
    assert!(missing.stdout.is_empty());
    let error = missing.error();
    assert_eq!(error["kind"], "missing_credentials");
    assert_eq!(error["provider"], "hetzner");
    assert!(error["message"]
        .as_str()
        .is_some_and(|m| m.contains("Hetzner Cloud")));
    assert!(error["hint"].as_str().is_some_and(|h| h
        .contains("`nimbus login --provider hetzner`")
        && h.contains("HCLOUD_TOKEN")));

    let login = sandbox
        .run_with_stdin(
            &["login", "--provider", "hetzner", "--stdin"],
            &[],
            &format!("{STORED_SECRET}\n"),
        )
        .ok();
    let report = login.json();
    assert_eq!(report["provider"], "hetzner");
    assert_eq!(report["stored"], true);
    assert_eq!(
        report["store"], "file",
        "the output says which store was used"
    );
    assert_eq!(
        report["location"],
        sandbox.credentials_file().display().to_string()
    );
    assert_eq!(report["fields"], serde_json::json!(["HCLOUD_TOKEN"]));
    assert_eq!(report["next"], "nimbus --provider hetzner verify");
    assert!(
        !login.stdout.contains(STORED_SECRET),
        "login echoed the secret"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            |p: PathBuf| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode(sandbox.credentials_file()), 0o600, "owner-only file");
        assert_eq!(mode(sandbox.config_dir()), 0o700, "owner-only directory");
    }

    // The stored credential is now what commands use.
    let regions = sandbox.api("hetzner", &["regions"], &[]).ok().json();
    assert_eq!(regions[0]["id"], "fsn1");
    let verify = sandbox.api("hetzner", &["verify"], &[]).ok().json();
    assert_eq!(verify["credential_source"], "file");
    assert_eq!(verify["authenticated"], true);

    let logout = sandbox
        .run(&["logout", "--provider", "hetzner"], &[])
        .ok()
        .json();
    assert_eq!(logout["removed"], serde_json::json!(["file"]));
    assert!(!sandbox.credentials_file().exists());
    assert_eq!(
        sandbox.api("hetzner", &["regions"], &[]).failed().error()["kind"],
        "missing_credentials"
    );

    // Logging out again is not an error.
    let again = sandbox
        .run(&["logout", "--provider", "hetzner"], &[])
        .ok()
        .json();
    assert_eq!(again["removed"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_from_file_stores_every_field_of_a_multi_value_provider() {
    let sandbox = Sandbox::new().await;
    let file = sandbox.write(
        "ovh.env",
        &format!(
            "OVH_APPLICATION_KEY=fake-app-key\nOVH_APPLICATION_SECRET=fake-app-secret-value\nOVH_CONSUMER_KEY={STORED_SECRET}\nOVH_PROJECT_ID=test-project\n"
        ),
    );
    let login = sandbox
        .run(&["login", "--provider", "ovh", "--from-file", &file], &[])
        .ok();
    assert_eq!(
        login.json()["fields"],
        serde_json::json!([
            "OVH_APPLICATION_KEY",
            "OVH_APPLICATION_SECRET",
            "OVH_CONSUMER_KEY",
            "OVH_PROJECT_ID"
        ])
    );
    assert!(!login.stdout.contains(STORED_SECRET));
    assert!(!login.stdout.contains("fake-app-secret-value"));

    let verify = sandbox.api("ovh", &["verify"], &[]).ok().json();
    assert_eq!(verify["credential_source"], "file");
    assert_eq!(verify["can_create_instances"], "yes");

    // A file that is missing a field is refused, and says which one.
    let partial = sandbox.write("partial.env", "OVH_APPLICATION_KEY=fake-app-key\n");
    let error = sandbox
        .run(
            &["login", "--provider", "ovh", "--from-file", &partial],
            &[],
        )
        .failed()
        .error();
    assert_eq!(error["kind"], "invalid_input");
    assert!(error["message"]
        .as_str()
        .is_some_and(|m| m.contains("OVH_APPLICATION_SECRET")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_is_never_accepted_as_an_argument() {
    let sandbox = Sandbox::new().await;
    for args in [
        vec!["login", "--provider", "hetzner", "--token", STORED_SECRET],
        vec!["login", "--provider", "hetzner", STORED_SECRET],
        vec!["login", "--provider", "hetzner", "--stdin", STORED_SECRET],
    ] {
        let out = sandbox.run(&args, &[]);
        assert_eq!(out.code, 2, "usage error expected for {args:?}");
        assert!(
            !sandbox.credentials_file().exists(),
            "nothing may be stored from an argument"
        );
        // Nor is the value quoted back in the complaint about it.
        assert_no_secret("login with a secret argument", &out);
        let error = out.error();
        assert_eq!(error["kind"], "usage");
        assert!(error["hint"]
            .as_str()
            .is_some_and(|h| h.contains("never passed on the command line")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_errors_are_json_and_help_is_not_an_error() {
    let sandbox = Sandbox::new().await;

    let missing = sandbox.run(
        &["--provider", "hetzner", "instance", "create", "web-1"],
        &[],
    );
    assert_eq!(missing.code, 2, "clap's usage status is kept");
    assert!(missing.stdout.is_empty());
    let error = missing.error();
    assert_eq!(error["kind"], "usage");
    assert!(error["message"]
        .as_str()
        .is_some_and(|m| m.contains("required") && m.contains("--type <TYPE>")));
    assert!(error["hint"]
        .as_str()
        .is_some_and(|h| h.contains("Usage: nimbus instance create") && h.contains("--help")));

    let unknown = sandbox.run(&["frobnicate"], &[]);
    assert_eq!(unknown.code, 2);
    assert_eq!(unknown.error()["kind"], "usage");

    let help = sandbox.run(&["instance", "create", "--help"], &[]).ok();
    assert!(help.stdout.contains("--user-data-file <PATH>"));
    for command in ["providers", "login", "logout", "verify", "images"] {
        assert!(
            sandbox.run(&["--help"], &[]).ok().stdout.contains(command),
            "--help does not mention {command}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_without_a_terminal_says_how_to_proceed() {
    let sandbox = Sandbox::new().await;
    // stdin is /dev/null: what an agent's shell looks like.
    let error = sandbox
        .run(&["login", "--provider", "linode"], &[])
        .failed()
        .error();
    assert_eq!(error["kind"], "no_terminal");
    let hint = error["hint"].as_str().expect("hint");
    assert!(hint.contains("`nimbus login --provider linode`"), "{hint}");
    assert!(
        hint.contains("--from-file") && hint.contains("--stdin"),
        "{hint}"
    );
}

/// A Linux machine with no Secret Service — a server, a container, a CI
/// runner. The session bus address points at a socket that does not exist,
/// so the real keyring client runs and fails exactly as it does there, and
/// no keyring that might exist on the machine running the tests can be
/// reached. (Linux only: on macOS the store is the Keychain, which a test
/// must not write to.)
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_secret_service_login_falls_back_to_the_file_and_says_so() {
    let sandbox = Sandbox::new().await;
    let no_bus = format!(
        "unix:path={}",
        sandbox.dir.path().join("no-session-bus").display()
    );
    let env = [
        ("NIMBUS_CREDENTIAL_STORE", "auto"),
        ("DBUS_SESSION_BUS_ADDRESS", no_bus.as_str()),
    ];

    let login = sandbox
        .run_with_stdin(
            &["login", "--provider", "hetzner", "--stdin"],
            &env,
            STORED_SECRET,
        )
        .ok();
    let report = login.json();
    assert_eq!(report["stored"], true);
    assert_eq!(report["store"], "file", "{report}");
    assert_eq!(
        report["location"],
        sandbox.credentials_file().display().to_string()
    );
    assert!(
        report["fallback_reason"]
            .as_str()
            .is_some_and(|r| r.contains("Secret Service")),
        "the report must say why the OS store was not used: {report}"
    );
    assert_no_secret("login fallback", &login);

    // Reading falls through the same way.
    let providers = sandbox.run(&["providers"], &env).ok().json();
    assert_eq!(find(&providers, "hetzner")["credentials"]["source"], "file");
    let verify = sandbox.api("hetzner", &["verify"], &env).ok().json();
    assert_eq!(verify["credential_source"], "file");

    // Insisting on the OS store fails instead, and says what to do.
    let forced = sandbox
        .run_with_stdin(
            &["login", "--provider", "vultr", "--stdin", "--store", "os"],
            &env,
            STORED_SECRET,
        )
        .failed();
    assert_no_secret("login --store os", &forced);
    let error = forced.error();
    assert_eq!(error["kind"], "credential_store_failed");
    assert!(error["hint"]
        .as_str()
        .is_some_and(|h| h.contains("--store file")));

    let logout = sandbox
        .run(&["logout", "--provider", "hetzner"], &env)
        .ok()
        .json();
    assert_eq!(logout["removed"], serde_json::json!(["file"]));
}

// ------------------------------------------------------- resolution order

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_variables_take_precedence_over_a_stored_credential() {
    let sandbox = Sandbox::new().await;
    sandbox
        .run_with_stdin(
            &["login", "--provider", "vultr", "--stdin"],
            &[],
            STORED_SECRET,
        )
        .ok();

    // Stored credential alone: accepted.
    let stored = sandbox.api("vultr", &["verify"], &[]).ok().json();
    assert_eq!(stored["credential_source"], "file");
    assert_eq!(stored["authenticated"], true);

    // The environment wins — shown by it being the one the mock rejects.
    let from_env = sandbox
        .api("vultr", &["verify"], &[("VULTR_API_KEY", "bad-token")])
        .failed();
    let report = from_env.json();
    assert_eq!(report["credential_source"], "environment");
    assert_eq!(report["authenticated"], false);
    assert!(report["hint"]
        .as_str()
        .is_some_and(|h| h.contains("environment variables")));

    // An empty variable is not a credential: the stored one is used.
    let empty = sandbox
        .api("vultr", &["verify"], &[("VULTR_API_KEY", "")])
        .ok()
        .json();
    assert_eq!(empty["credential_source"], "file");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_only_use_is_unchanged() {
    let sandbox = Sandbox::new().await;
    let env = [("HCLOUD_TOKEN", ENV_SECRET)];

    let regions = sandbox.api("hetzner", &["regions"], &env).ok();
    assert_eq!(
        regions.json(),
        serde_json::json!([
            { "id": "fsn1", "name": "Falkenstein", "country": "DE" },
            { "id": "nbg1", "name": "Nuremberg", "country": "DE" },
        ])
    );
    assert!(
        regions.stdout.starts_with("[\n  {\n    \"id\""),
        "pretty-printed as before"
    );

    let sizes = sandbox.api("hetzner", &["sizes", "fsn1"], &env).ok().json();
    assert_eq!(sizes[0]["id"], "cx22");
    assert_eq!(sizes[0]["monthly_price"], 4.35);
    assert_eq!(sizes[0]["currency"], "EUR");

    let key = sandbox.write("id.pub", SSH_KEY);
    let created = sandbox
        .api(
            "hetzner",
            &[
                "instance",
                "create",
                "web-1",
                "fsn1",
                "--type",
                "cx22",
                "--image",
                "ubuntu-24.04",
                "--ssh-key",
                &key,
            ],
            &env,
        )
        .ok()
        .json();
    for field in [
        "id",
        "name",
        "region",
        "instance_type",
        "status",
        "public_ipv4",
        "private_ipv4",
        "ssh_user",
        "ssh_port",
    ] {
        assert!(created.get(field).is_some(), "instance JSON lost `{field}`");
    }
    let id = created["id"].as_str().expect("id");
    assert!(
        sandbox
            .mock_get(&format!("/_mock/hetzner/instances/{id}/user-data"))
            .await
            .is_none(),
        "no user data unless asked for"
    );

    let deleted = sandbox
        .api("hetzner", &["instance", "delete", id], &env)
        .ok();
    assert_eq!(
        deleted.stdout,
        format!("deleted {id}\n"),
        "plain line, as before"
    );

    // NIMBUS_PROVIDER / NIMBUS_BASE_URL still stand in for the flags, and
    // the `do` alias still selects DigitalOcean.
    let base = sandbox.base_url("hetzner");
    let via_env = sandbox
        .run(
            &["regions"],
            &[
                ("NIMBUS_PROVIDER", "hetzner"),
                ("NIMBUS_BASE_URL", &base),
                ("HCLOUD_TOKEN", ENV_SECRET),
            ],
        )
        .ok()
        .json();
    assert_eq!(via_env[0]["id"], "fsn1");
    let digitalocean = sandbox.base_url("digitalocean");
    let alias = sandbox
        .run(
            &["--provider", "do", "--base-url", &digitalocean, "regions"],
            &[("DIGITALOCEAN_TOKEN", ENV_SECRET)],
        )
        .ok()
        .json();
    assert_eq!(alias[0]["id"], "nyc3");
}

// ------------------------------------------------------------------ verify

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn verify_reports_authenticated_rejected_and_unknown() {
    let sandbox = Sandbox::new().await;

    // Authenticated, and confirmed able to create.
    let yes = sandbox
        .api("vultr", &["verify"], &[("VULTR_API_KEY", ENV_SECRET)])
        .ok()
        .json();
    assert_eq!(yes["provider"], "vultr");
    assert_eq!(yes["authenticated"], true);
    assert_eq!(yes["can_create_instances"], "yes");
    assert_eq!(yes["checks"], serde_json::json!(["GET /account"]));
    assert!(yes["detail"].is_string() && yes["hint"].is_string());

    // Rejected: a report on stdout (not an error), and a failing status.
    let rejected = sandbox
        .api("vultr", &["verify"], &[("VULTR_API_KEY", "bad-token")])
        .failed();
    assert!(
        rejected.stderr.is_empty(),
        "the report is the output: {}",
        rejected.stderr
    );
    let report = rejected.json();
    assert_eq!(report["authenticated"], false);
    assert_eq!(report["can_create_instances"], "no");
    assert!(report["hint"]
        .as_str()
        .is_some_and(|h| h.contains("rejected")));

    // Authenticates but cannot create.
    let read_only = sandbox
        .api(
            "vultr",
            &["verify"],
            &[("VULTR_API_KEY", "read-only-token")],
        )
        .failed()
        .json();
    assert_eq!(read_only["authenticated"], true);
    assert_eq!(read_only["can_create_instances"], "no");
    assert!(read_only["detail"]
        .as_str()
        .is_some_and(|d| d.contains("provisioning")));

    // Unknown is reported as unknown, and is not a failure.
    for provider in ["hetzner", "digitalocean", "scaleway"] {
        let unknown = sandbox
            .api(provider, &["verify"], &env_for(provider, ENV_SECRET))
            .ok()
            .json();
        assert_eq!(unknown["authenticated"], true, "{provider}");
        assert_eq!(unknown["can_create_instances"], "unknown", "{provider}");
    }

    assert!(
        sandbox.writes().await.is_empty(),
        "verify must never write: {:?}",
        sandbox.writes().await
    );

    // The check itself failing is an error, not a verdict.
    let unreachable = sandbox
        .run(
            &[
                "--provider",
                "hetzner",
                "--base-url",
                "http://127.0.0.1:1/v1",
                "verify",
            ],
            &[("HCLOUD_TOKEN", ENV_SECRET)],
        )
        .failed();
    assert!(unreachable.stdout.is_empty());
    assert_eq!(unreachable.error()["kind"], "network_error");
}

// --------------------------------------------------------------- user data

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_data_file_reaches_every_provider_that_supports_it() {
    let sandbox = Sandbox::new().await;
    let key = sandbox.write("id.pub", SSH_KEY);
    let script = sandbox.write("first-boot.sh", SCRIPT);
    let providers = sandbox.run(&["providers"], &[]).ok().json();

    for provider in ALL {
        let (region, instance_type, image) = target(provider);
        let supported = find(&providers, provider)["caveats"]["supports_user_data"] == true;
        let out = sandbox.api(
            provider,
            &[
                "instance",
                "create",
                "boot-test",
                region,
                "--type",
                instance_type,
                "--image",
                image,
                "--ssh-key",
                &key,
                "--user-data-file",
                &script,
            ],
            &env_for(provider, ENV_SECRET),
        );
        if supported {
            let instance = out.ok().json();
            let id = instance["id"].as_str().expect("id");
            let record = sandbox
                .mock_get(&format!("/_mock/{provider}/instances/{id}/user-data"))
                .await
                .unwrap_or_else(|| panic!("{provider}: the script never reached the provider"));
            assert_eq!(
                record["script"], SCRIPT,
                "{provider}: script arrived altered"
            );
        } else {
            // Refused up front, naming the provider — never silently dropped.
            let error = out.failed().error();
            assert_eq!(error["kind"], "unsupported_request", "{provider}");
            assert_eq!(error["provider"], provider);
            let message = error["message"].as_str().expect("message");
            assert!(
                message.contains(provider) && message.contains("user_data"),
                "{message}"
            );
            let listed = sandbox
                .api(
                    provider,
                    &["instance", "list"],
                    &env_for(provider, ENV_SECRET),
                )
                .ok()
                .json();
            assert_eq!(
                listed,
                serde_json::json!([]),
                "{provider}: nothing was created"
            );
        }
    }
    assert_eq!(
        find(&providers, "hostinger")["caveats"]["supports_user_data"],
        false,
        "the refused case above must actually have been exercised"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unusable_user_data_file_fails_before_anything_is_sent() {
    let sandbox = Sandbox::new().await;
    let key = sandbox.write("id.pub", SSH_KEY);
    let empty = sandbox.write("empty.sh", "  \n");
    let missing = sandbox.dir.path().join("nope.sh").display().to_string();
    for (path, kind) in [(missing, "unreadable_file"), (empty, "invalid_input")] {
        let error = sandbox
            .api(
                "hetzner",
                &[
                    "instance",
                    "create",
                    "x",
                    "fsn1",
                    "--type",
                    "cx22",
                    "--image",
                    "ubuntu-24.04",
                    "--ssh-key",
                    &key,
                    "--user-data-file",
                    &path,
                ],
                &[("HCLOUD_TOKEN", ENV_SECRET)],
            )
            .failed()
            .error();
        assert_eq!(error["kind"], kind);
        assert!(error["hint"]
            .as_str()
            .is_some_and(|h| h.contains("--user-data-file")));
    }
    assert!(
        sandbox.writes().await.is_empty(),
        "no SSH key or server may be created when the script cannot be read"
    );
}

// ------------------------------------------------------------------ errors

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failures_are_json_and_say_what_to_do_next() {
    let sandbox = Sandbox::new().await;
    let env = [("HCLOUD_TOKEN", ENV_SECRET)];

    let no_provider = sandbox.run(&["regions"], &[]).failed().error();
    assert_eq!(no_provider["kind"], "missing_provider");

    let unknown = sandbox
        .run(&["--provider", "aws", "regions"], &[])
        .failed()
        .error();
    assert_eq!(unknown["kind"], "unknown_provider");
    assert!(unknown["hint"]
        .as_str()
        .is_some_and(|h| h.contains("nimbus providers")));

    let rejected = sandbox
        .api("hetzner", &["regions"], &[("HCLOUD_TOKEN", "bad-token")])
        .failed()
        .error();
    assert_eq!(rejected["kind"], "authentication_rejected");
    assert!(rejected["hint"]
        .as_str()
        .is_some_and(|h| h.contains("nimbus login --provider hetzner")));

    let not_found = sandbox
        .api("hetzner", &["instance", "get", "424242"], &env)
        .failed()
        .error();
    assert_eq!(not_found["kind"], "provider_error");

    let key = sandbox.write("id.pub", SSH_KEY);
    let invalid = sandbox
        .api(
            "hetzner",
            &[
                "instance",
                "create",
                "x",
                "fsn1",
                "--type",
                "no-such-type",
                "--image",
                "ubuntu-24.04",
                "--ssh-key",
                &key,
            ],
            &env,
        )
        .failed()
        .error();
    assert_eq!(invalid["kind"], "provider_error");
    assert!(invalid["hint"]
        .as_str()
        .is_some_and(|h| h.contains("sizes <region>")));

    let unsupported = sandbox
        .api(
            "hostinger",
            &["instance", "delete", "17001"],
            &[("HOSTINGER_API_TOKEN", ENV_SECRET)],
        )
        .failed()
        .error();
    assert_eq!(unsupported["kind"], "unsupported_request");

    let bad_store = sandbox
        .run(&["providers"], &[("NIMBUS_CREDENTIAL_STORE", "keychain")])
        .failed()
        .error();
    assert_eq!(bad_store["kind"], "invalid_configuration");
}

// ----------------------------------------------------------- secret hygiene

/// Every window of this many characters of a secret must be absent from
/// output — "in full or in part".
const WINDOW: usize = 8;

fn assert_no_secret(what: &str, out: &Ran) {
    for secret in [ENV_SECRET, STORED_SECRET, "fake-app-secret-value"] {
        let chars: Vec<char> = secret.chars().collect();
        for window in chars.windows(WINDOW) {
            let fragment: String = window.iter().collect();
            // The shared "fake-" prefix of the test values is not a secret.
            if fragment.starts_with("fake-") || "fake-".contains(&fragment) {
                continue;
            }
            for (stream, text) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
                assert!(
                    !text.contains(&fragment),
                    "{what}: {stream} contains part of a secret ({fragment}): {text}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_command_prints_a_secret() {
    let sandbox = Sandbox::new().await;
    let key = sandbox.write("id.pub", SSH_KEY);
    let script = sandbox.write("first-boot.sh", SCRIPT);
    let ovh_file = sandbox.write(
        "ovh.env",
        &format!(
            "OVH_APPLICATION_KEY=fake-app-key\nOVH_APPLICATION_SECRET=fake-app-secret-value\nOVH_CONSUMER_KEY={STORED_SECRET}\nOVH_PROJECT_ID=test-project\n"
        ),
    );
    let mut checked = 0;
    let mut check = |what: &str, out: Ran| {
        assert_no_secret(what, &out);
        checked += 1;
        out
    };

    // Storing, listing and removing credentials.
    check(
        "login --stdin",
        sandbox.run_with_stdin(
            &["login", "--provider", "hetzner", "--stdin"],
            &[],
            STORED_SECRET,
        ),
    );
    check(
        "login --from-file",
        sandbox.run(
            &["login", "--provider", "ovh", "--from-file", &ovh_file],
            &[],
        ),
    );
    check(
        "login with env set",
        sandbox.run_with_stdin(
            &["login", "--provider", "vultr", "--stdin"],
            &[("VULTR_API_KEY", ENV_SECRET)],
            STORED_SECRET,
        ),
    );
    check(
        "login bad input",
        sandbox.run_with_stdin(
            &["login", "--provider", "ovh", "--stdin"],
            &[],
            &format!("{STORED_SECRET}\nnot the expected format\n"),
        ),
    );
    check(
        "providers",
        sandbox.run(&["providers"], &[("LINODE_TOKEN", ENV_SECRET)]),
    );

    // Every provider command, with the credential from the environment and
    // from the store, succeeding and failing.
    for provider in ALL {
        let env = env_for(provider, ENV_SECRET);
        let (region, instance_type, image) = target(provider);
        for args in [
            vec!["verify"],
            vec!["regions"],
            vec!["sizes", region],
            vec!["images", region],
            vec!["instance", "list"],
            vec!["instance", "get", "424242"],
            vec!["instance", "delete", "424242"],
            vec!["volume", "list"],
            vec!["network", "list"],
            vec![
                "instance",
                "create",
                "leak-test",
                region,
                "--type",
                instance_type,
                "--image",
                image,
                "--ssh-key",
                &key,
                "--user-data-file",
                &script,
            ],
            vec![
                "instance",
                "create",
                "leak-test",
                region,
                "--type",
                "no-such-type",
                "--image",
                image,
                "--ssh-key",
                &key,
            ],
        ] {
            check(
                &format!("{provider} {args:?}"),
                sandbox.api(provider, &args, &env),
            );
        }
        // Unreachable API: the transport error must not carry the secret.
        check(
            &format!("{provider} unreachable"),
            sandbox.run(
                &[
                    "--provider",
                    provider,
                    "--base-url",
                    "http://127.0.0.1:1",
                    "verify",
                ],
                &env,
            ),
        );
    }
    for provider in ["hetzner", "ovh"] {
        for args in [
            vec!["verify"],
            vec!["regions"],
            vec!["instance", "get", "424242"],
        ] {
            check(
                &format!("{provider} stored {args:?}"),
                sandbox.api(provider, &args, &[]),
            );
        }
    }
    check(
        "logout",
        sandbox.run(
            &["logout", "--provider", "ovh"],
            &[("OVH_CONSUMER_KEY", ENV_SECRET)],
        ),
    );
    check(
        "logout again",
        sandbox.run(&["logout", "--provider", "ovh"], &[]),
    );
    assert!(checked > 80, "the sweep ran ({checked} commands)");
}

/// The backstop: even if a provider echoes the credential back in an error
/// body, it is blanked before the error is printed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_secret_echoed_back_by_the_provider_is_redacted() {
    use axum::{http::HeaderMap, http::StatusCode, Router};

    async fn echo(headers: HeaderMap) -> (StatusCode, String) {
        let sent = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\": \"could not process credential {sent}\"}}"),
        )
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let echo_base = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(echo))
            .await
            .expect("echo server");
    });

    let sandbox = Sandbox::new().await;
    for (args, env) in [
        (vec!["regions"], vec![("HCLOUD_TOKEN", ENV_SECRET)]),
        (vec!["verify"], vec![("HCLOUD_TOKEN", ENV_SECRET)]),
    ] {
        let mut all = vec!["--provider", "hetzner", "--base-url", &echo_base];
        all.extend(args);
        let out = sandbox.run(&all, &env).failed();
        assert_no_secret("echoed", &out);
        let error = out.error();
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|m| m.contains("[redacted]")),
            "the echoed value should be visibly blanked: {error}"
        );
    }

    // Same for a credential that came from the store.
    sandbox
        .run_with_stdin(
            &["login", "--provider", "hetzner", "--stdin"],
            &[],
            STORED_SECRET,
        )
        .ok();
    let out = sandbox
        .run(
            &["--provider", "hetzner", "--base-url", &echo_base, "regions"],
            &[],
        )
        .failed();
    assert_no_secret("echoed (stored)", &out);
    assert!(out.stderr.contains("[redacted]"));
}

// -------------------------------------------------------------------- misc

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn images_lists_what_instance_create_accepts() {
    let sandbox = Sandbox::new().await;
    let images = sandbox
        .api(
            "hostinger",
            &["images", "19"],
            &[("HOSTINGER_API_TOKEN", ENV_SECRET)],
        )
        .ok()
        .json();
    assert_eq!(
        images[0],
        serde_json::json!({ "id": "1130", "name": "Ubuntu 24.04 LTS" })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn version_flag_reports_the_package_version() {
    let sandbox = Sandbox::new().await;
    let out = sandbox.run(&["--version"], &[]).ok();
    assert_eq!(
        out.stdout,
        format!("nimbus {}\n", env!("CARGO_PKG_VERSION"))
    );
}
