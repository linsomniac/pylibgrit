# pygrit 6-Wheel Platform Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Widen pygrit's binary-wheel coverage from 3 targets to a full 6-target grid (add macOS Intel x86_64 + musllinux x86_64/aarch64) so `pip install pygrit` ships a wheel for the common platforms instead of source-compiling.

**Architecture:** Pure CI/packaging/docs change — no `src/` or `python/` edits. `release.yml` builds all 6 legs; `ci.yml` mirrors the 5 cheap legs (skips emulated musl-aarch64). The publish-gate inventory checker and its offline test move from 3→6 wheels. README platform table updated.

**Tech Stack:** GitHub Actions, PyO3/maturin-action (abi3 wheels), Python (the inventory checker + pytest), Docker (Alpine/slim smoke containers + QEMU for arm64).

**Branch:** `wheel-coverage` (already created off `main` @ e1d2fab).

---

## File Structure

| File | Responsibility | Change |
| --- | --- | --- |
| `.github/scripts/check_release_inventory.py` | Publish-gate: assert exactly N wheels + 1 sdist, all abi3, one version | `EXPECTED_WHEELS` 3→6, docstring, AIDEV-NOTE |
| `.github/scripts/test_check_release_inventory.py` | Offline test of the checker | 6-wheel fixture + count-assert update |
| `.github/workflows/release.yml` | Release build/publish — builds all 6 | `build` matrix→6, smoke refactor, artifact names, comments |
| `.github/workflows/ci.yml` | Per-PR CI — mirrors 5 legs | `build-wheels` matrix→5, musl smoke step, sdist guard, names |
| `README.md` | User-facing platform table | musl + Intel-Mac rows |

Task order is offline-testable-first (the inventory gate, which has real unit tests) → workflows (validated by the CI run on push) → docs → push-and-watch validation.

---

### Task 1: Inventory gate — bump to 6 wheels (TDD)

**Files:**
- Modify: `.github/scripts/check_release_inventory.py`
- Test: `.github/scripts/test_check_release_inventory.py`

The checker counts wheels in `dist/` and fails unless there are exactly `EXPECTED_WHEELS` of them, all `cp311-abi3`, all one version. It is pure and unit-tested offline. We update the test first (red), then the constant (green).

- [ ] **Step 1: Update the test fixture + count assertion to expect 6 wheels**

In `.github/scripts/test_check_release_inventory.py`, replace the `_good_dist` helper (currently builds 3 wheels) with one that builds all 6 target platforms:

```python
def _good_dist(directory: Path, version: str = "0.1.0") -> None:
    _touch(directory, f"pygrit-{version}.tar.gz")
    _touch(
        directory,
        f"pygrit-{version}-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
    )
    _touch(
        directory,
        f"pygrit-{version}-cp311-abi3-manylinux_2_17_aarch64.manylinux2014_aarch64.whl",
    )
    _touch(directory, f"pygrit-{version}-cp311-abi3-musllinux_1_2_x86_64.whl")
    _touch(directory, f"pygrit-{version}-cp311-abi3-musllinux_1_2_aarch64.whl")
    _touch(directory, f"pygrit-{version}-cp311-abi3-macosx_10_12_x86_64.whl")
    _touch(directory, f"pygrit-{version}-cp311-abi3-macosx_11_0_arm64.whl")
```

And in `test_wrong_wheel_count_fails`, change the asserted substring from `"3 wheels"` to `"6 wheels"` (deleting one wheel now leaves 5, and the error reads "expected exactly 6 wheels, found 5"):

```python
def test_wrong_wheel_count_fails(tmp_path: Path) -> None:
    _good_dist(tmp_path)
    (tmp_path / "pygrit-0.1.0-cp311-abi3-macosx_11_0_arm64.whl").unlink()
    errors = check_inventory(tmp_path)
    assert any("6 wheels" in e for e in errors), errors
```

