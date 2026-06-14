# pygrit Release-Readiness Design

**Date:** 2026-06-14
**Status:** Approved (design), hardened per Codex review — pending implementation plan
**Author:** Sean Reifschneider (with Claude)

## Goal

Make the existing read-core `pygrit` MVP installable from PyPI (`pip install
pygrit`) by filling in packaging metadata and adding a trusted-publishing release
workflow. No changes to the binding code or test suite — this is a packaging and
CI/CD pass over software that is already complete and has a fully green CI.

## Background / current state

- `pygrit` is a PyO3 + maturin `abi3` binding to `grit-lib` 0.4.1. CI is green:
  lint, `test` (3.11/3.13), and `build-wheels` for Linux x86_64, Linux aarch64
  (manylinux), and macOS arm64, including an installed-wheel suite + stubtest and
  an sdist source-compile smoke.
- The name **`pygrit` is available on PyPI** (verified: `GET
  https://pypi.org/pypi/pygrit/json` → HTTP 404).
- `pyproject.toml` already declares `name`, `description`, `readme`, `license`,
  `classifiers`, and a `dynamic = ["version"]` (maturin reads the version from
  `Cargo.toml`, currently `0.1.0`). It is **missing `[project.urls]`**.
- `Cargo.toml` `[package]` has only `name`, `version`, `license` — missing
  `description`/`repository`, which produces a "manifest has no description,
  documentation, homepage or repository" warning during the sdist build.
- CI's `build-wheels` job uploads wheels as **artifacts only**; nothing publishes.

## Approved decisions

| Decision | Choice |
| --- | --- |
| PyPI authentication | **Trusted Publishing (OIDC)** — no stored secrets |
| Release trigger | **GitHub Release published** → real PyPI |
| Dry-run path | **`workflow_dispatch`** → TestPyPI |
| Wheel targets | **The 3 CI-tested targets** (Linux x86_64 + aarch64, macOS arm64) **+ sdist** |
| Workflow structure | **Dedicated `release.yml`** (Approach A), isolated publish jobs carrying the OIDC permission |
| Cargo metadata | Add `description`, `repository`, `homepage`, `documentation` (all GitHub repo URL; no separate docs site yet) |

## Deliverables

### 1. `Cargo.toml` `[package]` metadata

Add to the existing `[package]` table (keep `name`, `version`, `license`):

```toml
description = "Python bindings for grit-lib (a Rust reimplementation of Git)"
repository = "https://github.com/linsomniac/pygrit"
homepage = "https://github.com/linsomniac/pygrit"
documentation = "https://github.com/linsomniac/pygrit"
```

- Clears the manifest warning seen during `maturin sdist`.
- `description` matches the existing `pyproject.toml` `description` for consistency.

### 2. `pyproject.toml` `[project.urls]`

Add a new table (the rest of `[project]` is unchanged):

```toml
[project.urls]
Homepage = "https://github.com/linsomniac/pygrit"
Repository = "https://github.com/linsomniac/pygrit"
Issues = "https://github.com/linsomniac/pygrit/issues"
```

These render as sidebar links on the PyPI project page.

### 3. `.github/workflows/release.yml`

A new workflow, separate from `ci.yml`, so the `id-token: write` OIDC permission
never lives in the everyday push/PR workflow.

**Triggers**

```yaml
on:
  release:
    types: [published]      # -> real PyPI
  workflow_dispatch: {}     # -> TestPyPI dry-run
```

**Workflow-level hardening (least privilege).** A restrictive default token scope
at the top; only the publish jobs add `id-token: write`. A `concurrency` group
prevents overlapping runs from racing on immutable PyPI filenames:

```yaml
permissions:
  contents: read
concurrency:
  group: release-${{ github.event.release.tag_name || github.ref }}
  cancel-in-progress: false   # never cancel a partially-completed publish
```

Every `actions/checkout` sets `persist-credentials: false` (no job pushes).

**`version-guard` job** (release trigger only; build/sdist `needs` it on the
release path):
- `if: github.event_name == 'release'`.
- Read the tag from `github.event.release.tag_name`; enforce the exact grammar
  `^v[0-9]+\.[0-9]+\.[0-9]+$` (final releases only — see out-of-scope).
- Extract the crate version with `cargo metadata --locked --format-version=1`
  (NOT grep) and assert it equals the tag minus the leading `v`.
- Any mismatch fails the release before anything is built, so a tag can never
  disagree with the artifact it ships. (On `workflow_dispatch` there is no release
  tag; the guard is skipped and dry-run versions are chosen manually — see Testing
  strategy.)

**`build` job** — mirrors CI's proven `build-wheels` recipe so we ship exactly
what CI validates:
- `needs: [version-guard]` (version-guard's own `if` lets the dispatch path run
  without it).
