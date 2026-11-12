# VC-2 — M0: Project setup — implementation plan

## Goal
Turn the repository (today only a 2-line `README.md` on `main`/`develop`) into a buildable, MIT-licensed Rust workspace with open-source files, agent guides and a `windows-latest` CI workflow, delivered as one PR into `develop`.

## Architecture
A Cargo workspace (`resolver = "3"`, edition 2024) holds five empty member crates under `crates/`; the workspace `Cargo.toml` is the single source of `version`, `edition`, `license`. `deny.toml` configures `cargo deny`. `.github/workflows/ci.yml` has two jobs: `checks` (fmt, clippy, test, deny, release build) on every push/PR to `develop`/`main`, and `release-guard` (inline bash) only on PRs into `main`. Branch protection is applied by hand by the maintainer after merge (checklist below).

## Facts about the repo (checked at `7e41d5f`)
- Tracked files: only `README.md` (`# VoiceChanger` + one description line). No `Cargo.toml`, no `.github/`, no tags.
- Remote `origin` = `https://github.com/mmhy2003/VoiceChanger`; branches `origin/main`, `origin/develop`. Work happens on the current branch `megoo/VC-2-m0-project-setup-repo-licence-branches-c`; the PR targets `develop`.
- Commit style: use conventional prefixes (`build:`, `docs:`, `ci:`, `chore:`), each commit ending with the `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` trailer.
- Requires Rust ≥ 1.85 locally (edition 2024 / resolver 3). Check with `rustc --version`.

## File map
| File | Responsibility |
|---|---|
| `Cargo.toml` | Workspace: members, `[workspace.package]` (version 0.1.0, edition 2024, MIT, repository, authors), `[workspace.lints]` |
| `Cargo.lock` | Committed (the workspace ships a binary) |
| `rust-toolchain.toml` | stable + rustfmt + clippy |
| `.gitignore` | `/target` |
| `crates/app/{Cargo.toml,src/main.rs}` | package/bin `voicechanger`, empty `main` |
| `crates/{audio,engine,models,convert}/{Cargo.toml,src/lib.rs}` | libs `vc-audio`, `vc-engine`, `vc-models`, `vc-convert`, doc comment only |
| `deny.toml` | cargo-deny: licences, advisories, bans, sources |
| `LICENSE` | MIT text |
| `README.md` | rewritten: what, status, requirements, build, licence |
| `CHANGELOG.md` | Keep a Changelog, `## [Unreleased]` with M0 entry |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md` | contributor docs |
| `AGENTS.md`, `CLAUDE.md` | PRD §13 text verbatim |
| `.github/ISSUE_TEMPLATE/{bug_report.yml,feature_request.yml,config.yml}` | issue forms, blank issues off |
| `.github/PULL_REQUEST_TEMPLATE.md` | PR checklist |
| `.github/workflows/ci.yml` | jobs `checks` and `release-guard` |

---

## Step 1 — Rust workspace skeleton with five crates

"Test first": `cargo metadata --format-version 1 --no-deps` and `cargo build` currently fail with `could not find Cargo.toml`. Run one to confirm.

Create exactly these files.

`Cargo.toml`:
```toml
[workspace]
members = ["crates/*"]
resolver = "3"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"
repository = "https://github.com/mmhy2003/VoiceChanger"
authors = ["VoiceChanger contributors"]

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`.gitignore`:
```
/target
```

`crates/app/Cargo.toml`:
```toml
[package]
name = "voicechanger"
description = "Real-time RVC voice changer for Windows"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true

[[bin]]
name = "voicechanger"
path = "src/main.rs"

[lints]
workspace = true
```
`crates/app/src/main.rs`:
```rust
//! VoiceChanger desktop app. The eframe UI arrives in M1.

fn main() {}
```

Library crates — same `Cargo.toml` shape (no `[[bin]]`), with:
| dir | `name` | `description` | `src/lib.rs` |
|---|---|---|---|
| `crates/audio` | `vc-audio` | `Audio devices, ring buffers and resampling for VoiceChanger` | `//! Audio device handling, ring buffers and resampling.` |
| `crates/engine` | `vc-engine` | `RVC v2 inference pipeline for VoiceChanger` | `//! RVC v2 pipeline: ONNX sessions, F0 extraction and crossfade.` |
| `crates/models` | `vc-models` | `Model library and metadata for VoiceChanger` | `//! Model library and per-model metadata.` |
| `crates/convert` | `vc-convert` | `PyTorch .pth to ONNX conversion for VoiceChanger` | `//! Conversion of PyTorch `.pth` RVC v2 models to ONNX.` |

