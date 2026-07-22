# nimbus

One async trait for instance, block-storage, and network provisioning across
cloud providers — Hetzner, Vultr, and OVHcloud today. Same call, same types,
different backend.

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

Provider selection is `--provider hetzner|vultr|ovh` (or `NIMBUS_PROVIDER`
env var); credentials come from provider-specific env vars:

| Provider | Env vars |
| --- | --- |
| Hetzner | `HCLOUD_TOKEN` |
| Vultr | `VULTR_API_KEY` |
| OVH | `OVH_APPLICATION_KEY`, `OVH_APPLICATION_SECRET`, `OVH_CONSUMER_KEY`, `OVH_PROJECT_ID` |

## Layout

- `lib/` — `nimbus-cloud` crate: the `CloudProvider` trait, shared types, and
  the three provider adapters (`lib/src/providers/`)
- `cli/` — `nimbus` binary: thin CLI over the trait
- `mock/` — `nimbus-mock`: in-memory mock servers for all three providers,
  for offline testing without real credentials or spend

## Testing without real cloud accounts

There's no LocalStack for Hetzner/Vultr/OVH, so `mock/` is a small
purpose-built one: an Axum server that fakes just the endpoints the
adapters call (create/get/list/delete instances, volumes, networks;
region/size discovery), in-memory, on an ephemeral port.

```bash
# standalone, for manual poking (e.g. with curl or the CLI)
cargo run -p nimbus-mock
# -> Hetzner on :8090/v1, Vultr on :8090/v2, OVH on :8090/1.0

HCLOUD_TOKEN=anything cargo run -p nimbus -- \
  --provider hetzner --base-url http://127.0.0.1:8090/v1 regions
```

In tests, spawn it in-process and point an adapter's `with_base_url` at it:

```rust
let base = nimbus_mock::spawn().await;
let provider = Hetzner::new("mock-token").with_base_url(format!("{base}/v1"));
```

`lib/tests/mock_providers.rs` runs the full instance → volume → network
create/attach/list/delete flow against the mock for all three providers —
`cargo test --workspace` to run it. It's a stand-in for each provider's API
shape, not a faithful emulation (no auth checks, no realistic error cases,
fixed region/size catalogs) — good enough to catch adapter bugs, not a
substitute for testing against a real account before depending on this in
production.

## Status

Early. Hetzner and Vultr adapters are complete against their documented
REST APIs. The OVH adapter is complete for instance/volume/network CRUD but
does not yet resolve flavor pricing (`instance_types().monthly_usd` is `0.0`
for OVH pending a `/cloud/project/{id}/price` integration). None of the
adapters have been exercised against live provider accounts yet — treat as
unverified until that happens.

## Adding a provider

Implement `CloudProvider` in `lib/src/providers/<name>.rs` and register it
in the CLI's `build_provider`. Nothing provider-specific should leak past
the trait — no provider-specific enum variants or fields on the shared
`Instance`/`Volume`/`Network` types.

## License

Apache-2.0
