<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="nimbus" width="360">
  </picture>
</p>

[![CI](https://github.com/gotempsh/nimbus/actions/workflows/ci.yml/badge.svg)](https://github.com/gotempsh/nimbus/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)

One async trait for instance, block-storage, and network provisioning across
cloud providers — Hetzner, Vultr, OVHcloud, DigitalOcean, Scaleway, Linode,
and Hostinger today. Same call, same types, different backend.

```rust
use nimbus_cloud::{CloudProvider, CreateInstance, providers::Hetzner};

let provider = Hetzner::new(std::env::var("HCLOUD_TOKEN")?);

let regions = provider.regions().await?;
let sizes = provider.instance_types(&regions[0].id).await?;

let instance = provider
    .create_instance(CreateInstance {
        name: "web-1".into(),
        region: regions[0].id.clone(),
        instance_type: sizes[0].id.clone(),
        image: "ubuntu-24.04".into(),
        ssh_public_key: std::fs::read_to_string("~/.ssh/id_ed25519.pub")?,
        network_id: None,
        user_data: None,
    })
    .await?;
```

Swap `Hetzner` for `Vultr` or `Ovh` and the rest of the call is unchanged —
that's the point.

## What it covers

- **Discovery** — regions, instance types (with monthly price)
- **Instances** — create, get, list, delete
- **Storage** — block volumes: create, list, attach, detach, delete
- **Networks** — private networks/VPCs: create, list, delete

Every call is a plain REST request over `reqwest`; there's no external
runtime dependency (no Terraform/Pulumi binary, no state file).

## CLI

`nimbus` is built to be driven by a program — a coding agent, a script — as
readily as by a person: results are JSON on stdout, failures are JSON on
stderr with a `hint` saying what to do next, and the provider credential
never has to pass through whatever is driving it.

### Install

Each release has a prebuilt binary for Linux (x86_64, aarch64 — statically
linked, so any distribution) and macOS (Intel, Apple silicon), plus a
`SHA256SUMS` file:

```bash
# x86_64-unknown-linux-musl | aarch64-unknown-linux-musl
# x86_64-apple-darwin       | aarch64-apple-darwin
target=aarch64-apple-darwin
base=https://github.com/gotempsh/nimbus/releases/latest/download

curl -fsSLO "$base/nimbus-$target.tar.gz"
curl -fsSLO "$base/SHA256SUMS"

# Verify before unpacking. This must print "nimbus-<target>.tar.gz: OK";
# anything else (FAILED, "no file was verified") means do not use the file.
sha256sum --check --ignore-missing SHA256SUMS       # Linux
shasum -a 256 --check --ignore-missing SHA256SUMS   # macOS

tar -xzf "nimbus-$target.tar.gz" nimbus
install -m 0755 nimbus ~/.local/bin/nimbus          # or any directory on PATH
nimbus --version
```

Every release is listed on the
[releases page](https://github.com/gotempsh/nimbus/releases). To build from
source instead (needs a Rust toolchain):

```bash
git clone https://github.com/gotempsh/nimbus && cd nimbus
cargo build --release --locked -p nimbus    # -> target/release/nimbus
```

Releases are cut by `.github/workflows/release.yml`: set `version` in the
workspace `Cargo.toml`, merge, then push a new tag `v<version>` on that
commit. A tag that does not match the version fails the build instead of
publishing. The same workflow builds every target (without publishing) on
pull requests that touch the CLI.

### Quick start

```bash
nimbus providers                      # what is supported, what is configured
nimbus login --provider hetzner       # type the token at a hidden prompt
nimbus --provider hetzner verify      # does it work? can it create servers?
nimbus --provider hetzner regions
nimbus --provider hetzner sizes fsn1
nimbus --provider hetzner images fsn1
nimbus --provider hetzner instance create web-1 fsn1 \
  --type cx22 --image ubuntu-24.04 --ssh-key ~/.ssh/id_ed25519.pub \
  --user-data-file ./first-boot.sh
```

Provider selection is `--provider hetzner|vultr|ovh|digitalocean|scaleway|linode|hostinger`
(or the `NIMBUS_PROVIDER` env var; `do` is accepted for `digitalocean`).

### Credentials

`nimbus login --provider <id>` asks for each value the provider needs —
with echo off for the secret ones — and stores them; `nimbus logout
--provider <id>` removes them again.

```console
$ nimbus login --provider hetzner
Hetzner Cloud API token (HCLOUD_TOKEN) (input hidden):
{
  "provider": "hetzner",
  "stored": true,
  "store": "credential_store",
  "location": "macOS Keychain (service \"nimbus-cli\")",
  "fields": ["HCLOUD_TOKEN"],
  "next": "nimbus --provider hetzner verify"
}
```

**Where they are kept.** The operating system's credential store: the
Keychain on macOS, the Secret Service (GNOME Keyring, KWallet, …) on Linux.
On a machine that has none — a server, a container — `login` falls back to
`credentials.json` in nimbus's config directory (`~/.config/nimbus`, or
`$XDG_CONFIG_HOME/nimbus`), readable and writable by its owner only (mode
`0600`, in a `0700` directory); a file with wider permissions is refused
rather than read. The output always says which was used (`store`,
`location`) and, when the file was a fallback, why (`fallback_reason`).

| Setting | Effect |
| --- | --- |
| `--store auto\|os\|file` (on `login`) | Where to store. `auto` (default) is the OS store with the file as fallback; `os` fails instead of falling back; `file` never touches the OS store. |
| `NIMBUS_CREDENTIAL_STORE=auto\|os\|file` | The same choice for every command, including which stores are read. |
| `NIMBUS_CONFIG_DIR=<dir>` | Where the credentials file lives. |

**Without a terminal.** `--from-file <path>` or `--stdin` read the
credential instead of prompting: either the value alone (providers with a
single value), or one `NAME=value` line per field using the env var names
below.

```bash
nimbus login --provider hetzner --stdin < /run/secrets/hcloud-token
nimbus login --provider ovh --from-file ./ovh.env
```

There is deliberately no flag that takes a secret as its value: command
lines end up in shell history, process listings and agent transcripts.

**Resolution order.** For each command, the first of these that has the
provider's credentials wins:

1. the provider's environment variables (as before — existing setups keep
   working unchanged);
2. the OS credential store;
3. the credentials file.

| Provider | Values (env var names) | Secret |
| --- | --- | --- |
| Hetzner | `HCLOUD_TOKEN` | yes |
| Vultr | `VULTR_API_KEY` | yes |
| OVH | `OVH_APPLICATION_KEY`, `OVH_PROJECT_ID` | no |
| | `OVH_APPLICATION_SECRET`, `OVH_CONSUMER_KEY` | yes |
| DigitalOcean | `DIGITALOCEAN_TOKEN` | yes |
| Scaleway | `SCW_SECRET_KEY` | yes |
| | `SCW_DEFAULT_PROJECT_ID`, `SCW_DEFAULT_ZONE` (optional, default `fr-par-1`) | no |
| Linode | `LINODE_TOKEN` | yes |
| Hostinger | `HOSTINGER_API_TOKEN` | yes |

**What is never printed.** No command prints a credential, whole or in
part — not `providers`, not `verify`, not an error. Besides never putting
one in a message in the first place, everything the CLI writes passes
through a filter that blanks the credential values in use, so a provider
that echoes a token back in an error body cannot leak it either.

When nothing is configured, the error says exactly what to run:

```console
$ nimbus --provider linode regions
{
  "error": {
    "kind": "missing_credentials",
    "provider": "linode",
    "message": "no credentials found for Linode (Akamai) (linode)",
    "hint": "Run `nimbus login --provider linode` in a terminal to store them (the secret is typed with echo off and never shown), or set LINODE_TOKEN."
  }
}
```

### `nimbus providers`

Lists every provider as JSON and needs no credentials: its id and display
name, the credential fields it takes (`name`, `env`, `secret`, `required`),
whether credentials are currently available and from which source
(`environment`, `credential_store` or `file` — never the values), the exact
`login_command`, and its caveats:

| Caveat | Meaning |
| --- | --- |
| `can_delete_instances` | `false`: there is no API to delete a server (Hostinger). |
| `reports_prices` | `false`: `sizes` reports `monthly_price` as `0.0` (OVH). |
| `reports_instance_specs` | `false`: `sizes` reports `vcpus`/`memory_gb`/`disk_gb` as `0` (no provider currently; Hostinger's come from a built-in table of its plan tiers, see its `notes`). |
| `create_is_irreversible_purchase` | `true`: `instance create` buys a subscription that cannot be undone through the API (Hostinger). |
| `supports_user_data` | `false`: `--user-data-file` is refused (Hostinger). |
| `notes` | Anything else worth knowing before using the provider, in plain sentences. |

The same data is available to SDK users as `nimbus_cloud::PROVIDERS` and
`CloudProvider::info()`.

### `nimbus verify`

Checks the credential without creating, changing or deleting anything — it
only issues `GET` requests — and reports:

```console
$ nimbus --provider vultr verify
{
  "provider": "vultr",
  "credential_source": "credential_store",
  "authenticated": true,
  "can_create_instances": "yes",
  "checks": ["GET /account"],
  "detail": "The API key is valid and its user has the permission to deploy new servers (`provisioning`).",
  "hint": "The credential is ready to create instances."
}
```

`can_create_instances` is `"yes"`, `"no"` or `"unknown"`. It is `unknown`
whenever the provider offers no way to find out short of creating a server —
that is said plainly rather than guessed. The exit status is `0` unless the
credential is rejected or is known not to be able to create instances, in
which case it is `1` (the JSON report is still on stdout). If the check
itself cannot run — no network, the provider is down — that is an error on
stderr, not a verdict.

| Provider | What is called | What it can establish |
| --- | --- | --- |
| Hetzner | `GET /servers` | Authentication only. Tokens are read-only or read-write and no endpoint says which → always `unknown`. |
| Vultr | `GET /account` | `yes`/`no` from the key's ACLs (`provisioning`, or `manage_users`). |
| OVH | `GET /auth/currentCredential`, `GET /cloud/project/{id}` | `yes`/`no` from the consumer key's status and access rules (must allow `POST` on the project's `sshkey` and `instance` paths) and the project's status. `unknown` if the key may not read its own rules. |
| DigitalOcean | `GET /account` | `no` if the account is locked or its Droplet limit is 0; otherwise `unknown` — a token's scopes cannot be read back. |
| Scaleway | `GET /instance/v1/zones/{zone}/servers` | `no` if the key may not even list Instances in the project; otherwise `unknown` — IAM permissions cannot be read with the secret key alone. |
| Linode | `GET /profile`, `GET /profile/grants` | `yes`/`no` from the token's scopes (`linodes:read_write` or `*`) and, for restricted users, the `add_linodes` grant. `unknown` if the scopes are not reported. |
| Hostinger | `GET /vps/v1/virtual-machines`, `GET /billing/v1/payment-methods` | `yes`/`no` from whether the account has a usable default payment method (tokens carry all of their user's permissions). The charge itself can still be declined at purchase. |

SDK users get the same through `CloudProvider::check_credential()`.

### First-boot script

`instance create --user-data-file <path>` sends the file's contents as the
instance's cloud-init user data, to run on first boot. It is a file rather
than a value so that a multi-line script — or a single-use join code inside
it — stays out of shell history. The file is read before anything is sent,
so a wrong path cannot leave a half-created instance behind.

| Provider | How the script is delivered |
| --- | --- |
| Hetzner | `user_data` in the create request, as text |
| Vultr | `user_data` in the create request, base64-encoded |
| OVH | `userData` in the create request, as text |
| DigitalOcean | `user_data` in the create request, as text |
| Scaleway | A separate `PATCH …/servers/{id}/user_data/cloud-init` (the create request has no such field), sent while the server is still stopped so it is in place for the first boot |
| Linode | `metadata.user_data` in the create request, base64-encoded |
| Hostinger | **Not supported** — the purchase API accepts no user data. The request is refused before anything is bought, with an error naming the provider. |

### Output and errors

- Results are pretty-printed JSON on stdout. The `delete`, `attach` and
  `detach` commands print a one-line confirmation (`deleted <id>`), as they
  always have.
- Failures are JSON on stderr, with exit status `1`:
  `{"error": {"kind", "provider", "message", "hint"}}`. `kind` is stable and
  meant to be matched on — `missing_credentials`, `authentication_rejected`,
  `provider_error`, `network_error`, `unsupported_request`, `not_found`,
  `invalid_input`, `unreadable_file`, `no_terminal`,
  `credential_store_failed`, … — and `hint` is the next thing to do.
- A command line that cannot be parsed is `kind: "usage"` with exit status
  `2`. `--help` and `--version` print text.

## Layout

- `lib/` — `nimbus-cloud` crate: the `CloudProvider` trait, shared types, and
  the provider adapters (`lib/src/providers/`)
- `cli/` — `nimbus` binary: thin CLI over the trait, plus credential
  storage (`login`/`logout`)
- `mock/` — `nimbus-mock`: in-memory mock servers for all providers,
  for offline testing without real credentials or spend

## Testing without real cloud accounts

There's no LocalStack for these providers, so `mock/` is a small
purpose-built one: an Axum server that fakes just the endpoints the
adapters call (create/get/list/delete instances, volumes, networks;
region/size discovery), in-memory, on an ephemeral port.

```bash
# standalone, for manual poking (e.g. with curl or the CLI)
cargo run -p nimbus-mock
# -> Hetzner :8090/v1 · Vultr :8090/v2 · OVH :8090/1.0 · DO :8090/do/v2
#    Linode :8090/v4 · Scaleway :8090 (full /instance/v1/... paths)
#    Hostinger :8090 (full /api/vps/v1/... and /api/billing/v1/... paths)

HCLOUD_TOKEN=anything cargo run -p nimbus -- \
  --provider hetzner --base-url http://127.0.0.1:8090/v1 regions
```

In tests, spawn it in-process and point an adapter's `with_base_url` at it:

```rust
let base = nimbus_mock::spawn().await;
let provider = Hetzner::new("mock-token").with_base_url(format!("{base}/v1"));
```

`lib/tests/mock_providers.rs` runs the full instance → volume → network
create/attach/list/delete flow against the mock for every provider except
Hostinger, which has its own test covering what it actually supports (see
Status below) and asserting the rest fails explicitly.
`lib/tests/mock_providers_unhappy.rs` covers the failure paths: rejected
credentials (any missing token or the sentinel `bad-token` → 401 →
`Error::Auth`), unknown resource ids (404), create-time validation errors
(each provider's real error status and body shape, surfaced in
`Error::Api`), and unreachable hosts (`Error::Transport`).
`lib/tests/user_data.rs` asserts the first-boot script reaches each provider
in the form its API expects, `lib/tests/credential_check.rs` covers
`check_credential` (working, rejected, cannot-create and cannot-tell
credentials), and `cli/tests/cli.rs` runs the real `nimbus` binary against
the mock — including a sweep asserting no command ever prints a credential.

Two things make those assertions possible. The mock records what it was
sent:

```bash
curl http://127.0.0.1:8090/_mock/hetzner/instances/<id>/user-data  # the script that arrived, decoded
curl http://127.0.0.1:8090/_mock/requests                          # every method + path served so far
```

and, besides `bad-token`, it recognises a few credentials that authenticate
but cannot create: `read-only-token` (Vultr, OVH consumer key, DigitalOcean,
Linode, Hostinger), `restricted-token` (Linode), `no-access-token`
(Scaleway), and the OVH project ids `missing-project` and
`suspended-project`.

`cargo test --workspace` runs everything. The mock is a stand-in for each
provider's API shape, not a faithful emulation (fixed catalogs, no
pagination/rate limits, instant state transitions) — good enough to catch
adapter bugs, not a substitute for testing against a real account before
depending on this in production.

## Status

Early. All seven adapters are complete for instance/volume/network CRUD
against their documented REST APIs, with a few gaps: OVH does not yet
resolve flavor pricing (`monthly_price` is `0.0` pending a price-catalog
integration), and Linode does not yet support attaching a VPC at instance
create time (returns an explicit error rather than silently ignoring it).
Scaleway's Instance API is zone-scoped, so a `Scaleway` client is bound to
one zone at construction. None of the adapters have been exercised against
live provider accounts yet — treat as unverified until that happens. That
includes `verify`: which call proves what was worked out from each
provider's API documentation and is tested against the mock only.

The CLI can be installed from a release binary,
stores credentials outside the environment (`login`/`logout`), checks them
(`verify`), describes every provider and its caveats as data (`providers`),
and passes a first-boot script (`--user-data-file`) to every provider except
Hostinger. Credential storage in the macOS Keychain and the Linux Secret
Service goes through the `keyring` crate and is covered by tests only up to
that boundary (the tests use an in-memory store, never a real keychain); the
owner-only file fallback is tested end to end.

Hostinger is structurally different from the rest and worth reading closely
before using it: `create_instance` purchases a real billing subscription
(there is no metered pay-as-you-go instance API), there is no API endpoint
to delete a VPS at all (only cancelling the subscription from the billing
dashboard), there is no block-storage or private-networking product, and
plan specs (`vcpus`/`memory_gb`/`disk_gb`) aren't exposed by the catalog
ahead of purchase, so they come from a table of Hostinger's published plan
tiers built into the adapter (`0` for a plan it does not know).
`create_volume`/`create_network`/
`delete_instance` return an explicit `Error::InvalidRequest` rather than
pretending to support them, as does `create_instance` when given
`user_data` (the purchase API has no field for it); the corresponding list
calls return an empty list, since that's simply true. `nimbus providers`
reports all of this as data. See the doc comment at the top of
`lib/src/providers/hostinger.rs` for the full rundown.

## Adding a provider

Implement `CloudProvider` in `lib/src/providers/<name>.rs` (including
`check_credential`), describe it in `lib/src/catalog.rs` — credential
fields and caveats — and register it in the CLI's `build_provider`. Nothing provider-specific should leak past
the trait — no provider-specific enum variants or fields on the shared
`Instance`/`Volume`/`Network` types. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Naming note

The library crate is `nimbus-cloud`; the CLI binary is `nimbus`. The bare
`nimbus` name on crates.io belongs to an unrelated crate, so if/when this is
published to crates.io the CLI package will need a distinct name
(`nimbus-cloud-cli` or similar).

## License

Apache-2.0 — see [LICENSE](LICENSE).
