# Contributing

Thanks for helping with VoiceChanger.

## Branches

- `develop` is the integration branch. Create `feature/<name>` or `fix/<name>`
  from `develop` and open the pull request into `develop`.
- `main` only receives release PRs from `develop` and `hotfix/*` PRs.
- Never push directly to `main`. It is protected and CI must pass before a
  PR can be merged. Direct pushes to `develop` are allowed; CI still runs on
  every push.

## Before you push

Run the same checks as CI:

    cargo build --release
    cargo test --workspace
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo deny check

CI runs these on `windows-latest` and fails on warnings.

## Changelog

If your change affects users, add a line under `## [Unreleased]` in
`CHANGELOG.md`. Don't bump the version: that happens only in the release PR
into `main` (see AGENTS.md "Releasing").

## Dependencies

New crates must pass `cargo deny check`. Widening the licence allow-list in
`deny.toml` needs a reason in the PR.

## Licence

By contributing, you agree that your contributions are licensed under the MIT
licence (see [LICENSE](LICENSE)).

## Code of conduct

This project follows the [Code of Conduct](CODE_OF_CONDUCT.md). By taking
part, you agree to uphold it.