Each `lib.rs` contains only that one `//!` line (plus trailing newline). No dependencies anywhere.

Verify (all must pass, no warnings):
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --release
```
`target/release/voicechanger.exe` exists. Commit `Cargo.lock` with the rest.

Commit: `build: add Rust workspace with app, audio, engine, models and convert crates`

## Step 2 — cargo-deny configuration

Test first: with cargo-deny installed (`cargo install cargo-deny --locked` if missing), `cargo deny check` without `deny.toml` runs with defaults and fails on licences (no allow-list, so MIT of our own crates is rejected). Run it and note the failure.

`deny.toml`:
```toml
[graph]
targets = ["x86_64-pc-windows-msvc"]

[advisories]
version = 2
yanked = "deny"

[licenses]
version = 2
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
]
confidence-threshold = 0.8
unused-allowed-license = "allow"

[bans]
multiple-versions = "warn"
wildcards = "allow"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
allow-git = []
```
Add a short header comment above `[licenses]`: `# Widen this list only with a reason in the PR description (project licence is MIT).`

Verify: `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`. If the local cargo-deny rejects a key (older version), upgrade cargo-deny rather than removing the key; CI installs the latest.

Commit: `build: add cargo-deny configuration`

## Step 3 — LICENSE, README and CHANGELOG

