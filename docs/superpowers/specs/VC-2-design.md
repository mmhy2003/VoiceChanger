# VC-2 — M0: Project setup

## Goal
Turn the empty repository (only a 2-line `README.md` today) into a buildable Rust workspace. It gets open-source files, agent guides and a CI workflow on `windows-latest` that checks every push and PR to `develop` and `main`. You then protect both branches in GitHub settings.

## Decisions
- **Licence: MIT only.** You chose this over the `MIT OR Apache-2.0` the PRD proposed and over GPL-3.0. That means one `LICENSE` file (no `LICENSE-MIT`/`LICENSE-APACHE`), `license = "MIT"` in the workspace, and README/CONTRIBUTING saying contributions are MIT. This closes the PRD §17 licence question.
- **Product name: VoiceChanger.** You chose the current repo name. The binary is `voicechanger.exe` (package `voicechanger` in `crates/app`), and the app data folder will be `%APPDATA%\VoiceChanger` (used from M1 on). The installer `AppId` GUID is generated in M6.
- **Branch protection: you apply it by hand.** The PR description will list the exact settings (below). Rejected: the agent applying it with `gh api`, and a committed ruleset JSON.
- **Release guard on PRs into `main`: included now**, not deferred to M6.
- **Starting version `0.1.0`**, set in `[workspace.package]`. No tags or releases in M0; `1.0.0` is the M6 release.
- **`cargo-deny` is installed with `taiki-e/install-action`.** `EmbarkStudios/cargo-deny-action` is Docker-based and does not run on Windows runners.
- Rejected / not needed now: `release.yml`, installer smoke, base-model fetching, dev artifacts (all M6). Also `THIRD_PARTY_NOTICES` generation (M6).

## Architecture: files added (PR from a feature branch into `develop`)
```
Cargo.toml                 # [workspace] members = crates/*, resolver = "3"
                           # [workspace.package] version = "0.1.0", edition = "2024",
                           #   license = "MIT", repository, authors
                           # [workspace.lints] (clippy/rust baselines)
rust-toolchain.toml        # channel = "stable", components rustfmt + clippy
deny.toml                  # licences allow-list, advisories, bans, sources
.gitignore                 # /target
crates/
  app/      Cargo.toml (package voicechanger, bin), src/main.rs: empty fn main (eframe in M1)
  audio/    Cargo.toml (lib vc-audio),   src/lib.rs: crate doc comment only
  engine/   Cargo.toml (lib vc-engine),  src/lib.rs
  models/   Cargo.toml (lib vc-models),  src/lib.rs
  convert/  Cargo.toml (lib vc-convert), src/lib.rs
LICENSE                    # MIT, "Copyright (c) 2026 VoiceChanger contributors"
README.md                  # what it is, status (pre-alpha, M0), requirements, build commands, licence
CONTRIBUTING.md            # branch model (feature/* from develop → PR into develop), local checks, CHANGELOG rule, MIT
CODE_OF_CONDUCT.md         # Contributor Covenant 2.1; reports go to the maintainers through GitHub
SECURITY.md                # report privately via GitHub private vulnerability reporting; supported = latest release
CHANGELOG.md               # Keep a Changelog format, "## [Unreleased]" with the M0 entry
AGENTS.md                  # PRD §13 content verbatim
CLAUDE.md                  # PRD §13 content verbatim (imports @AGENTS.md)
.github/
  ISSUE_TEMPLATE/bug_report.yml, feature_request.yml, config.yml (blank issues off)
  PULL_REQUEST_TEMPLATE.md # summary, checklist: fmt/clippy/test run, CHANGELOG updated, targets develop
  workflows/ci.yml
```
The workspace members depend on nothing yet. Each crate gets `version.workspace = true`, `license.workspace = true` and `edition.workspace = true`, so the workspace `Cargo.toml` is the single version source (PRD §11).

## CI (`.github/workflows/ci.yml`)
- Triggers: `push` and `pull_request` on `develop` and `main`. `permissions: contents: read`. `concurrency` cancels earlier runs of the same ref.
- **Job `checks`** (`windows-latest`): checkout, `dtolnay/rust-toolchain@stable` (rustfmt, clippy), `Swatinem/rust-cache@v2`, then in order:
  1. `cargo fmt --all -- --check`
  2. `cargo clippy --all-targets --all-features -- -D warnings`
  3. `cargo test --workspace`
  4. `taiki-e/install-action` (cargo-deny), then `cargo deny check`
  5. `cargo build --release`
- **Job `release-guard`** (`windows-latest`, `shell: bash`, only `if: github.event_name == 'pull_request' && github.base_ref == 'main'`). It checks out with `fetch-depth: 0` and tags, then:
  - fails unless `github.head_ref` is `develop` or starts with `hotfix/`;
  - reads `version` from `[workspace.package]` in `Cargo.toml` and fails unless it is plain `X.Y.Z`;
  - finds the latest `vX.Y.Z` tag. With no tags yet, any valid version passes; otherwise the version must be strictly higher (`sort -V`);
  - fails unless `CHANGELOG.md` has a `## [X.Y.Z]` heading.

  Each failure prints one clear `::error::` line.

## Branch protection (checklist in the PR description, you apply it)
Rulesets (or classic protection) on `develop` and `main`: require a pull request before merging, require status checks to pass (`checks`; on `main` also `release-guard`), block force pushes and deletion, and no bypass for direct pushes. The check names only appear after the workflow has run once. So the steps are: merge this PR into `develop`, then apply the settings.

## Error handling and edge cases
- First PR into `main` with no tags: the guard skips the tag comparison and only checks the source branch, version format and CHANGELOG.
- `deny.toml` licence allow-list: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0. Unknown registries and git sources are denied. Advisories are denied; duplicate versions only warn. With no dependencies yet, it passes trivially. The list will be widened as needed in later milestones, with a reason for each change.
- Empty crates must still pass `clippy -D warnings`: no unused items, and each lib has only a `//!` doc comment.

## Testing
- Locally on Windows, all five CI commands pass on the new workspace.
- The PR into `develop` shows a green `checks` job and no `release-guard` job.
- I'll test the guard script by hand by running its bash on sample values (feature branch rejected; missing CHANGELOG section rejected; version ≤ tag rejected). No PR into `main` is opened for this.

## Out of scope
`release.yml`, installer and `setup.iss`, signing, base-model download, the dev artifact upload (M6). Any UI or audio code (M1). Updating the PRD in the KB to record the MIT and name decisions (can follow separately).

Note: `AGENTS.md` keeps the PRD §13 text as-is. It already describes `release.yml` and the installer, which will exist from M6 on.
