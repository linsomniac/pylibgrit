# pygrit — Python bindings for grit-lib (design)

- **Date:** 2026-06-13 (revised after external Codex review)
- **Status:** Approved (design); ready for implementation planning
- **Author:** Sean Reifschneider (with Claude Code)

## 1. Summary

`pygrit` provides native Python bindings to [`grit-lib`](https://crates.io/crates/grit-lib),
the core Rust library of [gitbutlerapp/grit](https://github.com/gitbutlerapp/grit)
(a from-scratch reimplementation of Git in Rust). The bindings are built with
**PyO3** and packaged as a wheel with **maturin**. The first version is a
**thin, 1:1-style mapping** of grit-lib's **read-core** API and is tested with
pytest using a combination of differential ("oracle") tests against the real `git`
CLI and a set of mirrored grit-lib unit tests.

## 2. Goals and non-goals

### Goals (MVP / read-core)

- Open or discover a repository.
- Read objects by id: commit, tree, blob, tag.
- List and resolve references.
- Basic revision walk / log.
- Produce diffs (tree/content).
- Read configuration.
- Robust, well-isolated, well-tested code with type stubs for mypy.

### Non-goals (deferred to later milestones)

- Index / staging mutation; creating commits / writing objects.
- Merge, rebase, cherry-pick.
- Networking: fetch / push / transport.
- Blame, notes, reflog editing, hooks.
- Free-threaded ("no-GIL") CPython support (standard abi3 wheels do not load there).
- Windows support in the first release (grit-lib is Unix-oriented; see §8.1, §11).

## 3. Decisions (settled)

| Question | Decision |
| --- | --- |
| Binding mechanism | **Native PyO3 FFI** to `grit-lib` (in-process, no external binary at runtime) |
| Initial scope | **Read-core MVP** (see §2) |
| API style | **Thin 1:1-style mapping** of grit-lib names/types, as a documented Python façade (see §5) |
| Testing | **Oracle (vs real `git`) + mirrored grit-lib unit tests**; CI is mandatory (§7) |
| grit-lib dependency | **Strategy A** — pin the published `grit-lib` crate from crates.io, **exact** `=` pin + committed `Cargo.lock` |
| Python ABI | **abi3 with exact `abi3-py311`** (CPython 3.11+); free-threaded CPython excluded |
| Target platforms (v1) | **Linux/Unix (x86_64, aarch64)**; macOS best-effort; Windows deferred |
| License | **MIT** (match grit-lib; confirm exact license in spike) |

### Dependency strategy A (chosen), with fallback

Depend on the **published `grit-lib` crate** from crates.io with an **exact pin**
(`grit-lib = "=X.Y.Z"`), a **committed `Cargo.lock`**, and `cargo --locked` builds
for reproducibility. Pin PyO3, maturin, and the Rust toolchain versions too, and
build wheels/sdist in a clean environment.

If the spike (§8.1) shows the published crate is too old to expose the read-core
API, fall back to pinning a specific **git revision** of `gitbutlerapp/grit`
(strategy B). This is **not** a trivial change: a git dependency affects source
distributions, requires network access to build, weakens provenance, and prevents
publishing to PyPI from an sdist that depends on it — so it is a deliberate
fallback with its own checklist, not a one-liner. Record a pygrit ↔ grit-lib
version-compatibility note in the README.

## 4. Architecture and repository layout

Standard maturin **mixed** (Rust + Python) layout. The Rust binding layer is split
into one file per bound subsystem so each file stays small and focused, mirroring
grit-lib's own module boundaries.

```
pygrit/
├── Cargo.toml              # crate "pygrit": cdylib; module-name "pygrit._pygrit"
│                           #   deps: pyo3 (abi3-py311, exact pin), grit-lib (= pin)
├── Cargo.lock              # committed; builds use --locked
├── pyproject.toml          # build-backend = maturin; PEP 621 metadata; requires-python
├── src/                    # Rust binding layer (thin wrappers, no business logic)
│   ├── lib.rs              # #[pymodule] — registers classes/exceptions
│   ├── error.rs            # grit_lib::Error -> Python exception hierarchy (table-driven)
│   ├── repository.rs       # Repository: discover/open, refs, config, odb accessors
│   ├── odb.rs              # Odb.read(oid) -> Object; exists(oid)
│   ├── objects.rs          # ObjectId, ObjectKind, Object + Commit/Tree/TreeEntry/Blob/Tag
│   ├── refs.rs             # Reference + listing/resolution
│   ├── revwalk.rs          # revision walk / log iteration (owns traversal state)
│   └── diff.rs             # tree/content diff -> Diff / DiffEntry
├── python/
│   └── pygrit/
│       ├── __init__.py     # re-exports the native module's public symbols
│       ├── __init__.pyi    # hand-written type stubs (mypy coverage)
│       └── py.typed        # PEP 561 marker
└── tests/                  # pytest: oracle + mirrored units + fixtures
```

- The native extension is imported as `pygrit._pygrit` (explicit `module-name`);
  `python/pygrit/__init__.py` re-exports it so users write `import pygrit`.
- Ship `py.typed` so mypy consumes the stubs.
- Each Rust binding file is a *thin* wrapper: convert arguments, call grit-lib,
  convert results/errors. No domain logic lives in the binding layer.

## 5. Python API surface (provisional — pinned by the spike)

> **This section is provisional.** grit-lib's published API differs from a naive
> "all methods on `Repository`" mental model: several operations
> (resolve / rev-list / diff / ref / config helpers) are exposed as **module-level
> functions**, `Repository::open` takes a `(git_dir, work_tree)` pair, and
> `Odb::read` returns a raw object (`kind` + `data`) from which parsed
> `Commit`/`Tree`/`Tag` views are constructed in the **binding layer**. The spike
> (§8.1) MUST produce a **checked-in Rust→Python API matrix** with the exact
> grit-lib signatures, and the names below are then reconciled to it. We describe
> pygrit explicitly as a thin **Python façade** over grit-lib, not a literal 1:1
> re-export, so that constructing parsed views and grouping functions onto handles
> is in scope and not treated as drift.

### Repository
- `Repository.discover(path) -> Repository` — walk upward to find a repo.
- `Repository.open(git_dir, work_tree=None) -> Repository` — open explicit dirs.
- Properties: `.git_dir -> bytes-path`, `.work_tree -> bytes-path | None`, `.is_bare`.
- Accessors: `.odb -> Odb`, `.config -> ConfigSet`.
- `.references() -> Iterator[Reference]`.
- `.resolve(spec) -> ObjectId` — resolve a revision/refspec to an id.
- `.revwalk(start, *, order=...) -> Iterator[Commit]`.
- `.diff(a, b, *, options=...) -> Diff`.

### Objects
- `ObjectId` — construct from hex/bytes; `.hex`, `.raw`, `.kind_hint`,
  `.hash_algorithm` (SHA-1 / SHA-256); `__eq__`, `__hash__`, `__repr__`.
- `ObjectKind` — enum: `COMMIT`, `TREE`, `BLOB`, `TAG`.
- `Object` — raw view: `.id`, `.kind`, `.data -> bytes`; typed views built on top:
  - `Commit`: `.tree`, `.parents`, `.author`, `.committer`, `.message_bytes`,
    `.message(encoding=...)`.
  - `Tree`: `Iterator[TreeEntry]`.
  - `TreeEntry`: `.name -> bytes`, `.mode`, `.id`, `.kind` (a class, not a tuple).
  - `Blob`: `.data -> bytes`.
  - `Tag`: `.target`, `.name -> bytes`, `.tagger`, `.message_bytes`.
- `Signature` (author/committer/tagger): `.name -> bytes`, `.email -> bytes`,
  `.when` (timestamp + offset), with optional decoded `.name_str`/`.email_str`.

### Odb
- `Odb.read(oid) -> Object`; `Odb.exists(oid) -> bool`.

### References
- `Reference`: `.name -> bytes`, `.target -> ObjectId | None`,
  `.symbolic_target -> bytes | None`, `.is_symbolic`, `.peel() -> ObjectId`.

### Diff
- `Diff`: `Iterator[DiffEntry]`, plus diffstat summary.
- `DiffEntry`: `.old_path -> bytes`, `.new_path -> bytes`, `.status`, `.old_id`,
  `.new_id`, hunks/patch bytes.

### Config
- `ConfigSet.get_str/get_bool/get_int(key, *, scope=...) -> ... | None`.

### Byte / text policy (decided up front)

Git stores tree names, identities, messages, and ref/diff paths as **byte
sequences**; OS paths are a separate concern. The binding therefore:

- Accepts **path inputs** as `str | bytes | os.PathLike`, preserving
  surrogate-escaped paths (`os.fsencode`/`fsdecode` semantics) where grit-lib
  permits; documents any grit-lib API that only accepts `str`/`String` as a known
  limitation.
- Returns **tree/diff paths and raw commit/tag/identity fields as `bytes`**.
- Offers **optional decoded text accessors** (`*_str` / `message(encoding=...)`)
  with an explicit encoding and error policy (default `utf-8`, `errors="strict"`,
  caller-overridable).

## 6. Ownership, lifetime, and concurrency (FFI safety)

### Ownership / lifetimes
- Python-visible classes must **own or share** their data — no borrowed Rust
  references cross the FFI boundary.
- `Repository` is wrapped in an `Arc`; child handles (`Odb`, `ConfigSet`,
  iterators, parsed objects) hold their own `Arc` clone so they remain valid after
  the parent Python object is dropped.
- Object/blob buffers are **owned** (or `Arc<[u8]>`); iterators (`references`,
  `revwalk`, `Tree`, `Diff`) **own their traversal state**.
- Tests must delete the parent `Repository` while children/iterators are still in
  use and assert continued validity (no use-after-free, no panic).

### GIL / concurrency
- Release the GIL (`Python::allow_threads`) around potentially blocking or
  CPU-heavy grit-lib calls (object/pack reads, revwalk, diff) **after** extracting
  and owning all Python inputs, reacquiring it only to convert results.
- Document thread-safety and the snapshot/consistency guarantees, including that
  another process mutating the repo concurrently is out of scope for consistency.
- Add tests: concurrent reads from multiple Python threads; verify GIL is actually
  released (e.g. measurable parallel speedup or a blocking-call probe).

## 7. Error handling and testing

### Error handling
- Base exception `pygrit.GritError`, with subclasses chosen to be **mutually
  exclusive** (resolving the earlier repo-not-found overlap):
  - `RepositoryError` — discover/open/config-load failures (incl. "no repository
    found").
  - `ObjectNotFoundError` — a requested object/ref id does not exist.
  - `InvalidObjectError` — malformed id or corrupt/undecodable object.
  - `GritError` — fallback for any unmapped grit-lib error (always reachable).
- The spike produces an **error-mapping table** covering every exposed operation →
  exception, preserving the offending path/OID and the source error message
  (`__cause__`). Use `ValueError` for bad argument types/shapes and `OSError`
  (with `errno`) for filesystem failures where appropriate.
- **Binding code must be panic-free**; `catch_unwind` at the boundary is a
  last-resort backstop that maps to `GritError`, not the primary strategy. Tests
  feed malformed ids and corrupt objects and assert clean exceptions, never aborts.

### Testing strategy (oracle + mirrored units; CI mandatory)

- **Hermetic fixtures:** build temp repos with the real `git` CLI (`git 2.53.0`),
  under an isolated `HOME`/`GIT_CONFIG_GLOBAL`/`GIT_CONFIG_NOSYSTEM`, fixed
  `TZ=UTC`, `LC_ALL=C`, and deterministic author/committer dates. Cover loose and
  packed objects, packed refs, and deltas.
- **Robust oracle output:** compare against **machine-readable** git output, not
  human formatting — `git cat-file --batch`/`--batch-check`, `git rev-parse`,
  `git for-each-ref -z`, `git ls-tree -z`, `git diff-tree`/`git diff --raw -z`,
  `git config --get` — so binary and non-UTF-8 data round-trip safely.
- **Mirrored unit tests:** port representative grit-lib Rust unit tests into pytest
  (oid hex round-trip, object-kind classification, ref-name validation).
- **Edge cases:** empty repo, detached HEAD, bare repos, gitfile/worktree links,
  packed vs loose objects and ref deltas, binary blobs, non-UTF-8 paths/messages,
  symbolic refs, and (where the toolchain supports it) **SHA-256** repositories.
- **Test the shipped artifact, not just `maturin develop`:** CI builds the wheel
  and sdist, installs the **wheel** into a clean venv, verifies wheel platform/abi
  tags, builds-and-installs from the sdist, and runs `stubtest` against `.pyi`.
- **CI matrix (mandatory, not optional):** GitHub Actions building wheels with
  maturin across the supported Python range (oldest = 3.11, newest = current) and
  each supported platform/arch (Linux x86_64 + aarch64; macOS best-effort), with a
  cargo cache; runs pytest, mypy, `stubtest`, `ruff`, `cargo fmt --check`, and
  `cargo clippy`.

## 8. Milestones

### 8.1 Build spike (de-risk first) — required exit criteria
Install `maturin` (`uv tool install maturin`); add the exact-pinned `grit-lib`
dependency; build a minimal `#[pymodule]` that does `Repository.discover()` and
reads the `HEAD` commit. Before leaving the spike, produce/record:
1. A **checked-in Rust→Python API matrix** with exact grit-lib signatures for the
   read-core surface (§5 reconciled to reality).
2. `cargo tree -e features` output and the **actual feature flags** (do not assume
   a `transport`/`default` toggle exists — current grit-lib appears to expose only
   `test-tools`; confirm what, if anything, can be disabled).
3. Confirmed **platform scope** by building for each intended target; note any
   Unix-only / `-sys` (`pkg-config`) dependencies and install `pkg-config` if
   required.
4. The **exact crate version**, committed `Cargo.lock`, and grit-lib's **license**.
**Exit criteria:** wheel builds with `--locked`, the read works from Python, and
items 1–4 are committed.

### 8.2 Object model + odb read (with tests)
`ObjectId`, `ObjectKind`, `Object`/`Commit`/`Tree`/`TreeEntry`/`Blob`/`Tag`,
`Signature`, `Odb.read/exists`; byte/text policy implemented.

### 8.3 References + resolve
`Reference`, `references()`, `resolve(spec)`, symbolic-ref handling.

### 8.4 Revwalk / log
`revwalk(...)` with ordering options; owned traversal state.

### 8.5 Diff
`diff(a, b)` → `Diff`/`DiffEntry`, diffstat.

### 8.6 Config + stubs + CI polish
`ConfigSet`, finalize `__init__.pyi` + `py.typed`, mypy/`stubtest` clean, CI matrix
green, README with version-compatibility note.

## 9. Tooling and conventions

- **Python:** `ruff format`; type annotations everywhere; `mypy` + `stubtest` clean
  against the stubs; `uv` for env/deps (no pip/poetry/requirements.txt).
- **Rust:** `cargo fmt`, `cargo clippy`, `cargo build --locked`.
- **Build:** maturin (mixed layout, exact `abi3-py311`), `py.typed` shipped,
  PEP 621 metadata, explicit `module-name = "pygrit._pygrit"`.
- **CI:** mandatory (see §7) — wheel/sdist build + install-and-test of the shipped
  artifacts across the supported Python/platform matrix.

## 10. Toolchain status (as of 2026-06-13)

- ✅ `rustc 1.94.1`, `cargo 1.94.0` (source-tarball install; no `rustup` — not required).
- ✅ `gcc 15.2.0` (linker).
- ✅ `uv 0.11.14`, Python 3.13.12, `git 2.53.0`.
- ⚠️ `maturin` — to be installed via `uv tool install maturin` (spike step).
- ⚠️ `pkg-config` — install only if grit-lib pulls a C-backed `-sys` dependency
  (confirmed via `cargo tree` in the spike).

## 11. Open items / defaults

- **License:** confirm grit-lib's license during the spike; default `pygrit` to the
  same (expected MIT).
- **Minimum Python:** `abi3-py311` (3.11+). Adjustable, but not below 3.11 — 3.9
  is EOL (Oct 2025) and 3.10 nears EOL; no concrete requirement for older.
- **Platforms:** Linux/Unix first; macOS best-effort; **Windows deferred** until
  grit-lib's Windows support lands.
- **SHA-256 repositories:** support is desirable for read-core; gated on grit-lib
  capability and confirmed in the spike — tested where the toolchain allows.
- **grit-lib version pin:** exact `=` version + `Cargo.lock`, recorded after the spike.
```
