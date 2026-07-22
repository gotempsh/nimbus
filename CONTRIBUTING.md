# Contributing to nimbus

Thanks for your interest in contributing!

## Development setup

```bash
# clone and enter the repo, then:
cargo build --workspace
```

No external services or credentials are needed for development — the test
suite runs entirely against `nimbus-mock` (see `mock/`), an in-memory fake
of the Hetzner/Vultr/OVH APIs.

## Before opening a PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must pass — CI enforces the same commands.

## Making changes

1. Fork the repo and create a branch off `main`.
2. Keep PRs focused: one logical change per PR.
3. Add or update tests for behavior you change. Provider adapter changes
   should extend `lib/tests/mock_providers.rs` and, if the API surface
   changes, the corresponding mock routes in `mock/src/`.
4. Write a clear PR description: what changed and why, not just what.
5. Link any related issue.

## Adding a provider

Implement `CloudProvider` in `lib/src/providers/<name>.rs`, add a mock in
`mock/src/<name>.rs`, register both, wire the CLI's `build_provider`, and
add a full-flow test. Nothing provider-specific may leak past the trait —
no provider-specific fields on the shared `Instance`/`Volume`/`Network`
types.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/) where
practical (`feat:`, `fix:`, `docs:`, `chore:`, ...).

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind.

## Questions

Open a [Discussion](../../discussions) or an issue tagged `question`.