Leave `test_missing_sdist_fails`, `test_extra_sdist_fails`, `test_non_abi3_wheel_fails`, and `test_version_mismatch_fails` unchanged: each deletes/re-adds `pygrit-0.1.0-cp311-abi3-macosx_11_0_arm64.whl` (still present in the 6-set) and keeps the wheel count at 6, so their specific checks still fire.

- [ ] **Step 2: Run the test to verify it fails**

Run: `uv run pytest .github/scripts/test_check_release_inventory.py -v`
Expected: FAIL — `test_valid_inventory_passes` fails (6 wheels found but `EXPECTED_WHEELS` is still 3, so `check_inventory` returns a non-empty error list) and `test_wrong_wheel_count_fails` fails (message still says "3 wheels").

- [ ] **Step 3: Bump the checker to 6 + AIDEV-NOTE + docstring**

In `.github/scripts/check_release_inventory.py`, update the constant and add an anchor note. Replace:

```python
DIST_NAME = "pygrit"
EXPECTED_WHEELS = 3
```

with:

```python
DIST_NAME = "pygrit"
# AIDEV-NOTE: EXPECTED_WHEELS must equal the number of build legs in the release
# workflow's `build` matrix (.github/workflows/release.yml). Bump both together when
# adding or removing a wheel target.
EXPECTED_WHEELS = 6
```

Then fix the two docstring references. Change `three CPython ``abi3`` wheels` to `six CPython ``abi3`` wheels` (top paragraph), and change `"exactly 3 wheels"` to `"exactly 6 wheels"` (the paragraph explaining the platform-count implication). The exact current lines:

```
Asserts that a ``dist/`` directory contains exactly the artifacts the release
workflow is expected to produce: three CPython ``abi3`` wheels (one per target
platform) and one sdist, every file carrying the same project version. Any
```
→ change `three` to `six`. And:
```
Because each target platform yields a uniquely named wheel, "exactly 3 wheels"
already implies three distinct platforms; there is no separate platform-tag
```
→ change `"exactly 3 wheels"` to `"exactly 6 wheels"` and `three distinct platforms` to `six distinct platforms`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `uv run pytest .github/scripts/test_check_release_inventory.py -v`
Expected: PASS — all 6 tests green.

- [ ] **Step 5: Format + lint + type-check the changed files**

Run:
```bash
uv run ruff format .github/scripts/check_release_inventory.py .github/scripts/test_check_release_inventory.py
uv run ruff check .github/scripts/check_release_inventory.py .github/scripts/test_check_release_inventory.py
uv run mypy .github/scripts/check_release_inventory.py .github/scripts/test_check_release_inventory.py
```
Expected: ruff reports "All checks passed!" / no reformatting, mypy reports "Success: no issues found".

- [ ] **Step 6: Commit**