- `strategy.matrix` over the 3 targets (fail-fast: false):
  - `ubuntu-latest` / `x86_64` — native smoke
  - `ubuntu-latest` / `aarch64` — emulated smoke
  - `macos-14` / `aarch64` — native smoke
- Steps: `actions/checkout@v6` (`persist-credentials: false`),
  `dtolnay/rust-toolchain@1.94.1`, `PyO3/maturin-action@v1` with
  `args: --release --locked --out dist`, **`manylinux: 2014`** (explicit, for a
  deterministic broadly-compatible Linux tag rather than `auto`).
- **abi3 tag verify** (every leg): assert exactly one wheel matching the expected
  `*-cp311-abi3-*.whl` pattern exists (stricter than a bare `grep abi3`).
- **Import smoke on ALL three wheels** — a native binary wheel is never published
  without being executed:
  - native legs (linux x86_64, macOS arm64): install the wheel into a clean venv
    on **Python 3.11** (the declared abi3 floor) and run
    `python -c "import pygrit; pygrit.Repository"`.
  - **linux aarch64 (emulated):** via `docker/setup-qemu-action`, run an arm64
    Python 3.11 container (`--platform linux/arm64`), `pip install` the built
    wheel, and run the same import.
- Upload each leg's `dist/` as an artifact (`wheels-<os>-<target>`) via
  `actions/upload-artifact@v7` with `if-no-files-found: error`.

**`sdist` job** (dedicated, `ubuntu-latest`, `needs: [version-guard]`):
`actions/checkout@v6` (`persist-credentials: false`),
`dtolnay/rust-toolchain@1.94.1`, `astral-sh/setup-uv@v8.2.0`, `uv sync --group
dev`, then **`uv run maturin sdist --out dist`** — uses the maturin pinned in
`uv.lock` (1.14.0), not a floating `uvx maturin`. Build+install into a clean venv
(compiles from source via the pinned Rust toolchain), `import pygrit;
pygrit.Repository` smoke, upload as artifact `sdist` (`upload-artifact@v7`,
`if-no-files-found: error`). A dedicated job keeps the sdist a single, unambiguous
artifact for the publish jobs.

**Publish jobs** — two explicit jobs, each with a static environment and target
(clearer and lower-risk than one job with a dynamic `environment` expression).
Both have `needs: [build, sdist]`, `permissions: { id-token: write, contents:
read }`, check out with `fetch-depth: 0` (history needed for the provenance
check), and run a **provenance + inventory gate** before publishing:
- **Provenance:** assert the built commit is an ancestor of the protected default
  branch (`git merge-base --is-ancestor "$GITHUB_SHA" origin/main`) — a Release
  tag pointing at an unreviewed commit cannot publish.
- **Inventory:** download all artifacts into `dist/`
  (`actions/download-artifact@v7`, `pattern: '*'`, `merge-multiple: true`,
  `path: dist`), then assert `dist/` holds **exactly the 3 expected wheels + 1
  sdist, all of the same normalized version** (guards against a stray or missing
  artifact silently changing the upload set).
- `publish-pypi`: `if: github.event_name == 'release'`, `environment: pypi`,
  `pypa/gh-action-pypi-publish@cef221092ed1bacb1cc03d23a2d87d1d172e277b` (v1.14.0,
  **commit-SHA-pinned** — the credential/OIDC-bearing action), **no token** (OIDC,
  real PyPI).
- `publish-testpypi`: `if: github.event_name == 'workflow_dispatch'`,
  `environment: testpypi`, same SHA-pinned publish action with
  `repository-url: https://test.pypi.org/legacy/`.

All other actions stay on floating major tags to match `ci.yml`'s convention; only
the publish action is SHA-pinned (it is the one action that handles the OIDC
credential / uploads the release).

### 4. Manual prerequisites (human-only — cannot be automated)

Two human-only setup steps; the plan documents the exact values:

