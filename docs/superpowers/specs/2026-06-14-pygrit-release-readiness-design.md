# pygrit Release-Readiness Design

**Date:** 2026-06-14
**Status:** Approved (design) — pending implementation plan
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

**`build` job** — mirrors CI's proven `build-wheels` recipe so we ship exactly
what CI validates:

- `strategy.matrix` over the 3 targets (fail-fast: false):
  - `ubuntu-latest` / `x86_64` (smoke: true)
  - `ubuntu-latest` / `aarch64` (smoke: false — cross-built under emulation)
  - `macos-14` / `aarch64` (smoke: true)
- Steps: `actions/checkout@v6`, `dtolnay/rust-toolchain@1.94.1`,
  `PyO3/maturin-action@v1` with `args: --release --locked --out dist`,
  `manylinux: auto`. (No `setup-uv` here — the `build` job only builds/smokes
  wheels; the sdist's `uvx` lives in the dedicated `sdist` job below.)
- **abi3 tag verify** (every leg): `ls dist/*.whl | grep -q abi3`.
- **Per-wheel import smoke** (native legs, `if: matrix.smoke`): install the built
  wheel into a clean venv and `python -c "import pygrit; pygrit.Repository"`, so a
  wheel that does not import can never be published. (Lighter than CI's full
  installed-wheel suite — `ci.yml` already gates correctness on every push to
  `main`; the release leg's job is to catch a broken/unimportable wheel.)
- Upload each leg's `dist/` as an artifact (`wheels-<os>-<target>`) via
  `actions/upload-artifact@v7` (node24, v4+ backend — matches `ci.yml`).

**`sdist` job** (dedicated, on `ubuntu-latest`): `actions/checkout@v6`,
`dtolnay/rust-toolchain@1.94.1`, `astral-sh/setup-uv@v8.2.0`, then `uvx maturin
sdist --out dist`, build+install into a clean venv (compiles from source via the
pinned Rust toolchain), `import pygrit; pygrit.Repository` smoke, and upload as
artifact `sdist` (`actions/upload-artifact@v7`). A dedicated job (rather than
folding sdist into a matrix leg) keeps the sdist a single, unambiguous artifact
for the publish jobs to collect.

**Version-match guard** (release trigger only): assert the wheel/Cargo.toml
version equals the release tag with the leading `v` stripped (e.g. tag `v0.1.0` ⇒
version `0.1.0`). Fail the release on mismatch so a tag can never disagree with the
artifact it ships. Skipped on `workflow_dispatch` (dry-runs have no release tag).

**Publish jobs** — two explicit jobs, each with a static environment and target
(clearer and lower-risk than one job with a dynamic `environment` expression):

- `publish-pypi`: `if: github.event_name == 'release'`, `needs: [build, sdist]`,
  `environment: pypi`, `permissions: { id-token: write }`. Downloads all artifacts
  into `dist/` (`actions/download-artifact@v7` with `pattern: '*'`,
  `merge-multiple: true`), then `pypa/gh-action-pypi-publish@release/v1` with **no
  token** (OIDC, real PyPI).
- `publish-testpypi`: `if: github.event_name == 'workflow_dispatch'`, `needs:
  [build, sdist]`, `environment: testpypi`, `permissions: { id-token: write }`.
  Same download + publish, but `repository-url: https://test.pypi.org/legacy/`.

### 4. Manual prerequisite (human-only — cannot be automated)

Trusted publishing requires a "pending publisher" to be registered on the index
*before* the first publish. The implementer cannot do this; the plan will document
the exact field values for the user to enter:

- **PyPI** (https://pypi.org/manage/account/publishing/):
  - PyPI Project Name: `pygrit`
  - Owner: `linsomniac`
  - Repository name: `pygrit`
  - Workflow name: `release.yml`
  - Environment name: `pypi`
- **TestPyPI** (https://test.pypi.org/manage/account/publishing/) — same values
  except Environment name: `testpypi` (only needed to exercise the dry-run path).

### 5. Documentation

Add a short **"Releasing"** section to `README.md` documenting the cut-a-release
procedure: ensure `Cargo.toml` version is bumped, create a GitHub Release with tag
`vX.Y.Z`, and that publishing happens automatically via trusted publishing. Note
the TestPyPI dry-run via `workflow_dispatch`.

## Out of scope (YAGNI)

- No write/mutation API — read-core only, unchanged.
- No new wheel platforms beyond the 3 green targets (no Intel macOS, no Windows).
- No switch away from `maturin-action` to cibuildwheel.
- No automated version bumping / release-please; version is bumped by hand in
  `Cargo.toml` and the tag is created manually.
- No changes to the existing `ci.yml` or the test suite.

## Testing strategy

A publish cannot be unit-tested, so validation is:

1. **TestPyPI dry-run** via `workflow_dispatch` exercises the entire build →
   collect → OIDC-publish path end-to-end before any real release.
2. **Per-wheel abi3 verify + import smoke** in the `build` job — a broken or
   unimportable wheel fails the job and never reaches publish.
3. **sdist build+install+import smoke** — a broken sdist fails the job.
4. **Version-match guard** — tag/version disagreement fails the release.
5. Existing `ci.yml` continues to gate code correctness on every push to `main`.

## Risks & mitigations

- **Pending publisher not registered** → first publish fails with an OIDC trust
  error. Mitigation: documented prerequisite + TestPyPI dry-run surfaces it before
  a real release.
- **Tag/version drift** → mitigated by the version-match guard.
- **Artifact action major mismatch** (upload vs download) → release.yml owns both
  sides; both pinned to `@v7` (node24, v4+ backend, matching `ci.yml`'s
  `upload-artifact@v7`) to stay on the compatible backend.
- **aarch64 leg is cross-built under emulation** and does not run the foreign
  interpreter — same accepted limitation as CI; the native legs + sdist cover the
  import smoke.