```bash
git add .github/scripts/check_release_inventory.py .github/scripts/test_check_release_inventory.py
git commit -m "ci: inventory gate expects 6 wheels (was 3)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: release.yml — full 6-leg build matrix + smoke refactor

**Files:**
- Modify: `.github/workflows/release.yml` (the `build` job, the header comment, both publish jobs' inventory step name)

The `build` job currently has a 3-entry matrix and two smoke modes (`native`, `emulated`). Replace it with a 6-entry matrix and a generalized `container` smoke step parameterized by `image` + `platform`, so one step covers all three container legs (glibc-arm, musl-x64, musl-arm). Every leg defines `manylinux` so artifact names are unique and no empty input reaches maturin-action.

- [ ] **Step 1: Replace the entire `build:` job**

Replace the `build:` job (from `  build:` through its `upload-artifact` step, i.e. the current lines `build:` … `if-no-files-found: error`) with exactly:

```yaml
  build:
    name: build (${{ matrix.os }} ${{ matrix.target }} ${{ matrix.manylinux }})
    needs: [version-guard]
    # version-guard is skipped on workflow_dispatch (its own `if`); allow build to
    # run when the guard either succeeded (release) or was skipped (dispatch), but
    # not when it actually failed.
    if: always() && (needs.version-guard.result == 'success' || needs.version-guard.result == 'skipped')
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          # Linux glibc (manylinux_2_17 via "2014") — native + emulated-arm64 smoke.
          - os: ubuntu-latest
            target: x86_64
            manylinux: "2014"
            smoke: native
          - os: ubuntu-latest
            target: aarch64
            manylinux: "2014"
            smoke: container
            image: python:3.11-slim
            platform: linux/arm64
          # Linux musl (musllinux_1_2) — Alpine container smoke (arm64 via QEMU).
          - os: ubuntu-latest
            target: x86_64
            manylinux: musllinux_1_2
            smoke: container
            image: python:3.11-alpine
            platform: linux/amd64
          - os: ubuntu-latest
            target: aarch64
            manylinux: musllinux_1_2
            smoke: container
            image: python:3.11-alpine
            platform: linux/arm64
          # macOS — native smoke. `manylinux: auto` is a no-op here (Linux-only); it is
          # set only so every leg defines it (unique artifact names; no empty input).
          - os: macos-13
            target: x86_64
            manylinux: auto
            smoke: native
          - os: macos-14
            target: aarch64
            manylinux: auto
            smoke: native
    steps:
      - uses: actions/checkout@v6
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@1.94.1
      - uses: PyO3/maturin-action@v1
        with:
          target: ${{ matrix.target }}
          args: --release --locked --out dist
          # glibc legs pin manylinux_2_17 ("2014") for the broadest-compatible tag on
          # RELEASED wheels; musl legs build musllinux_1_2; macOS passes `auto` (no-op).
          # ci.yml uses `auto` for glibc (newer glibc, fine for dev — intentional
          # divergence). Ignored on macOS (manylinux is Linux-only).
          manylinux: ${{ matrix.manylinux }}
      - name: Verify exactly one cp311-abi3 wheel
        shell: bash
        run: |
          set -euo pipefail
          shopt -s nullglob
          wheels=(dist/*-cp311-abi3-*.whl)
          if [[ ${#wheels[@]} -ne 1 ]]; then
            echo "::error::expected exactly one *-cp311-abi3-*.whl, found ${#wheels[@]}: ${wheels[*]:-<none>}"
            ls -la dist || true
            exit 1
          fi
          echo "abi3 wheel OK: ${wheels[0]}"
      - name: Set up Python 3.11 for native smoke
        if: matrix.smoke == 'native'
        uses: actions/setup-python@v5
        with:
          python-version: "3.11"
      - name: Import smoke (native, Python 3.11)
        if: matrix.smoke == 'native'
        shell: bash
        run: |
          set -euxo pipefail
          python -m venv /tmp/smoke
          /tmp/smoke/bin/python -m pip install --upgrade pip
          /tmp/smoke/bin/pip install dist/*-cp311-abi3-*.whl
          /tmp/smoke/bin/python -c "import pygrit; pygrit.Repository"
          /tmp/smoke/bin/python -c "import pygrit, os; p=os.path.dirname(pygrit.__file__); assert 'site-packages' in p, p; print('imported from', p)"
      - name: Set up QEMU for emulated arm64 smoke
        if: matrix.smoke == 'container' && matrix.platform == 'linux/arm64'
        uses: docker/setup-qemu-action@v3
      - name: Import smoke (container)
        if: matrix.smoke == 'container'
        shell: bash
        run: |
          set -euxo pipefail
          # One step for all container legs: glibc-arm64 (slim) + musl x86_64/arm64
          # (alpine). `sh -c` works in both slim (dash) and alpine (busybox sh).
          wheel="$(ls dist/*-cp311-abi3-*.whl)"
          docker run --rm --platform ${{ matrix.platform }} \
            -v "$PWD/dist:/dist:ro" ${{ matrix.image }} \
            sh -c "pip install /dist/$(basename "$wheel") && python -c 'import pygrit; pygrit.Repository'"
      - uses: actions/upload-artifact@v7
        with:
          name: wheels-${{ matrix.os }}-${{ matrix.target }}-${{ matrix.manylinux }}
          path: dist
          if-no-files-found: error
```

- [ ] **Step 2: Update the header comment's `build` description**

Replace this comment block near the top of the file:

```
#   build          — 3 targets (linux x86_64/aarch64, macOS arm64): build the abi3
#                    wheel, assert exactly one cp311-abi3 wheel, import-smoke it
#                    (native on x86_64/macOS; emulated arm64 via QEMU), upload.
```

with:

```
#   build          — 6 targets (linux glibc x86_64/aarch64, linux musl x86_64/aarch64,
#                    macOS x86_64/arm64): build the abi3 wheel, assert exactly one
#                    cp311-abi3 wheel, import-smoke it (native on glibc-x86_64 + macOS;
#                    container on the arm64 + musl legs, arm64 via QEMU), upload.
```

- [ ] **Step 3: Update both publish jobs' inventory step name (3→6)**

Both `publish-pypi` and `publish-testpypi` have a step named `Inventory — exactly 3 wheels + 1 sdist, single version`. Change both to `Inventory — exactly 6 wheels + 1 sdist, single version` (the underlying `check_release_inventory.py dist` call is unchanged — Task 1 made it expect 6).

- [ ] **Step 4: Validate the YAML parses**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('release.yml: valid YAML')"
```
Expected: `release.yml: valid YAML` (no traceback).

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: build the full 6-wheel grid in release.yml

Add Intel-Mac (macos-13) + musllinux_1_2 x86_64/aarch64 legs; collapse
the container smoke steps into one image/platform-parameterized step.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: ci.yml — mirror the 5 cheap legs

**Files:**
- Modify: `.github/workflows/ci.yml` (the `build-wheels` job name, matrix, maturin step, a new musl smoke step, the sdist-step guard, the artifact name, and the header comment)

Add two legs to `build-wheels` (macOS x86_64, musllinux x86_64); leave the emulated musl-aarch64 to release only. A `manylinux` discriminator field disambiguates the two ubuntu-x86_64 legs (job name + artifact name) and gates the one-shot sdist step to the glibc leg. The musl leg does an Alpine import-smoke only.

- [ ] **Step 1: Disambiguate the job name (two x86_64 legs would collide)**

Change the job `name`:
```yaml
    name: build-wheels (${{ matrix.target }})
```
to:
```yaml
    name: build-wheels (${{ matrix.os }} ${{ matrix.target }} ${{ matrix.manylinux }})
```

- [ ] **Step 2: Replace the matrix with the 5-leg version**

Replace the current matrix:
```yaml
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64
            smoke: true
          - os: ubuntu-latest
            target: aarch64
            smoke: false # cross-built under emulation; foreign interpreter not run here
          - os: macos-14
            target: aarch64
            smoke: true
```
with:
```yaml
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64
            manylinux: auto
            smoke: true
          - os: ubuntu-latest
            target: aarch64
            manylinux: auto
            smoke: false # cross-built under emulation; foreign interpreter not run here
          - os: ubuntu-latest
            target: x86_64
            manylinux: musllinux_1_2
            smoke: false
            musl: true # import-smoke only in an Alpine container (native docker)
          - os: macos-13
            target: x86_64
            manylinux: auto
            smoke: true
          - os: macos-14
            target: aarch64
            manylinux: auto
            smoke: true
```

- [ ] **Step 3: Parameterize the maturin `manylinux` input**

Replace:
```yaml
      - uses: PyO3/maturin-action@v1
        with:
          target: ${{ matrix.target }}
          args: --release --locked --out dist
          manylinux: auto
```
with:
```yaml
      - uses: PyO3/maturin-action@v1
        with:
          target: ${{ matrix.target }}
          args: --release --locked --out dist
          # glibc legs use `auto` (newer glibc, fine for dev — release.yml pins 2014);
          # the musl leg builds musllinux_1_2; macOS ignores this (Linux-only).
          manylinux: ${{ matrix.manylinux }}
```

- [ ] **Step 4: Add the musl import-smoke step**

Immediately AFTER the existing `test the INSTALLED wheel (clean venv, full suite + stubtest)` step (the one gated `if: matrix.smoke`) and BEFORE the `build + install + smoke the sdist (fail hard)` step, insert:

```yaml
      - name: import-smoke the musl wheel (Alpine container)
        if: matrix.musl
        shell: bash
        run: |
          set -euxo pipefail
          # The full pytest+stubtest suite runs on glibc x86_64 + both macOS legs; this
          # leg just proves the wheel loads and links against musl libc. Native docker
          # (x86_64 host) — no QEMU.
          wheel="$(ls dist/*-cp311-abi3-*.whl)"
          docker run --rm -v "$PWD/dist:/dist:ro" python:3.11-alpine \
            sh -c "pip install /dist/$(basename "$wheel") && python -c 'import pygrit; pygrit.Repository'"
```

(The existing full-suite step stays gated `if: matrix.smoke` — the musl leg has `smoke: false`, so it correctly skips that heavier step and runs only the import-smoke above.)

- [ ] **Step 5: Tighten the sdist-step guard to the glibc x86_64 leg**

The one-shot sdist step is currently gated:
```yaml
      - name: build + install + smoke the sdist (fail hard)
        if: matrix.os == 'ubuntu-latest' && matrix.target == 'x86_64'
```
Both ubuntu-x86_64 legs (glibc + musl) now match. Restrict it to the glibc leg:
```yaml
      - name: build + install + smoke the sdist (fail hard)
        # glibc x86_64 leg only — the musl x86_64 leg also matches os+target.
        if: matrix.os == 'ubuntu-latest' && matrix.target == 'x86_64' && matrix.manylinux == 'auto'
```

- [ ] **Step 6: Disambiguate the artifact name**

Replace:
```yaml
      - uses: actions/upload-artifact@v7
        with:
          name: wheels-${{ matrix.os }}-${{ matrix.target }}
          path: dist
```
with:
```yaml
      - uses: actions/upload-artifact@v7
        with:
          name: wheels-${{ matrix.os }}-${{ matrix.target }}-${{ matrix.manylinux }}
          path: dist
```

- [ ] **Step 7: Update the header comment (macOS + coverage note)**

Replace the macOS best-effort bullet:
```
#   * macOS (macos-14 / Apple-silicon aarch64) is best-effort: grit-lib is
#     Unix-oriented and the deps are pure-Rust, so it is expected to build, but
#     fail-fast is disabled so a macOS hiccup does not mask Linux results.
```
with:
```
#   * macOS (macos-14 arm64 + macos-13 x86_64) is best-effort: grit-lib is
#     Unix-oriented and the deps are pure-Rust, so it is expected to build, but
#     fail-fast is disabled so a macOS hiccup does not mask Linux results.
#   * Wheel coverage mirrors most of release.yml's grid: linux glibc x86_64/aarch64,
#     linux musl x86_64 (Alpine import-smoke only), macOS x86_64 + arm64. The emulated
#     musl-aarch64 leg is release-only (kept off per-PR CI to avoid a second slow QEMU
#     build).
```

- [ ] **Step 8: Validate the YAML parses**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml')); print('ci.yml: valid YAML')"
```
Expected: `ci.yml: valid YAML` (no traceback).

- [ ] **Step 9: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: mirror Intel-Mac + musl-x86_64 wheel legs on PRs

Adds macos-13 (full suite) and musllinux_1_2 x86_64 (Alpine import-smoke)
to build-wheels; emulated musl-aarch64 stays release-only.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: README — platform table

**Files:**
- Modify: `README.md` (the "Supported Python / platforms" section)

- [ ] **Step 1: Update the platform bullets**

Replace:
```markdown
- **Linux/Unix** — x86_64 and aarch64.
- **macOS** — best-effort (Apple-silicon aarch64). grit-lib is Unix-oriented and
  its dependencies are pure-Rust, so it is expected to build.
- **Windows** — **deferred** until grit-lib gains Windows support (it currently
  depends on `libc`/`nix` and is Unix-oriented).
```
with:
```markdown
- **Linux (glibc)** — `manylinux_2_17` wheels for x86_64 and aarch64.
- **Linux (musl)** — `musllinux_1_2` wheels for x86_64 and aarch64 (Alpine and other
  musl-based distros / containers).
- **macOS** — x86_64 (Intel) and arm64 (Apple silicon). grit-lib is Unix-oriented and
  its dependencies are pure-Rust.
- **Windows** — **deferred** until grit-lib gains Windows support (it currently
  depends on `libc`/`nix` and is Unix-oriented).
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document the 6-platform wheel coverage

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Validate on CI (push + watch)

**This is a controller/validation step, not a code change.** The new Intel-Mac and musl-x86_64 wheels are only actually built/smoked when `ci.yml` runs, so push the branch and confirm the run is green before finishing.

- [ ] **Step 1: Push the branch**

```bash
git push -u origin wheel-coverage
```

- [ ] **Step 2: Watch the CI run to completion**

```bash
gh run list --branch wheel-coverage --limit 1
gh run watch "$(gh run list --branch wheel-coverage --limit 1 --json databaseId --jq '.[0].databaseId')" --exit-status
```
Expected: all jobs succeed —
- `lint`
- `test (3.11)`, `test (3.13)`
- `build-wheels (ubuntu-latest x86_64 auto)` (full suite + sdist)
- `build-wheels (ubuntu-latest aarch64 auto)` (build only)
- `build-wheels (ubuntu-latest x86_64 musllinux_1_2)` (Alpine import-smoke)
- `build-wheels (macos-13 x86_64 auto)` (full suite)
- `build-wheels (macos-14 aarch64 auto)` (full suite)

- [ ] **Step 3: If any leg fails, diagnose before merging**

The most likely first-run failures and where they point:
- musl build fails to find a Rust musl target → maturin-action musllinux container toolchain; check the `build-wheels (… musllinux_1_2)` log.
- `import pygrit` fails under Alpine → the abi3 `.so` doesn't load against musl libc (a real portability finding — surface it, do not silently drop the musl legs).
- macos-13 wheel/test failure → Intel-runner-specific; check the `build-wheels (macos-13 …)` log.

Do not proceed to finishing the branch until the run is green (or a failure is understood and the plan adjusted with the user).

---

## Self-Review

**1. Spec coverage:**
- 6-leg release matrix + smoke refactor → Task 2 ✓
- Artifact-name disambiguation (release) → Task 2 Step 1 (`…-${{ matrix.manylinux }}`) ✓
- ci.yml 5 legs + musl import-smoke + sdist guard + disambiguation → Task 3 ✓
- Inventory `EXPECTED_WHEELS` 3→6 + docstring + AIDEV-NOTE → Task 1 Step 3 ✓
- Inventory test 6-wheel fixture + count assert → Task 1 Step 1 ✓
- Docs/comment sweep (release header, both inventory step names, ci header, README) → Tasks 2/3/4 ✓
- Testing strategy (offline test + CI push validation) → Task 1 + Task 5 ✓
- Out of scope (Windows, full musl suite, ci musl-aarch64, binding code) → untouched ✓

**2. Placeholder scan:** No TBD/TODO; every code/YAML/edit step shows the exact content. ✓

**3. Consistency:** Matrix field names (`manylinux`, `smoke`, `image`, `platform`, `musl`) are used identically across the maturin step, smoke gates, sdist guard, job name, and artifact name. Every leg in both workflows defines `manylinux`, so `${{ matrix.manylinux }}` is never empty. The wheel filenames in the Task 1 fixture match the platform tags maturin produces for these targets, and all 6 are `-`-free in the platform field (so the checker's 5-part `-`-split holds). ✓
