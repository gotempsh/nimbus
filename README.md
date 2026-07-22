<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo-dark.svg">
    <img src="assets/logo.svg" alt="nimbus" width="360">
  </picture>
</p>

[![CI](https://github.com/gotempsh/nimbus/actions/workflows/ci.yml/badge.svg)](https://github.com/gotempsh/nimbus/actions/workflows/ci.yml)
![License](https://img.shields.io/badge/license-Apache--2.0-blue)

One async trait for instance, block-storage, and network provisioning across
cloud providers — Hetzner, Vultr, OVHcloud, DigitalOcean, Scaleway, and
Linode today. Same call, same types, different backend.

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

```bash
export HCLOUD_TOKEN=...
cargo run -p nimbus -- --provider hetzner regions
cargo run -p nimbus -- --provider hetzner sizes fsn1
cargo run -p nimbus -- --provider hetzner instance create web-1 fsn1 \
  --type cx22 --image ubuntu-24.04 --ssh-key ~/.ssh/id_ed25519.pub
```

Provider selection is `--provider hetzner|vultr|ovh|digitalocean|scaleway|linode` (or `NIMBUS_PROVIDER`
env var); credentials come from provider-specific env vars:

| Provider | Env vars |
| --- | --- |
| Hetzner | `HCLOUD_TOKEN` |
| Vultr | `VULTR_API_KEY` |
| OVH | `OVH_APPLICATION_KEY`, `OVH_APPLICATION_SECRET`, `OVH_CONSUMER_KEY`, `OVH_PROJECT_ID` |
| DigitalOcean | `DIGITALOCEAN_TOKEN` |
| Scaleway | `SCW_SECRET_KEY`, `SCW_DEFAULT_PROJECT_ID`, `SCW_DEFAULT_ZONE` (default `fr-par-1`) |
| Linode | `LINODE_TOKEN` |

## Layout

- `lib/` — `nimbus-cloud` crate: the `CloudProvider` trait, shared types, and
  the provider adapters (`lib/src/providers/`)
- `cli/` — `nimbus` binary: thin CLI over the trait
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

HCLOUD_TOKEN=anything cargo run -p nimbus -- \
  --provider hetzner --base-url http://127.0.0.1:8090/v1 regions
```

In tests, spawn it in-process and point an adapter's `with_base_url` at it:

```rust
let base = nimbus_mock::spawn().await;
let provider = Hetzner::new("mock-token").with_base_url(format!("{base}/v1"));
```

`lib/tests/mock_providers.rs` runs the full instance → volume → network
create/attach/list/delete flow against the mock for every provider —
`cargo test --workspace` to run it. It's a stand-in for each provider's API
shape, not a faithful emulation (no auth checks, no realistic error cases,
fixed region/size catalogs) — good enough to catch adapter bugs, not a
substitute for testing against a real account before depending on this in
production.

## Status

Early. All six adapters are complete for instance/volume/network CRUD
against their documented REST APIs, with two gaps: OVH does not yet resolve
flavor pricing (`monthly_price` is `0.0` pending a price-catalog
integration), and Linode does not yet support attaching a VPC at instance
create time (returns an explicit error rather than silently ignoring it).
Scaleway's Instance API is zone-scoped, so a `Scaleway` client is bound to
one zone at construction. None of the adapters have been exercised against
live provider accounts yet — treat as unverified until that happens.

## Adding a provider

Implement `CloudProvider` in `lib/src/providers/<name>.rs` and register it
in the CLI's `build_provider`. Nothing provider-specific should leak past
the trait — no provider-specific enum variants or fields on the shared
`Instance`/`Volume`/`Network` types. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Naming note

The library crate is `nimbus-cloud`; the CLI binary is `nimbus`. The bare
`nimbus` name on crates.io belongs to an unrelated crate, so if/when this is
published to crates.io the CLI package will need a distinct name
(`nimbus-cloud-cli` or similar).

## License

Apache-2.0 — see [LICENSE](LICENSE).