**(a) Register the trusted-publisher "pending publisher"** on each index *before*
the first publish:
- **PyPI** (https://pypi.org/manage/account/publishing/): Project `pygrit`, Owner
  `linsomniac`, Repository `pygrit`, Workflow `release.yml`, Environment `pypi`.
- **TestPyPI** (https://test.pypi.org/manage/account/publishing/): same values,
  Environment `testpypi` (only needed for the dry-run path).

A pending publisher does **not** reserve the project name on PyPI — register it
and cut the first real release promptly so `pygrit` is claimed.

**(b) Create and protect the GitHub Environments.** GitHub silently auto-creates
an *unprotected* environment if a workflow merely references one, so create them
explicitly with protection:
- `pypi`: restrict deployments to **protected `v*` tags** (backed by a repository
  ruleset protecting `v*` tags). Required-reviewer protection is impractical for a
  solo maintainer (self-review is blocked), so tag-restriction is the baseline; add
  a reviewer later if the project gains maintainers.
- `testpypi`: restrict deployments to the `main` branch.

### 5. Documentation

Add a short **"Releasing"** section to `README.md` documenting the cut-a-release
procedure: bump the version in **both `Cargo.toml` and `Cargo.lock`** (the `cargo
metadata --locked` guard fails otherwise), commit to `main`, create a GitHub
Release with tag `vX.Y.Z`, and note that publishing then happens automatically via
trusted publishing. Document the TestPyPI dry-run via `workflow_dispatch` and that
a repeat dry-run needs a unique version (PyPI filenames are immutable).

## Out of scope (YAGNI)

- No write/mutation API — read-core only, unchanged.
- No new wheel platforms beyond the 3 green targets (no Intel macOS, no Windows).
- No switch away from `maturin-action` to cibuildwheel.
- No automated version bumping / release-please; version is bumped by hand in
  `Cargo.toml` (+ `Cargo.lock`) and the tag is created manually.
- No prerelease/PEP 440 versions in v1: tags must be `vX.Y.Z` final, so the guard
  can assume literal `tag == version` equality (avoids maturin's SemVer→PEP 440
  normalization, e.g. `1.0.0-alpha.1` → `1.0.0a1`).
- No changes to the existing `ci.yml` or the test suite.

## Testing strategy

A publish cannot be unit-tested, so validation is:

1. **TestPyPI dry-run** via `workflow_dispatch` exercises the build → smoke →
   collect → OIDC-publish *mechanics*. Do not overstate it: TestPyPI uses a
   **separate** trusted-publisher registration, so a green dry-run does **not**
   prove the real-PyPI OIDC config; and because filenames are immutable, a repeat
   dry-run needs a **unique version**. The first live release is what confirms the
   real-PyPI registration.
2. **abi3 verify + import smoke on all three wheels** (native x86_64/macOS +
   **emulated aarch64**) and the **sdist build+install+import smoke** — a broken or
   unimportable artifact fails the job and never reaches publish.
3. **Version guard** — tag/version disagreement fails before the build.
4. **Provenance + inventory gate** — a non-`main` commit, or the wrong artifact
   set, fails before upload.
5. Existing `ci.yml` continues to gate full code correctness (pytest + stubtest) on
   every push to `main`.

## Risks & mitigations

- **Pending publisher not registered / wrong environment name** → first publish
  fails with an OIDC trust error. Mitigation: documented prerequisite + TestPyPI
  dry-run surfaces config-shape errors early (though not the real-PyPI
  registration itself).
- **Unprotected auto-created environment** → mitigated by explicitly creating and
  tag-restricting the `pypi`/`testpypi` environments (prerequisite (b)).
- **Release tag on an unreviewed commit** → provenance gate (`git merge-base
  --is-ancestor … origin/main`) blocks publish.
- **Tag/version drift, or stale `Cargo.lock`** → version guard via `cargo metadata
  --locked` (a forgotten `Cargo.lock` re-bump fails the build, not the upload);
  release procedure documents bumping both.
- **Partial / interrupted publish** → PyPI filenames are immutable; `concurrency`
  (`cancel-in-progress: false`) avoids overlapping runs. Recovery from a partial
  upload requires a **new version**, not a rerun; `skip-existing` is intentionally
  **not** enabled (it can mask stale artifacts).
- **Supply-chain (action tampering)** → the credential-bearing publish action is
  commit-SHA-pinned; other actions follow `ci.yml`'s floating-major convention.
- **Artifact action major mismatch** (upload vs download) → release.yml owns both
  sides; both pinned to `@v7` (node24, v4+ backend, matching `ci.yml`).

## Codex review incorporation

An external Codex review of the first draft found no CRITICAL issues and validated
the core architecture (no-token OIDC, isolated static-environment publish jobs,
separate sdist job, merged artifact download). Its hardening findings were folded
in above:

- **Applied:** least-privilege `permissions: contents: read` + `persist-credentials:
  false` (#3); provenance ancestor gate (#2); hardened version guard as a dedicated
  `cargo metadata --locked` job + `Cargo.lock` re-bump note (#5); `uv run maturin`
  sdist pin to locked 1.14.0 (#4); pre-publish inventory check + `if-no-files-found:
  error` (#6); accurate TestPyPI claims + unique-version caveat (#7); explicit
  `manylinux: 2014` (#4); `concurrency` + partial-publish recovery note (#9);
  environment protection + name-not-reserved note (#1, #10); Python-3.11-pinned
  smoke + exact `cp311-abi3` tag assertion (#11).
- **Decisions (judgment calls):** SHA-pin the publish action only, others stay on
  floating majors per `ci.yml` convention (#4); **add** an emulated aarch64 import
  smoke (#8).
- **Already satisfied:** `py.typed` and the `.pyi` stub already ship in the package
  and are validated by CI's installed-wheel stubtest (#12, typing half).
- **Deferred (LOW, cosmetic):** SPDX `license` expression / `license-files`
  modernization (#12) — the existing `license = { text = "MIT" }` is valid; not
  worth churn for v1.
