## Summary

<!-- What does this change and why? Link the issue if there is one. -->

## Checklist

- [ ] The PR targets `develop` (or is a release/hotfix PR into `main`)
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo deny check` passes (if dependencies changed)
- [ ] `CHANGELOG.md` updated under `## [Unreleased]` (if users are affected)