`LICENSE`: standard MIT text (https://opensource.org/license/mit), first line `MIT License`, copyright line `Copyright (c) 2026 VoiceChanger contributors`.

`README.md` (replace the 2 lines; keep the title and the existing description sentence):
```markdown
# VoiceChanger

Real-time AI voice changer for Windows, written in Rust. Runs RVC voice models locally on your GPU via ONNX Runtime, with no Python required.

## Status

Pre-alpha. Milestone M0 (project setup) is in progress: the repository has the Rust workspace and CI, but no app features yet. Nothing is released.

## Requirements

- Windows 10 (22H2) or Windows 11, x64
- Rust stable (the version is pinned by `rust-toolchain.toml`; install with [rustup](https://rustup.rs))
- A virtual audio cable (e.g. VB-Audio Virtual Cable) to use the app once it exists

## Building

    cargo build --release
    cargo test --workspace
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo deny check

The binary is `target\release\voicechanger.exe`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests target `develop`.

## Licence

MIT. See [LICENSE](LICENSE).
```

`CHANGELOG.md`:
```markdown
# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses `X.Y.Z` versions as described in [AGENTS.md](AGENTS.md).

## [Unreleased]

### Added
- Project setup: Rust workspace (`app`, `audio`, `engine`, `models`, `convert`), MIT licence, contributor guides, agent guides and CI on Windows.
```

Verify: files exist; `cargo build` still green (no code changed).
Commit: `docs: add MIT licence, README and changelog`

## Step 4 — CONTRIBUTING, CODE_OF_CONDUCT and SECURITY

`CONTRIBUTING.md` sections:
1. **Branches** — `develop` is the integration branch; create `feature/<name>` or `fix/<name>` from `develop` and open the PR into `develop`. `main` only receives release PRs from `develop` and `hotfix/*` PRs. Never push directly to `develop` or `main`; both are protected and CI must pass.
2. **Before you push** — run the five commands from README "Building" (fmt, clippy `-D warnings`, test, `cargo deny check`, release build). CI on `windows-latest` runs the same and fails on warnings.
3. **Changelog** — if your change affects users, add a line under `## [Unreleased]` in `CHANGELOG.md`. Don't bump the version; that happens only in the release PR into `main` (see AGENTS.md "Releasing").
4. **Dependencies** — new crates must pass `cargo deny check`; widening the licence allow-list in `deny.toml` needs a reason in the PR.
5. **Licence** — "By contributing, you agree that your contributions are licensed under the MIT licence (see LICENSE)."
6. **Code of conduct** — link to `CODE_OF_CONDUCT.md`.

`CODE_OF_CONDUCT.md`: Contributor Covenant 2.1 full text, verbatim from https://www.contributor-covenant.org/version/2/1/code_of_conduct/ (copy the official markdown). Replace `[INSERT CONTACT METHOD]` with: `the project maintainers through GitHub, by contacting a maintainer of https://github.com/mmhy2003/VoiceChanger directly`. Keep the attribution section.

`SECURITY.md`:
```markdown
# Security policy

## Supported versions

Only the latest release receives security fixes. There is no release yet.

## Reporting a vulnerability

Please do not open a public issue. Report it privately through GitHub's
private vulnerability reporting: go to the repository's **Security** tab and
choose **Report a vulnerability**
(https://github.com/mmhy2003/VoiceChanger/security/advisories/new).

Include the version or commit, steps to reproduce and the impact. We aim to
reply within 7 days and will credit you in the advisory unless you ask us not to.

Files handled by the app (`.onnx`, `.pth`, `.zip`) are untrusted input;
reports about parsing them are in scope.
```

Verify: Markdown renders (preview); no TBD placeholders left (`git grep -n 'INSERT' -- '*.md'` returns nothing).
Commit: `docs: add contributing guide, code of conduct and security policy`

## Step 5 — AGENTS.md and CLAUDE.md

Copy the two fenced blocks from PRD §13 (KB "PRD: Real-time RVC Voice Changer for Windows", rev 2) **verbatim** into `AGENTS.md` and `CLAUDE.md` — the content between the ```` ```` ```` fences, not the fences. Do not edit the text, even where it mentions `release.yml`, `installer\setup.iss` or `assets/` that do not exist yet (decided in the design). Both files end with a newline.

First lines must be `# AGENTS.md` and `# CLAUDE.md`; `CLAUDE.md` contains `@AGENTS.md`. Full text is reproduced in the appendix at the end of this plan.

Verify: `git grep -n '@AGENTS.md' CLAUDE.md` matches; `git grep -n 'Never push directly to' AGENTS.md` matches.
Commit: `docs: add AGENTS.md and CLAUDE.md agent guides`

## Step 6 — Issue and pull request templates

`.github/ISSUE_TEMPLATE/config.yml`:
```yaml
blank_issues_enabled: false
contact_links:
  - name: Security vulnerability
    url: https://github.com/mmhy2003/VoiceChanger/security/advisories/new
    about: Report security issues privately, not as a public issue.
```

`.github/ISSUE_TEMPLATE/bug_report.yml`:
```yaml
name: Bug report
description: Something doesn't work as expected
labels: [bug]
body:
  - type: textarea
    id: what-happened
    attributes:
      label: What happened?
      description: What did you do, what did you expect, and what happened instead?
    validations:
      required: true
  - type: textarea
    id: steps
    attributes:
      label: Steps to reproduce
      placeholder: "1. ...\n2. ...\n3. ..."
    validations:
      required: true
  - type: input
    id: version
    attributes:
      label: VoiceChanger version or commit
    validations:
      required: true
  - type: input
    id: windows
    attributes:
      label: Windows version
      placeholder: Windows 11 23H2
    validations:
      required: true
  - type: input
    id: gpu
    attributes:
      label: GPU and execution provider
      placeholder: RTX 3060, DirectML
  - type: textarea
    id: logs
    attributes:
      label: Logs
      description: Relevant lines from the log folder.
      render: text
```

`.github/ISSUE_TEMPLATE/feature_request.yml`:
```yaml
name: Feature request
description: Suggest an idea or improvement
labels: [enhancement]
body:
  - type: textarea
    id: problem
    attributes:
      label: Problem
      description: What are you trying to do, and what gets in the way?
    validations:
      required: true
  - type: textarea
    id: proposal
    attributes:
      label: Proposed solution
    validations:
      required: true
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives considered
```

`.github/PULL_REQUEST_TEMPLATE.md`:
```markdown
## Summary

<!-- What does this change and why? Link the issue if there is one. -->

## Checklist

- [ ] The PR targets `develop` (or is a release/hotfix PR into `main`)
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo deny check` passes (if dependencies changed)
- [ ] `CHANGELOG.md` updated under `## [Unreleased]` (if users are affected)
```

Verify YAML parses: e.g. `python -c "import yaml,sys;[yaml.safe_load(open(f)) for f in sys.argv[1:]]" .github/ISSUE_TEMPLATE/*.yml` (or any YAML linter available).
Commit: `chore: add issue and pull request templates`

## Step 7 — CI workflow: `checks` job

Create `.github/workflows/ci.yml`:
```yaml
name: CI

on:
  push:
    branches: [develop, main]
  pull_request:
    branches: [develop, main]

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  checks:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - name: Format
        run: cargo fmt --all -- --check
      - name: Lint
        run: cargo clippy --all-targets --all-features -- -D warnings
      - name: Test
        run: cargo test --workspace
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-deny
      - name: Licences and advisories
        run: cargo deny check
      - name: Release build
        run: cargo build --release
```
The job id must be exactly `checks` (it becomes the required status check name).

Verify: YAML parses (as in Step 6); if `actionlint` is available run `actionlint .github/workflows/ci.yml` → no output. Locally re-run the five commands → all pass.
Commit: `ci: add CI workflow with fmt, clippy, test, cargo-deny and release build`

## Step 8 — CI workflow: `release-guard` job and manual guard test

Append to `jobs:` in `.github/workflows/ci.yml`:
```yaml
  release-guard:
    if: github.event_name == 'pull_request' && github.base_ref == 'main'
    runs-on: windows-latest
    defaults:
      run:
        shell: bash
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
          fetch-tags: true
      - name: Check source branch, version and changelog
        env:
          HEAD_REF: ${{ github.head_ref }}
        run: |
          set -euo pipefail

          case "$HEAD_REF" in
            develop|hotfix/*) ;;
            *)
              echo "::error::PRs into main must come from develop or hotfix/*, not '$HEAD_REF'."
              exit 1
              ;;
          esac

          version=$(awk '
            /^\[workspace\.package\]/ { in_pkg = 1; next }
            /^\[/ { in_pkg = 0 }
            in_pkg && /^version[[:space:]]*=/ {
              sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
            }' Cargo.toml)
          if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "::error file=Cargo.toml::[workspace.package] version '$version' is not plain X.Y.Z."
            exit 1
          fi

          latest=$(git tag --list 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sed 's/^v//' | sort -V | tail -n 1 || true)
          if [ -n "$latest" ]; then
            highest=$(printf '%s\n%s\n' "$latest" "$version" | sort -V | tail -n 1)
            if [ "$version" = "$latest" ] || [ "$highest" != "$version" ]; then
              echo "::error file=Cargo.toml::Version $version must be higher than the latest tag v$latest."
              exit 1
            fi
          else
            echo "No vX.Y.Z tags yet; skipping the tag comparison."
          fi

          escaped=${version//./\\.}
          if ! grep -qE "^## \[$escaped\]" CHANGELOG.md; then
            echo "::error file=CHANGELOG.md::CHANGELOG.md has no '## [$version]' section."
            exit 1
          fi

          echo "Release guard passed for $version (from $HEAD_REF)."
```
The job id must be exactly `release-guard`.

**Manual test (no PR into main).** Copy the `run:` body into a scratch script outside the repo (e.g. `%TEMP%\vc-guard\guard.sh`), then in Git Bash create a scratch git repo in another new temp dir with a `Cargo.toml` containing `[workspace.package]\nversion = "0.2.0"` and a `CHANGELOG.md` with `## [0.2.0] - 2026-11-12`, commit, and run `HEAD_REF=<ref> bash guard.sh` for each case:
| Case | Setup | Expected |
|---|---|---|
| feature branch | `HEAD_REF=feature/x` | exit 1, `::error::PRs into main must come from develop or hotfix/*` |
| no tags, valid | `HEAD_REF=develop` | exit 0, prints "No vX.Y.Z tags yet" and "Release guard passed" |
| hotfix ok | `git tag v0.1.0`; `HEAD_REF=hotfix/crash` | exit 0 |
| equal to tag | `git tag v0.2.0` | exit 1, "must be higher than the latest tag v0.2.0" |
| lower than tag | tag `v0.10.0` (checks `sort -V`, not lexical) | exit 1 |
| bad format | version `0.2` | exit 1, "is not plain X.Y.Z" |
| missing changelog | delete the `## [0.2.0]` line | exit 1, "has no '## [0.2.0]' section" |
Delete the scratch dirs afterwards. Record the results for the PR description.

Verify YAML parses / `actionlint` clean.
Commit: `ci: add release guard for pull requests into main`

## Step 9 — Final verification and PR into develop

Run the full verification below on a clean checkout of the branch. Then push the branch and open a PR into `develop` (`gh pr create --base develop`) titled `VC-2: M0 project setup` with the description below. Confirm on GitHub that the PR shows a green `checks` job and **no** `release-guard` run (skipped). Do not merge into `main`, tag, or apply branch protection.

PR description:
```markdown
## Summary
M0 project setup (VC-2): Rust workspace (`crates/app` → `voicechanger.exe`, `vc-audio`, `vc-engine`, `vc-models`, `vc-convert`), version 0.1.0, MIT licence, README/CONTRIBUTING/CODE_OF_CONDUCT/SECURITY/CHANGELOG, AGENTS.md + CLAUDE.md (PRD §13), issue/PR templates, `cargo-deny` config and CI on `windows-latest`.

CI jobs: `checks` (fmt, clippy -D warnings, test, cargo deny, release build) on pushes/PRs to develop and main; `release-guard` (source branch, version > latest tag, CHANGELOG section) only on PRs into main.

Release guard tested by hand: <paste the case table results from step 8>.

## Branch protection — please apply after merging this PR
The check names only exist after CI has run once, so merge first, then in Settings → Rules → Rulesets (or Branches → classic protection):

**`develop`**
- Require a pull request before merging
- Require status checks to pass: `checks`
- Block force pushes
- Restrict deletions
- No bypass list (no direct pushes)

**`main`**
- Same as `develop`, with required status checks `checks` and `release-guard`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

---

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
git ls-files
```
Expected: all commands succeed with no warnings; `target\release\voicechanger.exe` exists; `git ls-files` lists every file in the file map (and `Cargo.lock`), and nothing under `target/`. On GitHub the PR into `develop` shows `checks` green and `release-guard` skipped. Manual guard cases from step 8 behave as in the table.

## Requirement → step map
- Workspace + crate layout, stable toolchain, version in workspace: 1
- cargo deny check (licences/advisories/bans/sources): 2, 7
- LICENSE (MIT only), README, CHANGELOG: 3
- CONTRIBUTING, CODE_OF_CONDUCT, SECURITY: 4
- AGENTS.md / CLAUDE.md verbatim: 5
- Issue/PR templates: 6
- ci.yml (fmt, clippy, test, deny, release build): 7
- Release guard on PRs into main: 8
- Branch protection checklist in PR (applied by maintainer): 9

## Appendix — AGENTS.md and CLAUDE.md (PRD §13, verbatim)

### AGENTS.md
```markdown
# AGENTS.md

## Project
Open-source real-time voice changer for Windows (x64 only). Rust + egui (eframe).
Uses RVC v2 models in ONNX format via ONNX Runtime (`ort`, DirectML/CUDA/CPU)
and outputs to a virtual audio cable. A Convert tab turns PyTorch `.pth`
RVC v2 models into ONNX without Python. Released with an Inno Setup installer
through GitHub Releases.

## Ground rules
- Windows only. Don't add cross-platform abstractions or non-Windows code paths.
- Never block or allocate in audio callbacks. Use the ring buffers.
- Keep inference on the worker thread, never on the UI or audio threads.
- Never commit model files. Base models are fetched at build time
  (see `assets/base-models.toml`). Weight-free templates in `assets/templates/`
  are the only ONNX files in the repository.
- `.pth` files are untrusted. Read them only with the restricted unpickler in
  `crates/convert`; never add code that executes pickle contents.
- New dependencies must pass `cargo deny check` (licence and advisories).

## Branches
- `develop`: development. Feature/fix branches start here and PRs target it.
- `main`: production. Only release PRs from `develop` and `hotfix/*` PRs
  are merged into it. Every merge to `main` publishes a release.
- Hotfixes: branch from `main`, PR into `main`, then merge `main` back into `develop`.
- Never push directly to `develop` or `main`.

## Build
- `cargo build --release`
- `cargo test --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Installer: `ISCC /DAppVersion=X.Y.Z installer\setup.iss`

## CI/CD
- `.github/workflows/ci.yml`: runs on pushes and PRs to `develop` and `main`.
- `.github/workflows/release.yml`: runs on pushes to `main`; builds, signs,
  tags `vX.Y.Z` and publishes the GitHub Release.
- Run fmt, clippy and tests locally before pushing; CI fails on warnings.

## Versioning: X.Y.Z
- X = major release (breaking changes, new architecture, incompatible
  settings or model format). Resets Y and Z to 0.
- Y = minor update (new compatible features or improvements). Resets Z to 0.
- Z = build (every released build: fixes, tweaks, rebuilds).
- Single source of truth: `version` in the workspace `Cargo.toml`.
  Installer, About box and Windows file version (X.Y.Z.0) derive from it.
- Bump the version only in the release or hotfix PR into `main`.
- Never reuse a version. Fix a bad release with a new build number.

## Releasing
1. Open a PR from `develop` into `main`.
2. Bump the version in `Cargo.toml`; move "Unreleased" notes in
   `CHANGELOG.md` under the new version.
3. Merge after CI passes. The release workflow tags and publishes.
4. Merge `main` back into `develop`.

Agents must not bump versions, merge into `main`, tag or publish releases
unless asked to.
```

### CLAUDE.md
```markdown
# CLAUDE.md

Read and follow @AGENTS.md. It holds the project rules, branches, build
commands, CI/CD, the X.Y.Z versioning scheme and the release process.

Claude-specific notes:
- Base work on `develop` and open PRs against `develop` unless told otherwise.
- Ask before bumping the version, merging into `main`, creating tags or
  publishing releases.
- When a change affects users, add a line under "Unreleased" in CHANGELOG.md.
```
