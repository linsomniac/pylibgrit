# pylibgrit Write-Core Phase A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local object/ref write surface to pylibgrit (write objects, stage an index, write trees, create commit/tag objects, mutate refs) as thin wrappers over grit-lib 0.4.1 plumbing.

**Architecture:** Extend the existing OO façade. New write methods hang off `Repository` and a new `Index` object; each assembles a grit-lib plumbing workflow in Rust (`examples/commit_tree.rs` pattern). All writes release the GIL via `allow_threads`. No new crates, no Cargo features, no network.

**Tech Stack:** Rust + PyO3 0.23.3 (abi3-py311), maturin, grit-lib 0.4.1; Python 3.11+; pytest oracle tests against real `git`; mypy + stubtest gates.

**Source spec:** `docs/superpowers/specs/2026-06-14-pylibgrit-write-core-design.md` (read it for rationale; this plan is self-contained for execution).

---

## Conventions for every task

- **Branch:** `write-core-phase-a` (already checked out).
- **Build before testing Rust changes:** `uv run maturin develop --uv --locked` recompiles the extension into the venv. You MUST run it after any `src/*.rs` change before `pytest` will see the change.
- **Run one test file:** `uv run pytest tests/<file>.py -v`
- **Anchor comments:** This codebase uses `AIDEV-NOTE:` comments for non-obvious code (see existing `src/*.rs`). Add them where this plan's code is subtle. Do not remove existing `AIDEV-` comments.
- **Bytes policy:** paths, ref names, messages, identities cross the boundary as `bytes` (design §5). Ref names / encodings / tag fields that grit-lib types as `&str`/`String` are decoded from those bytes and a non-UTF-8 value raises (documented per method).
- **PyO3 single-impl constraint:** the `multiple-pymethods` feature is NOT enabled, so ALL `Repository` methods live in the ONE `#[pymethods] impl Repository` block in `src/repository.rs`. Tasks that add `Repository` methods append to that block.

## File structure (what changes, and why)

- `src/odb.rs` — extend `Odb` with `write`/`hash` (object writing).
- `src/objects.rs` — make `Signature` constructable + `.raw`; add shared helpers `py_to_kind`, `resolve_ident`, `tz/wire` formatters used by builders.
- `src/index.rs` — **new file.** `Index` pyclass (`Mutex<grit_lib::index::Index>` + `Arc<Repository>`), `IndexEntry` pyclass, `IndexEntryIter`.
- `src/refs.rs` — add free helpers `read_current_oid` (best-effort CAS read) and `zero_like` (width-matched zero oid). The `Reference` pyclass stays as-is.
- `src/repository.rs` — make `extract_path` `pub(crate)`; add `index()`, `create_commit()`, `create_tag()`, `update_ref()`, `delete_ref()`, `set_head()`, `set_symbolic_ref()`, `append_reflog()` to the single `Repository` impl block.
- `src/error.rs` — add `RefMismatchError(GritError)` + register it.
- `src/lib.rs` — register `Index`, `IndexEntry`, `IndexEntryIter`.
- `python/pylibgrit/__init__.py` — re-export `Index`, `IndexEntry`, `RefMismatchError`; extend `__all__`.
- `python/pylibgrit/__init__.pyi` — stubs for every new symbol (kept in exact sync; `stubtest` gate).
- `tests/test_*.py` — new oracle tests per task.

---

### Task 1: Odb object writing (`write`, `hash`)

**Files:**
- Modify: `src/objects.rs` (add `pub(crate) fn py_to_kind`)
- Modify: `src/odb.rs` (add `write`, `hash`)
- Modify: `python/pylibgrit/__init__.pyi` (Odb stubs)
- Test: `tests/test_odb_write.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_odb_write.py`:

```python
import subprocess

import pytest

from tests.gitlib import cat_file_data, cat_file_type


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def test_write_blob_matches_git_hash_object(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    # git's oracle oid for the same bytes
    git_oid = subprocess.run(
        ["git", "hash-object", "-w", "--stdin"],
        cwd=repo, env=git_env, input=b"hello\n",
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()

    pg = pylibgrit.Repository.open(str(repo / ".git"))
    oid = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")
    assert oid.hex == git_oid
    # and it is actually on disk / readable
    assert pg.odb.read(oid).data == b"hello\n"
    assert cat_file_type(repo, oid.hex) == "blob"
    assert cat_file_data(repo, oid.hex) == b"hello\n"


def test_hash_computes_without_writing(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    oid = pg.odb.hash(pylibgrit.ObjectKind.BLOB, b"nope\n")
    assert pg.odb.exists(oid) is False  # hash() must not write


def test_write_is_idempotent(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    a = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"dup\n")
    b = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"dup\n")
    assert a == b
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_odb_write.py -v`
Expected: FAIL — `Odb` has no attribute `write`.

- [ ] **Step 3: Add `py_to_kind` to `src/objects.rs`**

After `kind_to_py` (around line 319), add:

```rust
// AIDEV-NOTE: Inverse of kind_to_py: map a public pylibgrit.ObjectKind IntEnum member
// (an int subclass) back to grit_lib's ObjectKind. The integer values MUST match
// object_kind_discriminant()/the IntEnum in __init__.py (asserted by tests).
pub(crate) fn py_to_kind(obj: &Bound<'_, PyAny>) -> PyResult<grit_lib::objects::ObjectKind> {
    let v: i32 = obj.extract()?;
    match v {
        0 => Ok(grit_lib::objects::ObjectKind::Commit),
        1 => Ok(grit_lib::objects::ObjectKind::Tree),
        2 => Ok(grit_lib::objects::ObjectKind::Blob),
        3 => Ok(grit_lib::objects::ObjectKind::Tag),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "invalid ObjectKind value: {other}"
        ))),
    }
}
```

- [ ] **Step 4: Add `write`/`hash` to `src/odb.rs`**

Inside `#[pymethods] impl Odb`, after `exists`:

```rust
    // AIDEV-NOTE: Write a loose object and return its oid. `data: Vec<u8>` owns a copy so it
    // can move into the allow_threads closure (the write decompress/hash/IO is released off the
    // GIL). On-disk effect is immediate (atomic temp-file + rename; loose objects are 0o444).
    // Re-writing an existing object is a no-op "freshen", not an error (git semantics).
    fn write(
        &self,
        py: Python<'_>,
        kind: &Bound<'_, PyAny>,
        data: Vec<u8>,
    ) -> PyResult<ObjectId> {
        let k = crate::objects::py_to_kind(kind)?;
        let oid = py
            .allow_threads(|| self.repo.odb.write(k, &data))
            .map_err(map_err)?;
        Ok(ObjectId::from_inner(oid))
    }

    // AIDEV-NOTE: Compute an object's oid WITHOUT writing it (git hash-object without -w).
    // grit-lib's Odb::hash is infallible and auto-detects the repo's SHA-1/SHA-256 algo.
    fn hash(&self, py: Python<'_>, kind: &Bound<'_, PyAny>, data: Vec<u8>) -> PyResult<ObjectId> {
        let k = crate::objects::py_to_kind(kind)?;
        let oid = py.allow_threads(|| self.repo.odb.hash(k, &data));
        Ok(ObjectId::from_inner(oid))
    }
```

- [ ] **Step 5: Update `python/pylibgrit/__init__.pyi`**

In `class Odb`, add above `read`:

```python
    def write(self, kind: ObjectKind, data: bytes) -> ObjectId: ...
    def hash(self, kind: ObjectKind, data: bytes) -> ObjectId: ...
```

- [ ] **Step 6: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_odb_write.py -v`
Expected: PASS (3 tests).

- [ ] **Step 7: Commit**

```bash
git add src/objects.rs src/odb.rs python/pylibgrit/__init__.pyi tests/test_odb_write.py
git commit -m "feat: Odb.write/hash object writing"
```

---

### Task 2: Constructable `Signature` + `.raw`

**Files:**
- Modify: `src/objects.rs` (add `#[new]`, `raw` getter, `pub(crate) fn wire_bytes`, `fn format_tz_offset`)
- Modify: `python/pylibgrit/__init__.pyi` (Signature ctor + raw)
- Test: `tests/test_signature.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_signature.py`:

```python
def test_signature_wire_format():
    import pylibgrit

    sig = pylibgrit.Signature(b"Ada Lovelace", b"ada@example.com", (1718000000, 0))
    assert sig.name == b"Ada Lovelace"
    assert sig.email == b"ada@example.com"
    assert sig.when == (1718000000, 0)
    assert sig.raw == b"Ada Lovelace <ada@example.com> 1718000000 +0000"


def test_signature_positive_and_negative_tz():
    import pylibgrit

    east = pylibgrit.Signature(b"E", b"e@x", (1, 19800))   # +05:30
    west = pylibgrit.Signature(b"W", b"w@x", (1, -28800))   # -08:00
    assert east.raw == b"E <e@x> 1 +0530"
    assert west.raw == b"W <w@x> 1 -0800"
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_signature.py -v`
Expected: FAIL — `Signature` is not constructable (no `__new__`).

- [ ] **Step 3: Add constructor, `raw`, and helpers to `src/objects.rs`**

In `#[pymethods] impl Signature`, add at the top of the block:

```rust
    // AIDEV-NOTE: Write-side constructor. `when` is (unix_seconds, utc_offset_seconds); the
    // offset is signed and in SECONDS (e.g. +05:30 -> 19800). name/email are raw bytes for
    // non-UTF-8 fidelity (design §5). The Git wire form is produced by `wire_bytes`/`raw`.
    #[new]
    #[pyo3(signature = (name, email, when))]
    fn new(name: Vec<u8>, email: Vec<u8>, when: (i64, i32)) -> Self {
        Self {
            name,
            email,
            when_secs: when.0,
            when_offset_secs: when.1,
        }
    }

    /// The Git wire ident bytes: `Name <email> <unix-seconds> <+HHMM>`.
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.wire_bytes())
    }
```

In the plain `impl Signature` block (after `parse`), add:

```rust
    // AIDEV-NOTE: Serialize this identity to Git wire form. Used by `raw` and by the commit/
    // tag builders (which place these exact bytes into CommitData.author_raw / the tag tagger
    // header) so produced object OIDs are byte-identical to git's.
    pub(crate) fn wire_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.name);
        out.extend_from_slice(b" <");
        out.extend_from_slice(&self.email);
        out.extend_from_slice(b"> ");
        out.extend_from_slice(self.when_secs.to_string().as_bytes());
        out.push(b' ');
        out.extend_from_slice(format_tz_offset(self.when_offset_secs).as_bytes());
        out
    }
```

At module scope (near `split_name_email`), add:

```rust
// AIDEV-NOTE: Format a signed second-offset as Git's `+HHMM`/`-HHMM` timezone field.
// e.g. 0 -> "+0000", 19800 -> "+0530", -28800 -> "-0800".
fn format_tz_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("{sign}{:02}{:02}", a / 3600, (a % 3600) / 60)
}
```

- [ ] **Step 4: Update `python/pylibgrit/__init__.pyi`**

In `class Signature`, add above the existing `name` property:

```python
    def __init__(self, name: bytes, email: bytes, when: tuple[int, int]) -> None: ...
    @property
    def raw(self) -> bytes: ...
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_signature.py -v`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/objects.rs python/pylibgrit/__init__.pyi tests/test_signature.py
git commit -m "feat: constructable Signature with .raw wire bytes"
```

---

### Task 3: `IndexEntry` pyclass

**Files:**
- Create: `src/index.rs`
- Modify: `src/lib.rs` (`mod index;` + register classes)
- Modify: `python/pylibgrit/__init__.py` (import + `__all__`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_index_entry.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_index_entry.py`:

```python
def test_index_entry_minimal():
    import pylibgrit

    oid = pylibgrit.ObjectId.from_hex("0" * 40)
    e = pylibgrit.IndexEntry(b"a.txt", oid, 0o100644)
    assert e.path == b"a.txt"
    assert e.oid == oid
    assert e.mode == 0o100644
    assert e.size == 0
    assert e.ctime == (0, 0)


def test_index_entry_full_fields():
    import pylibgrit

    oid = pylibgrit.ObjectId.from_hex("1" * 40)
    e = pylibgrit.IndexEntry(
        b"src/x", oid, 0o100755,
        ctime=(11, 12), mtime=(13, 14), dev=5, ino=6, uid=7, gid=8, size=9, flags=3,
    )
    assert (e.ctime, e.mtime, e.dev, e.ino, e.uid, e.gid, e.size, e.flags) == (
        (11, 12), (13, 14), 5, 6, 7, 8, 9, 3,
    )
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_entry.py -v`
Expected: FAIL — module has no `IndexEntry`.

- [ ] **Step 3: Create `src/index.rs` with the `IndexEntry` pyclass**

```rust
//! Python wrappers over grit-lib's index (`Index`, `IndexEntry`) write surface.

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::objects::ObjectId;

// AIDEV-NOTE: Wraps grit_lib::index::IndexEntry (a 15-field struct). The constructor exposes
// the settable stat/mode/oid/path/flags subset; flags_extended is always None and
// base_index_pos always 0 (split-index is not a Phase A concern). `flags` defaults to 0; the
// index serializer recomputes the low 12 bits (path length) on write, so 0 is safe for a
// normal stage-0 entry.
#[pyclass(module = "pylibgrit._pylibgrit")]
pub struct IndexEntry {
    pub(crate) inner: grit_lib::index::IndexEntry,
}

#[pymethods]
impl IndexEntry {
    #[new]
    #[pyo3(signature = (path, oid, mode, *, ctime=(0, 0), mtime=(0, 0),
                        dev=0, ino=0, uid=0, gid=0, size=0, flags=0))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        path: Vec<u8>,
        oid: ObjectId,
        mode: u32,
        ctime: (u32, u32),
        mtime: (u32, u32),
        dev: u32,
        ino: u32,
        uid: u32,
        gid: u32,
        size: u32,
        flags: u16,
    ) -> Self {
        Self {
            inner: grit_lib::index::IndexEntry {
                ctime_sec: ctime.0,
                ctime_nsec: ctime.1,
                mtime_sec: mtime.0,
                mtime_nsec: mtime.1,
                dev,
                ino,
                mode,
                uid,
                gid,
                size,
                oid: oid.inner(),
                flags,
                flags_extended: None,
                path,
                base_index_pos: 0,
            },
        }
    }

    #[getter]
    fn path<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.path)
    }
    #[getter]
    fn oid(&self) -> ObjectId {
        ObjectId::from_inner(self.inner.oid)
    }
    #[getter]
    fn mode(&self) -> u32 {
        self.inner.mode
    }
    #[getter]
    fn ctime(&self) -> (u32, u32) {
        (self.inner.ctime_sec, self.inner.ctime_nsec)
    }
    #[getter]
    fn mtime(&self) -> (u32, u32) {
        (self.inner.mtime_sec, self.inner.mtime_nsec)
    }
    #[getter]
    fn dev(&self) -> u32 {
        self.inner.dev
    }
    #[getter]
    fn ino(&self) -> u32 {
        self.inner.ino
    }
    #[getter]
    fn uid(&self) -> u32 {
        self.inner.uid
    }
    #[getter]
    fn gid(&self) -> u32 {
        self.inner.gid
    }
    #[getter]
    fn size(&self) -> u32 {
        self.inner.size
    }
    #[getter]
    fn flags(&self) -> u16 {
        self.inner.flags
    }
}

// AIDEV-NOTE: The Index pyclass and its helpers are added in Tasks 4–7; this file is the single
// home for the index write surface.
```

> The `Index` struct (Task 4) brings its own imports (`std::sync::{Arc, Mutex}`, `crate::error::map_err`). Task 3 imports only what `IndexEntry` needs, so the file compiles cleanly on its own (`cargo`/`maturin` allow unused-import warnings during a build; there are none here anyway).

- [ ] **Step 4: Register in `src/lib.rs`**

Add `mod index;` after `mod error;` (keep alphabetical-ish order: `mod diff; mod error; mod index; mod objects;`). In `fn _pylibgrit`, after the `objects::*` classes, add:

```rust
    m.add_class::<index::IndexEntry>()?;
```

- [ ] **Step 5: Re-export in `python/pylibgrit/__init__.py`**

Add `IndexEntry,` to the `from pylibgrit._pylibgrit import (...)` block and to `__all__` (keep both alphabetical).

- [ ] **Step 6: Add stub in `python/pylibgrit/__init__.pyi`**

Add `"IndexEntry",` to `__all__`, and after the `Signature` class add:

```python
@final
class IndexEntry:
    def __init__(self, path: bytes, oid: ObjectId, mode: int, *,
                 ctime: tuple[int, int] = ..., mtime: tuple[int, int] = ...,
                 dev: int = 0, ino: int = 0, uid: int = 0, gid: int = 0,
                 size: int = 0, flags: int = 0) -> None: ...
    @property
    def path(self) -> bytes: ...
    @property
    def oid(self) -> ObjectId: ...
    @property
    def mode(self) -> int: ...
    @property
    def ctime(self) -> tuple[int, int]: ...
    @property
    def mtime(self) -> tuple[int, int]: ...
    @property
    def dev(self) -> int: ...
    @property
    def ino(self) -> int: ...
    @property
    def uid(self) -> int: ...
    @property
    def gid(self) -> int: ...
    @property
    def size(self) -> int: ...
    @property
    def flags(self) -> int: ...
```

- [ ] **Step 7: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_entry.py -v`
Expected: PASS (2 tests).

- [ ] **Step 8: Commit**

```bash
git add src/index.rs src/lib.rs python/pylibgrit/__init__.py python/pylibgrit/__init__.pyi tests/test_index_entry.py
git commit -m "feat: constructable IndexEntry pyclass"
```

---

### Task 4: `Index` — load, mutate, persist (`repo.index`, `add`, `add_entry`, `remove`, `write`)

**Files:**
- Modify: `src/index.rs` (add `Index` pyclass)
- Modify: `src/repository.rs` (make `extract_path` `pub(crate)`; add `index()`)
- Modify: `src/lib.rs` (register `Index`)
- Modify: `python/pylibgrit/__init__.py`, `python/pylibgrit/__init__.pyi`
- Test: `tests/test_index_write.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_index_write.py`:

```python
import subprocess


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _ls_files_stage(repo, env):
    return subprocess.run(
        ["git", "ls-files", "--stage"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode()


def test_index_add_and_write_persists(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")

    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()

    staged = _ls_files_stage(repo, git_env)
    assert blob.hex in staged
    assert "a.txt" in staged
    assert staged.startswith("100644 ")


def test_index_remove(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    assert idx.remove(b"a.txt") is True
    assert idx.remove(b"a.txt") is False
    idx.write()
    assert _ls_files_stage(repo, git_env).strip() == ""


def test_index_add_entry_raw(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"y\n")
    idx = pg.index()
    idx.add_entry(pylibgrit.IndexEntry(b"b.txt", blob, 0o100644))
    idx.write()
    assert "b.txt" in _ls_files_stage(repo, git_env)
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -v`
Expected: FAIL — `Repository` has no `index`.

- [ ] **Step 3: Make `extract_path` reusable in `src/repository.rs`**

Change its declaration from `fn extract_path(` to `pub(crate) fn extract_path(`.

- [ ] **Step 4: Add the `Index` pyclass to `src/index.rs`**

First add the imports it needs to the top of the file (under the existing `use` lines):

```rust
use std::sync::{Arc, Mutex};

use crate::error::map_err;
```

Then add:

```rust
// AIDEV-NOTE: `Index` owns a grit_lib::index::Index behind a Mutex (binding-owned mutable
// value; grit's Index mutators take &mut self) plus an Arc<Repository> so write_tree can reach
// the odb and write() can target the repo's default index path. Index methods run UNDER the GIL:
// a std MutexGuard is !Send and cannot be held across allow_threads, and Phase A index ops are
// fast enough that this is fine. (stage()'s odb blob write does release the GIL — it never holds
// the guard during the heavy work; see Task 6.)
#[pyclass(module = "pylibgrit._pylibgrit")]
pub struct Index {
    inner: Mutex<grit_lib::index::Index>,
    repo: Arc<grit_lib::repo::Repository>,
}

impl Index {
    pub fn new_loaded(inner: grit_lib::index::Index, repo: Arc<grit_lib::repo::Repository>) -> Self {
        Self {
            inner: Mutex::new(inner),
            repo,
        }
    }
}

#[pymethods]
impl Index {
    // AIDEV-NOTE: Add a synthetic entry (blob already in the odb). Stat fields are zeroed (the
    // commit_tree.rs pattern); `flags` carries the path length so the in-memory entry is
    // well-formed, though the writer recomputes it. add_or_replace upserts by (path, stage 0).
    fn add(&self, path: Vec<u8>, oid: ObjectId, mode: u32) {
        let entry = grit_lib::index::IndexEntry {
            ctime_sec: 0,
            ctime_nsec: 0,
            mtime_sec: 0,
            mtime_nsec: 0,
            dev: 0,
            ino: 0,
            mode,
            uid: 0,
            gid: 0,
            size: 0,
            oid: oid.inner(),
            flags: (path.len().min(0xFFF)) as u16,
            flags_extended: None,
            path,
            base_index_pos: 0,
        };
        self.inner.lock().unwrap().add_or_replace(entry);
    }

    fn add_entry(&self, entry: PyRef<'_, IndexEntry>) {
        self.inner.lock().unwrap().add_or_replace(entry.inner.clone());
    }

    fn remove(&self, path: Vec<u8>) -> bool {
        self.inner.lock().unwrap().remove(&path)
    }

    // AIDEV-NOTE: Persist the index. `path=None` writes the repo's default index (via
    // Repository::write_index, which honors sparse-index collapsing); an explicit path uses
    // Index::write directly. Runs under the GIL — a std MutexGuard is !Send so it cannot be held
    // across allow_threads, and index serialization is fast enough that this is fine for Phase A.
    #[pyo3(signature = (path=None))]
    fn write(&self, path: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        match path {
            None => {
                let mut guard = self.inner.lock().unwrap();
                self.repo.write_index(&mut guard).map_err(map_err)
            }
            Some(p) => {
                let pathbuf = crate::repository::extract_path(p)?;
                let guard = self.inner.lock().unwrap();
                guard.write(&pathbuf).map_err(map_err)
            }
        }
    }
}
```

- [ ] **Step 5: Add `index()` to the `Repository` impl in `src/repository.rs`**

Inside the single `#[pymethods] impl Repository` block (e.g. after `references`):

```rust
    // AIDEV-NOTE: Load the repo's index into a binding-owned, mutable Index. If no index file
    // exists yet (fresh repo / bare repo before any staging), start from an empty Index rather
    // than erroring. We check the conventional `<git_dir>/index` path; GIT_INDEX_FILE overrides
    // are not honored here (Phase A limitation). The load releases the GIL.
    fn index(&self, py: Python<'_>) -> PyResult<crate::index::Index> {
        let index_path = self.inner.git_dir.join("index");
        let loaded = if index_path.exists() {
            py.allow_threads(|| self.inner.load_index()).map_err(map_err)?
        } else {
            grit_lib::index::Index::new()
        };
        Ok(crate::index::Index::new_loaded(
            loaded,
            Arc::clone(&self.inner),
        ))
    }
```

- [ ] **Step 6: Register + export**

`src/lib.rs`: add `m.add_class::<index::Index>()?;` next to `IndexEntry`.
`python/pylibgrit/__init__.py`: add `Index,` to the import block and `__all__`.

- [ ] **Step 7: Add stubs in `python/pylibgrit/__init__.pyi`**

Add `"Index",` to `__all__`. After `class IndexEntry`, add:

```python
@final
class Index:
    def add(self, path: bytes, oid: ObjectId, mode: int) -> None: ...
    def add_entry(self, entry: IndexEntry) -> None: ...
    def remove(self, path: bytes) -> bool: ...
    def write(self, path: bytes | os.PathLike[str] | None = None) -> None: ...
```

In `class Repository`, add (after `references`):

```python
    def index(self) -> Index: ...
```

- [ ] **Step 8: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -v`
Expected: PASS (3 tests).

- [ ] **Step 9: Commit**

```bash
git add src/index.rs src/repository.rs src/lib.rs python/pylibgrit/__init__.py python/pylibgrit/__init__.pyi tests/test_index_write.py
git commit -m "feat: Index load/add/remove/write + repo.index()"
```

---

### Task 5: `Index.write_tree`

**Files:**
- Modify: `src/index.rs` (add `write_tree`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_index_write.py` (append)

- [ ] **Step 1: Write the failing test (append to `tests/test_index_write.py`)**

```python
def test_write_tree_matches_git(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")

    idx = pg.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    tree = idx.write_tree()

    git_tree = subprocess.run(
        ["git", "write-tree"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert tree.hex == git_tree
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py::test_write_tree_matches_git -v`
Expected: FAIL — `Index` has no `write_tree`.

- [ ] **Step 3: Add `write_tree` to `#[pymethods] impl Index` in `src/index.rs`**

```rust
    // AIDEV-NOTE: Build a tree object from the current in-memory index and return its oid
    // (== `git write-tree`). prefix="" means the whole index from the root; writes the tree (and
    // any sub-trees) into the odb. Runs under the GIL — the MutexGuard is !Send so it cannot
    // cross allow_threads. (A future optimization could clone the Index to release the GIL.)
    fn write_tree(&self) -> PyResult<ObjectId> {
        let guard = self.inner.lock().unwrap();
        let oid = grit_lib::write_tree::write_tree_from_index(&self.repo.odb, &guard, "")
            .map_err(map_err)?;
        Ok(ObjectId::from_inner(oid))
    }
```

- [ ] **Step 4: Stub**

In `class Index` (`.pyi`), add after `write`:

```python
    def write_tree(self) -> ObjectId: ...
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -v`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/index.rs python/pylibgrit/__init__.pyi tests/test_index_write.py
git commit -m "feat: Index.write_tree"
```

---

### Task 6: `Index.stage` (hash a real working-tree file)

**Files:**
- Modify: `src/index.rs` (add `stage` + `mode_from_metadata` + `io_err` helpers)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_index_write.py` (append)

- [ ] **Step 1: Write the failing test (append)**

```python
def test_stage_real_file_matches_git(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    (repo / "a.txt").write_text("hello\n")

    pg = pylibgrit.Repository.open(str(repo / ".git"), str(repo))
    idx = pg.index()
    idx.stage(b"a.txt")
    idx.write()
    tree = idx.write_tree()

    subprocess.run(["git", "add", "a.txt"], cwd=repo, env=git_env, check=True)
    git_tree = subprocess.run(
        ["git", "write-tree"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert tree.hex == git_tree


def test_stage_executable_bit(tmp_path, git_env):
    import os
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    script = repo / "run.sh"
    script.write_text("#!/bin/sh\n")
    os.chmod(script, 0o755)

    pg = pylibgrit.Repository.open(str(repo / ".git"), str(repo))
    idx = pg.index()
    idx.stage(b"run.sh")
    idx.write()

    staged = _ls_files_stage(repo, git_env)
    assert staged.startswith("100755 ")


def test_stage_bare_repo_raises(tmp_path, git_env):
    import pylibgrit
    import pytest

    repo = tmp_path / "r.git"
    subprocess.run(["git", "init", "-q", "--bare", str(repo)], env=git_env, check=True)
    pg = pylibgrit.Repository.open(str(repo))
    idx = pg.index()
    with pytest.raises(pylibgrit.RepositoryError):
        idx.stage(b"a.txt")
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -k stage -v`
Expected: FAIL — `Index` has no `stage`.

- [ ] **Step 3: Add `stage` + helpers to `src/index.rs`**

Add to `#[pymethods] impl Index`:

```rust
    // AIDEV-NOTE: Stage a real working-tree file: read it, write its blob to the odb, build a
    // full stat-backed IndexEntry via grit's entry_from_stat, and upsert. `path` is relative to
    // the work_tree root; a bare repo (no work_tree) raises RepositoryError. Symlinks are staged
    // as their link target bytes (mode 120000), matching git. extract_path touches Python, so it
    // runs before any GIL release.
    fn stage(&self, py: Python<'_>, path: &Bound<'_, PyAny>) -> PyResult<()> {
        let rel = crate::repository::extract_path(path)?;
        let work_tree = self.repo.work_tree.clone().ok_or_else(|| {
            crate::error::invalid_ref("cannot stage a file in a bare repository (no work tree)")
        })?;
        let abs = work_tree.join(&rel);
        let rel_bytes = path_to_bytes(&rel);

        let meta = std::fs::symlink_metadata(&abs).map_err(io_err)?;
        let mode = mode_from_metadata(&meta);
        let blob_bytes = if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&abs).map_err(io_err)?;
            path_to_bytes(&target)
        } else {
            std::fs::read(&abs).map_err(io_err)?
        };

        let oid = py
            .allow_threads(|| self.repo.odb.write(grit_lib::objects::ObjectKind::Blob, &blob_bytes))
            .map_err(map_err)?;
        let entry = py
            .allow_threads(|| grit_lib::index::entry_from_stat(&abs, &rel_bytes, oid, mode))
            .map_err(map_err)?;
        self.inner.lock().unwrap().add_or_replace(entry);
        Ok(())
    }
```

At module scope in `src/index.rs`:

```rust
// AIDEV-NOTE: Git file mode from filesystem metadata (Unix): symlink -> 120000, any execute bit
// -> 100755, else 100644. Mirrors how `git add` chooses a blob's tree mode.
#[cfg(unix)]
fn mode_from_metadata(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.file_type().is_symlink() {
        0o120000
    } else if meta.permissions().mode() & 0o111 != 0 {
        0o100755
    } else {
        0o100644
    }
}

// AIDEV-NOTE: Index/relative path bytes (Unix: OS bytes 1:1, preserving non-UTF-8 fidelity).
#[cfg(unix)]
fn path_to_bytes(p: &std::path::Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

// AIDEV-NOTE: Map a std::io::Error to OSError with errno (mirrors error::map_err's Io arm for
// errors that don't originate from grit_lib::error::Error).
fn io_err(e: std::io::Error) -> PyErr {
    match e.raw_os_error() {
        Some(errno) => pyo3::exceptions::PyOSError::new_err((errno, format!("{e}"))),
        None => pyo3::exceptions::PyOSError::new_err(format!("{e}")),
    }
}
```

> The plan targets Unix (the project is Linux/Unix-first per the spike; CI is glibc/macOS). The `#[cfg(unix)]` helpers match the existing `bytes_to_pathbuf` pattern in `repository.rs`.

- [ ] **Step 4: Stub**

In `class Index` (`.pyi`), add after `add_entry`:

```python
    def stage(self, path: bytes | os.PathLike[str]) -> None: ...
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -v`
Expected: PASS (7 tests).

- [ ] **Step 6: Commit**

```bash
git add src/index.rs python/pylibgrit/__init__.pyi tests/test_index_write.py
git commit -m "feat: Index.stage real working-tree files"
```

---

### Task 7: `Index.__len__` / `__iter__`

**Files:**
- Modify: `src/index.rs` (add `__len__`, `__iter__`, `IndexEntryIter`)
- Modify: `src/lib.rs` (register `IndexEntryIter`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_index_write.py` (append)

- [ ] **Step 1: Write the failing test (append)**

```python
def test_index_len_and_iter(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    b1 = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"1\n")
    b2 = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"2\n")
    idx = pg.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.add(b"b.txt", b2, 0o100644)
    assert len(idx) == 2
    names = sorted(e.path for e in idx)
    assert names == [b"a.txt", b"b.txt"]
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py::test_index_len_and_iter -v`
Expected: FAIL — `Index` has no `__len__`.

- [ ] **Step 3: Add iteration to `src/index.rs`**

Add to `#[pymethods] impl Index`:

```rust
    fn __len__(&self) -> usize {
        self.inner.lock().unwrap().entries.len()
    }

    // AIDEV-NOTE: Snapshot the entries at iteration time into the iterator (owning design,
    // mirroring TreeIter/ReferenceIter). The iterator outlives this Index and is unaffected by
    // later mutations. grit's Index exposes its entries via the public `entries` Vec field.
    fn __iter__(&self) -> IndexEntryIter {
        let snapshot: Vec<grit_lib::index::IndexEntry> =
            self.inner.lock().unwrap().entries.clone();
        IndexEntryIter {
            entries: snapshot.into(),
            idx: 0,
        }
    }
```

At module scope:

```rust
/// Iterator over a snapshot of an `Index`'s entries; owns its data.
#[pyclass(module = "pylibgrit._pylibgrit")]
pub struct IndexEntryIter {
    entries: Arc<[grit_lib::index::IndexEntry]>,
    idx: usize,
}

#[pymethods]
impl IndexEntryIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> Option<IndexEntry> {
        let e = self.entries.get(self.idx)?.clone();
        self.idx += 1;
        Some(IndexEntry { inner: e })
    }
}
```

- [ ] **Step 4: Register `IndexEntryIter` in `src/lib.rs`**

```rust
    m.add_class::<index::IndexEntryIter>()?;
```

(Internal iterator — registered but NOT added to `__init__.py`'s `__all__`, mirroring `TreeIter`.)

- [ ] **Step 5: Stub**

In `class Index` (`.pyi`), add:

```python
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[IndexEntry]: ...
```

(`Iterator` is already imported in the stub header.)

- [ ] **Step 6: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_index_write.py -v`
Expected: PASS (8 tests).

- [ ] **Step 7: Commit**

```bash
git add src/index.rs src/lib.rs python/pylibgrit/__init__.pyi tests/test_index_write.py
git commit -m "feat: Index __len__/__iter__"
```

---

### Task 8: `Repository.create_commit`

**Files:**
- Modify: `src/objects.rs` (add `pub(crate) fn resolve_ident`)
- Modify: `src/repository.rs` (add `create_commit`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_create_commit.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_create_commit.py`:

```python
import subprocess

import pytest


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _empty_tree(repo, env):
    return subprocess.run(
        ["git", "write-tree"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()


def test_create_commit_matches_git_commit_tree(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    tree_hex = _empty_tree(repo, git_env)

    pg = pylibgrit.Repository.open(str(repo / ".git"))
    tree = pylibgrit.ObjectId.from_hex(tree_hex)
    # Pin the same identity + time git uses below (epoch 1112911993, +0000).
    sig = pylibgrit.Signature(b"Test Author", b"author@example.com", (1112911993, 0))
    committer = pylibgrit.Signature(b"Test Committer", b"committer@example.com", (1112911993, 0))
    commit = pg.create_commit(
        tree, parents=[], author=sig, committer=committer, message=b"initial commit\n"
    )

    env = dict(git_env)
    env.update(
        GIT_AUTHOR_NAME="Test Author", GIT_AUTHOR_EMAIL="author@example.com",
        GIT_AUTHOR_DATE="1112911993 +0000",
        GIT_COMMITTER_NAME="Test Committer", GIT_COMMITTER_EMAIL="committer@example.com",
        GIT_COMMITTER_DATE="1112911993 +0000",
    )
    git_commit = subprocess.run(
        ["git", "commit-tree", tree_hex, "-m", "initial commit"],
        cwd=repo, env=env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert commit.hex == git_commit


def test_create_commit_author_raw_byte_exact(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    tree_hex = _empty_tree(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    tree = pylibgrit.ObjectId.from_hex(tree_hex)

    ident = b"Test Author <author@example.com> 1112911993 +0000"
    commit = pg.create_commit(
        tree, parents=[], author_raw=ident, committer_raw=ident, message=b"x\n"
    )
    # The raw author/committer header must round-trip verbatim.
    obj = pg.odb.read(commit)
    assert b"author " + ident + b"\n" in obj.data
    # And it equals git commit-tree with the same identity.
    env = dict(git_env)
    env.update(
        GIT_AUTHOR_NAME="Test Author", GIT_AUTHOR_EMAIL="author@example.com",
        GIT_AUTHOR_DATE="1112911993 +0000",
        GIT_COMMITTER_NAME="Test Author", GIT_COMMITTER_EMAIL="author@example.com",
        GIT_COMMITTER_DATE="1112911993 +0000",
    )
    git_commit = subprocess.run(
        ["git", "commit-tree", tree_hex, "-m", "x"],
        cwd=repo, env=env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert commit.hex == git_commit


def test_create_commit_multi_parent(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    tree_hex = _empty_tree(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    tree = pylibgrit.ObjectId.from_hex(tree_hex)
    sig = pylibgrit.Signature(b"A", b"a@x", (1, 0))
    p1 = pg.create_commit(tree, parents=[], author=sig, committer=sig, message=b"p1\n")
    p2 = pg.create_commit(tree, parents=[], author=sig, committer=sig, message=b"p2\n")
    merge = pg.create_commit(tree, parents=[p1, p2], author=sig, committer=sig, message=b"m\n")
    assert pg.commit(merge).parents == [p1, p2]


def test_create_commit_rejects_both_author_forms(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    tree = pylibgrit.ObjectId.from_hex(_empty_tree(repo, git_env))
    sig = pylibgrit.Signature(b"A", b"a@x", (1, 0))
    with pytest.raises(ValueError):
        pg.create_commit(
            tree, parents=[], author=sig, author_raw=b"A <a@x> 1 +0000",
            committer=sig, message=b"x\n",
        )
    with pytest.raises(ValueError):
        pg.create_commit(tree, parents=[], committer=sig, message=b"x\n")  # missing author
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_create_commit.py -v`
Expected: FAIL — `Repository` has no `create_commit`.

- [ ] **Step 3: Add `resolve_ident` to `src/objects.rs`**

At module scope (it is used by the Repository builders):

```rust
// AIDEV-NOTE: Resolve a commit/tag identity to the exact header bytes that go into the object.
// Exactly one of a structured Signature or a raw byte string must be supplied. Returning the
// bytes (placed in CommitData.author_raw / committer_raw) guarantees byte-identical OIDs.
pub(crate) fn resolve_ident(
    field: &str,
    sig: Option<&Signature>,
    raw: Option<Vec<u8>>,
) -> PyResult<Vec<u8>> {
    match (sig, raw) {
        (Some(s), None) => Ok(s.wire_bytes()),
        (None, Some(r)) => Ok(r),
        (Some(_), Some(_)) => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "provide {field}= or {field}_raw=, not both"
        ))),
        (None, None) => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "{field}= or {field}_raw= is required"
        ))),
    }
}
```

- [ ] **Step 4: Add `create_commit` to the `Repository` impl in `src/repository.rs`**

Add `use crate::objects::Signature;` near the top imports if not present (it is referenced by signature). Then in the `#[pymethods] impl Repository` block:

```rust
    // AIDEV-NOTE: Build a commit object and write it (== git commit-tree). Pure: returns the new
    // oid and moves no ref. Identity comes from a Signature (formatted to wire bytes) or a raw
    // byte header (author_raw/committer_raw) — exactly one of each pair, enforced by
    // resolve_ident. We always populate CommitData.author_raw/committer_raw and raw_message so
    // serialize_commit emits our exact bytes (byte-identical OID to git). `encoding` (an ASCII
    // charset name) is optional. The serialize is cheap and runs under the GIL; the odb write
    // releases it.
    #[pyo3(signature = (tree, parents, *, message, author=None, committer=None,
                        author_raw=None, committer_raw=None, encoding=None))]
    #[allow(clippy::too_many_arguments)]
    fn create_commit(
        &self,
        py: Python<'_>,
        tree: &crate::objects::ObjectId,
        parents: Vec<crate::objects::ObjectId>,
        message: Vec<u8>,
        author: Option<PyRef<'_, Signature>>,
        committer: Option<PyRef<'_, Signature>>,
        author_raw: Option<Vec<u8>>,
        committer_raw: Option<Vec<u8>>,
        encoding: Option<String>,
    ) -> PyResult<crate::objects::ObjectId> {
        let author_bytes = crate::objects::resolve_ident("author", author.as_deref(), author_raw)?;
        let committer_bytes =
            crate::objects::resolve_ident("committer", committer.as_deref(), committer_raw)?;
        let parent_oids: Vec<grit_lib::objects::ObjectId> =
            parents.iter().map(|p| p.inner()).collect();

        let cdata = grit_lib::objects::CommitData {
            tree: tree.inner(),
            parents: parent_oids,
            author: String::new(),
            committer: String::new(),
            author_raw: author_bytes,
            committer_raw: committer_bytes,
            encoding,
            message: String::new(),
            raw_message: Some(message),
        };
        let raw = grit_lib::objects::serialize_commit(&cdata);
        let oid = py
            .allow_threads(|| {
                self.inner
                    .odb
                    .write(grit_lib::objects::ObjectKind::Commit, &raw)
            })
            .map_err(map_err)?;
        Ok(crate::objects::ObjectId::from_inner(oid))
    }
```

- [ ] **Step 5: Stub**

In `class Repository` (`.pyi`), add:

```python
    def create_commit(self, tree: ObjectId, parents: list[ObjectId], *,
                      message: bytes,
                      author: Signature | None = None, committer: Signature | None = None,
                      author_raw: bytes | None = None, committer_raw: bytes | None = None,
                      encoding: str | None = None) -> ObjectId: ...
```

- [ ] **Step 6: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_create_commit.py -v`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
git add src/objects.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_create_commit.py
git commit -m "feat: Repository.create_commit"
```

---

### Task 9: `Repository.create_tag`

**Files:**
- Modify: `src/repository.rs` (add `create_tag`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_create_tag.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_create_tag.py`:

```python
import subprocess

import pytest

from tests.gitlib import cat_file_type


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _one_commit(repo, env):
    (repo / "a.txt").write_text("hi\n")
    subprocess.run(["git", "add", "a.txt"], cwd=repo, env=env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "c"], cwd=repo, env=env, check=True)
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()


def test_create_tag_matches_git(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    head = _one_commit(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    target = pylibgrit.ObjectId.from_hex(head)
    tagger = pylibgrit.Signature(b"Tagger", b"tag@example.com", (1112911993, 0))
    tag = pg.create_tag(
        target, pylibgrit.ObjectKind.COMMIT, b"v1", message=b"release one\n", tagger=tagger,
    )
    assert cat_file_type(repo, tag.hex) == "tag"
    # read-side view agrees
    assert pg.tag(tag).name == b"v1"
    assert pg.tag(tag).target == target

    # Oracle: `git tag -a` takes its tagger from the committer identity/date, so pin those to
    # match the Signature above; refs/tags/v1 then points at the annotated-tag object.
    env = dict(git_env)
    env.update(
        GIT_COMMITTER_NAME="Tagger",
        GIT_COMMITTER_EMAIL="tag@example.com",
        GIT_COMMITTER_DATE="1112911993 +0000",
    )
    subprocess.run(
        ["git", "tag", "-a", "v1", "-m", "release one", head],
        cwd=repo, env=env, check=True,
    )
    git_tag = subprocess.run(
        ["git", "rev-parse", "refs/tags/v1"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert tag.hex == git_tag


def test_create_tag_non_utf8_message_raises(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    head = _one_commit(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    target = pylibgrit.ObjectId.from_hex(head)
    tagger = pylibgrit.Signature(b"T", b"t@x", (1, 0))
    with pytest.raises(ValueError):
        pg.create_tag(target, pylibgrit.ObjectKind.COMMIT, b"v2",
                      message=b"\xff\xfe", tagger=tagger)
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_create_tag.py -v`
Expected: FAIL — `Repository` has no `create_tag`.

- [ ] **Step 3: Add `create_tag` to the `Repository` impl in `src/repository.rs`**

```rust
    // AIDEV-NOTE: Build an annotated-tag OBJECT and write it; returns its oid (== git mktag).
    // Pointing refs/tags/<name> at it is a separate update_ref. FIDELITY LIMITATION: grit-lib's
    // TagData stores tag/tagger/message as String only (no *_raw byte fields like CommitData),
    // so all three must be valid UTF-8 — non-UTF-8 raises ValueError (mirrors the read-side Tag
    // limitation). target_kind names the tagged object's type ("commit"/"tree"/"blob"/"tag").
    // tagger comes from a Signature or raw bytes, or is omitted (None) for a tagger-less tag.
    #[pyo3(signature = (target, target_kind, name, *, message, tagger=None, tagger_raw=None))]
    fn create_tag(
        &self,
        py: Python<'_>,
        target: &crate::objects::ObjectId,
        target_kind: &Bound<'_, PyAny>,
        name: Vec<u8>,
        message: Vec<u8>,
        tagger: Option<PyRef<'_, Signature>>,
        tagger_raw: Option<Vec<u8>>,
    ) -> PyResult<crate::objects::ObjectId> {
        let kind = crate::objects::py_to_kind(target_kind)?;
        let type_str = match kind {
            grit_lib::objects::ObjectKind::Commit => "commit",
            grit_lib::objects::ObjectKind::Tree => "tree",
            grit_lib::objects::ObjectKind::Blob => "blob",
            grit_lib::objects::ObjectKind::Tag => "tag",
        };
        // tagger is optional; when present it must resolve (Signature XOR raw) and be UTF-8.
        let tagger_str = match (tagger, tagger_raw) {
            (None, None) => None,
            (s, r) => {
                let bytes = crate::objects::resolve_ident("tagger", s.as_deref(), r)?;
                Some(utf8_field("tagger", bytes)?)
            }
        };
        let tdata = grit_lib::objects::TagData {
            object: target.inner(),
            object_type: type_str.to_owned(),
            tag: utf8_field("tag name", name)?,
            tagger: tagger_str,
            message: utf8_field("tag message", message)?,
        };
        let raw = grit_lib::objects::serialize_tag(&tdata);
        let oid = py
            .allow_threads(|| self.inner.odb.write(grit_lib::objects::ObjectKind::Tag, &raw))
            .map_err(map_err)?;
        Ok(crate::objects::ObjectId::from_inner(oid))
    }
```

Add this free helper near the bottom of `src/repository.rs` (module scope):

```rust
// AIDEV-NOTE: grit-lib's TagData fields are `String`, so tag name/tagger/message must be UTF-8.
// Convert here and raise ValueError (not a silent lossy decode) on non-UTF-8 input.
fn utf8_field(what: &str, bytes: Vec<u8>) -> PyResult<String> {
    String::from_utf8(bytes)
        .map_err(|_| pyo3::exceptions::PyValueError::new_err(format!("{what} must be valid UTF-8")))
}
```

- [ ] **Step 4: Stub**

In `class Repository` (`.pyi`), add:

```python
    def create_tag(self, target: ObjectId, target_kind: ObjectKind, name: bytes, *,
                   message: bytes,
                   tagger: Signature | None = None, tagger_raw: bytes | None = None) -> ObjectId: ...
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_create_tag.py -v`
Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_create_tag.py
git commit -m "feat: Repository.create_tag"
```

---

### Task 10: `RefMismatchError` exception

**Files:**
- Modify: `src/error.rs` (define + register)
- Modify: `python/pylibgrit/__init__.py` (import + `__all__`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_ref_write.py` (created here with one import-smoke test; filled in Task 11)

- [ ] **Step 1: Write the failing test**

Create `tests/test_ref_write.py`:

```python
def test_ref_mismatch_error_is_griterror_subclass():
    import pylibgrit

    assert issubclass(pylibgrit.RefMismatchError, pylibgrit.GritError)
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -v`
Expected: FAIL — module has no `RefMismatchError`.

- [ ] **Step 3: Define + register in `src/error.rs`**

After the `InvalidObjectError` `create_exception!` block, add:

```rust
create_exception!(
    _pylibgrit,
    RefMismatchError,
    GritError,
    "A ref's current value did not match the expected value (compare-and-swap/create-only)."
);
```

In `pub fn register`, add:

```rust
    m.add("RefMismatchError", m.py().get_type::<RefMismatchError>())?;
```

- [ ] **Step 4: Re-export**

`python/pylibgrit/__init__.py`: add `RefMismatchError,` to the import block and `__all__`.
`python/pylibgrit/__init__.pyi`: add `"RefMismatchError",` to `__all__` and, in the Exceptions section, add:

```python
class RefMismatchError(GritError):
    """Raised when a ref's current value fails a compare-and-swap / create-only check."""
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -v`
Expected: PASS (1 test).

- [ ] **Step 6: Commit**

```bash
git add src/error.rs python/pylibgrit/__init__.py python/pylibgrit/__init__.pyi tests/test_ref_write.py
git commit -m "feat: RefMismatchError exception"
```

---

### Task 11: `update_ref` + `delete_ref` (overwrite / CAS / create-only)

**Files:**
- Modify: `src/refs.rs` (add `pub(crate) fn read_current_oid`, `pub(crate) fn zero_like`)
- Modify: `src/repository.rs` (add `update_ref`, `delete_ref`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_ref_write.py` (append)

- [ ] **Step 1: Write the failing test (append to `tests/test_ref_write.py`)**

```python
import subprocess

import pytest


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _commit(repo, env, msg):
    (repo / "f").write_text(msg)
    subprocess.run(["git", "add", "f"], cwd=repo, env=env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", msg], cwd=repo, env=env, check=True)
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()


def test_update_ref_overwrite(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    pg.update_ref(b"refs/heads/feature", pylibgrit.ObjectId.from_hex(c1))
    got = subprocess.run(
        ["git", "rev-parse", "refs/heads/feature"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert got == c1


def test_update_ref_create_only(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    oid = pylibgrit.ObjectId.from_hex(c1)
    pg.update_ref(b"refs/heads/new", oid, create=True)  # ok, doesn't exist
    with pytest.raises(pylibgrit.RefMismatchError):
        pg.update_ref(b"refs/heads/new", oid, create=True)  # now exists


def test_update_ref_cas(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    c2 = _commit(repo, git_env, "two")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    o1, o2 = pylibgrit.ObjectId.from_hex(c1), pylibgrit.ObjectId.from_hex(c2)
    pg.update_ref(b"refs/heads/cas", o1)
    # CAS succeeds when expected matches:
    pg.update_ref(b"refs/heads/cas", o2, expected_old=o1)
    # CAS fails when expected is stale:
    with pytest.raises(pylibgrit.RefMismatchError):
        pg.update_ref(b"refs/heads/cas", o1, expected_old=o1)  # current is o2 now


def test_update_ref_create_and_expected_old_is_error(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    oid = pylibgrit.ObjectId.from_hex(c1)
    with pytest.raises(ValueError):
        pg.update_ref(b"refs/heads/x", oid, create=True, expected_old=oid)


def test_delete_ref_and_cas_delete(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    c2 = _commit(repo, git_env, "two")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    o1, o2 = pylibgrit.ObjectId.from_hex(c1), pylibgrit.ObjectId.from_hex(c2)
    pg.update_ref(b"refs/heads/d", o2)
    with pytest.raises(pylibgrit.RefMismatchError):
        pg.delete_ref(b"refs/heads/d", expected_old=o1)   # stale -> refused
    pg.delete_ref(b"refs/heads/d", expected_old=o2)        # matches -> deleted
    rc = subprocess.run(
        ["git", "rev-parse", "--verify", "-q", "refs/heads/d"],
        cwd=repo, env=git_env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    ).returncode
    assert rc != 0  # ref is gone
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -v`
Expected: FAIL — `Repository` has no `update_ref`.

- [ ] **Step 3: Add helpers to `src/refs.rs`**

At module scope (these are plain free functions, not pyclass methods):

```rust
// AIDEV-NOTE: Best-effort read of a ref's CURRENT direct oid for compare-and-swap / create-only
// checks. Returns None if the ref does not resolve. grit-lib exposes no atomic CAS primitive
// (verified against 0.4.1 source), so callers do read -> compare -> write WITHOUT a held lock:
// this catches the common non-concurrent "did it move since I read it?" case but is not a hard
// guarantee against another process writing in the window (design §6). We deliberately collapse
// any resolve error to None (treat as "no current value"); a corrupt ref therefore reads as
// absent — acceptable under the documented best-effort contract.
pub(crate) fn read_current_oid(
    git_dir: &std::path::Path,
    refname: &str,
) -> Option<grit_lib::objects::ObjectId> {
    grit_lib::refs::resolve_ref(git_dir, refname).ok()
}

// AIDEV-NOTE: A zero (null) ObjectId matching the width of `like` (SHA-1 vs SHA-256). Used as the
// reflog "old" value when creating a previously-absent ref.
pub(crate) fn zero_like(like: &grit_lib::objects::ObjectId) -> grit_lib::objects::ObjectId {
    grit_lib::objects::ObjectId::from_bytes(&vec![0u8; like.as_bytes().len()])
        .expect("all-zero buffer of valid width is a valid ObjectId")
}
```

- [ ] **Step 4: Add `update_ref` + `delete_ref` to the `Repository` impl in `src/repository.rs`**

```rust
    // AIDEV-NOTE: Create/move a ref. Three states (design §Ref safety): default overwrites;
    // create=True requires the ref be absent; expected_old=<oid> is compare-and-swap. create +
    // expected_old together is a usage error. The read-compare-write is best-effort (no atomic
    // primitive in grit-lib — see refs::read_current_oid). Ref name must be UTF-8. message=/
    // signer= reflog wiring is added in Task 13.
    #[pyo3(signature = (name, target, *, expected_old=None, create=false))]
    fn update_ref(
        &self,
        py: Python<'_>,
        name: Vec<u8>,
        target: &crate::objects::ObjectId,
        expected_old: Option<crate::objects::ObjectId>,
        create: bool,
    ) -> PyResult<()> {
        if create && expected_old.is_some() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "pass create=True or expected_old=, not both",
            ));
        }
        let refname = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let git_dir = self.inner.git_dir.clone();
        let new_oid = target.inner();

        let current = py.allow_threads(|| crate::refs::read_current_oid(&git_dir, &refname));
        if create {
            if current.is_some() {
                return Err(crate::error::RefMismatchError::new_err(format!(
                    "ref {refname} already exists"
                )));
            }
        } else if let Some(exp) = &expected_old {
            let exp_oid = exp.inner();
            match current {
                Some(cur) if cur == exp_oid => {}
                Some(cur) => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} is {}, expected {}",
                        cur.to_hex(),
                        exp_oid.to_hex()
                    )))
                }
                None => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} does not exist, expected {}",
                        exp_oid.to_hex()
                    )))
                }
            }
        }

        py.allow_threads(|| grit_lib::refs::write_ref(&git_dir, &refname, &new_oid))
            .map_err(map_err)
    }

    // AIDEV-NOTE: Delete a ref. Default deletes unconditionally; expected_old=<oid> is a
    // compare-and-swap delete (best-effort, same caveat as update_ref). message=/signer= reflog
    // wiring is added in Task 13.
    #[pyo3(signature = (name, *, expected_old=None))]
    fn delete_ref(
        &self,
        py: Python<'_>,
        name: Vec<u8>,
        expected_old: Option<crate::objects::ObjectId>,
    ) -> PyResult<()> {
        let refname = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let git_dir = self.inner.git_dir.clone();

        if let Some(exp) = &expected_old {
            let exp_oid = exp.inner();
            let current = py.allow_threads(|| crate::refs::read_current_oid(&git_dir, &refname));
            match current {
                Some(cur) if cur == exp_oid => {}
                Some(cur) => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} is {}, expected {}",
                        cur.to_hex(),
                        exp_oid.to_hex()
                    )))
                }
                None => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} does not exist, expected {}",
                        exp_oid.to_hex()
                    )))
                }
            }
        }

        py.allow_threads(|| grit_lib::refs::delete_ref(&git_dir, &refname))
            .map_err(map_err)
    }
```

- [ ] **Step 5: Stub**

In `class Repository` (`.pyi`), add:

```python
    def update_ref(self, name: bytes, target: ObjectId, *,
                   expected_old: ObjectId | None = None, create: bool = False) -> None: ...
    def delete_ref(self, name: bytes, *,
                   expected_old: ObjectId | None = None) -> None: ...
```

- [ ] **Step 6: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -v`
Expected: PASS (6 tests).

- [ ] **Step 7: Commit**

```bash
git add src/refs.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_ref_write.py
git commit -m "feat: update_ref/delete_ref with best-effort CAS + create-only"
```

---

### Task 12: `set_head` + `set_symbolic_ref`

**Files:**
- Modify: `src/repository.rs` (add `set_head`, `set_symbolic_ref`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_ref_write.py` (append)

- [ ] **Step 1: Write the failing test (append)**

```python
def test_set_head_symbolic(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    pg.set_head(b"refs/heads/other")
    got = subprocess.run(
        ["git", "symbolic-ref", "HEAD"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert got == "refs/heads/other"


def test_set_symbolic_ref(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    pg.update_ref(b"refs/heads/main", pylibgrit.ObjectId.from_hex(c1))
    pg.set_symbolic_ref(b"refs/heads/alias", b"refs/heads/main")
    got = subprocess.run(
        ["git", "symbolic-ref", "refs/heads/alias"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert got == "refs/heads/main"
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -k symbolic -v`
Expected: FAIL — `Repository` has no `set_head`.

- [ ] **Step 3: Add to the `Repository` impl in `src/repository.rs`**

```rust
    // AIDEV-NOTE: Point HEAD at a branch (symbolic ref). target is a ref name, e.g.
    // b"refs/heads/main". Must be UTF-8.
    fn set_head(&self, py: Python<'_>, target: Vec<u8>) -> PyResult<()> {
        let target_str = std::str::from_utf8(&target)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref target"))?
            .to_owned();
        let git_dir = self.inner.git_dir.clone();
        py.allow_threads(|| grit_lib::refs::write_symbolic_ref(&git_dir, "HEAD", &target_str))
            .map_err(map_err)
    }

    // AIDEV-NOTE: Write an arbitrary symbolic ref (name -> target ref name). Both must be UTF-8.
    fn set_symbolic_ref(&self, py: Python<'_>, name: Vec<u8>, target: Vec<u8>) -> PyResult<()> {
        let name_str = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let target_str = std::str::from_utf8(&target)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref target"))?
            .to_owned();
        let git_dir = self.inner.git_dir.clone();
        py.allow_threads(|| grit_lib::refs::write_symbolic_ref(&git_dir, &name_str, &target_str))
            .map_err(map_err)
    }
```

- [ ] **Step 4: Stub**

In `class Repository` (`.pyi`), add:

```python
    def set_head(self, target: bytes) -> None: ...
    def set_symbolic_ref(self, name: bytes, target: bytes) -> None: ...
```

- [ ] **Step 5: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_ref_write.py -v`
Expected: PASS (8 tests).

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_ref_write.py
git commit -m "feat: set_head/set_symbolic_ref"
```

---

### Task 13: `append_reflog` + reflog opt-in on `update_ref`/`delete_ref`

**Files:**
- Modify: `src/repository.rs` (add `append_reflog`; extend `update_ref`/`delete_ref` with `message=`/`signer=`; add `reflog_args` helper)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_reflog.py`

- [ ] **Step 1: Write the failing test**

Create `tests/test_reflog.py`:

```python
import subprocess

import pytest


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def _commit(repo, env, msg):
    (repo / "f").write_text(msg)
    subprocess.run(["git", "add", "f"], cwd=repo, env=env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", msg], cwd=repo, env=env, check=True)
    return subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=repo, env=env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()


def test_update_ref_with_message_writes_reflog(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    sig = pylibgrit.Signature(b"Test", b"t@example.com", (1112911993, 0))
    pg.update_ref(b"refs/heads/logged", pylibgrit.ObjectId.from_hex(c1),
                  create=True, message=b"branch: created", signer=sig)
    log = (repo / ".git" / "logs" / "refs" / "heads" / "logged").read_text()
    assert "branch: created" in log
    assert "t@example.com" in log


def test_update_ref_without_message_no_reflog(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    pg.update_ref(b"refs/heads/silent", pylibgrit.ObjectId.from_hex(c1), create=True)
    assert not (repo / ".git" / "logs" / "refs" / "heads" / "silent").exists()


def test_message_requires_signer(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    with pytest.raises(ValueError):
        pg.update_ref(b"refs/heads/x", pylibgrit.ObjectId.from_hex(c1),
                      create=True, message=b"no signer")


def test_explicit_append_reflog(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    c1 = _commit(repo, git_env, "one")
    c2 = _commit(repo, git_env, "two")
    pg = pylibgrit.Repository.open(str(repo / ".git"))
    o1, o2 = pylibgrit.ObjectId.from_hex(c1), pylibgrit.ObjectId.from_hex(c2)
    sig = pylibgrit.Signature(b"Test", b"t@example.com", (1112911993, 0))
    pg.append_reflog(b"refs/heads/main", o1, o2, signer=sig,
                     message=b"manual entry", force_create=True)
    log = (repo / ".git" / "logs" / "refs" / "heads" / "main").read_text()
    assert "manual entry" in log
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_reflog.py -v`
Expected: FAIL — `update_ref` has no `message`/`signer`; no `append_reflog`.

- [ ] **Step 3: Add the `reflog_args` helper at module scope in `src/repository.rs`**

```rust
// AIDEV-NOTE: Resolve the optional reflog request for a ref op. Returns Some((identity, message))
// only when a message is given; a message without a signer is a usage error. append_reflog wants
// the full wire identity ("Name <email> <unix> <+HHMM>") and a UTF-8 message. signer.wire_bytes()
// is UTF-8 for normal identities; non-UTF-8 signer/message raise ValueError.
fn reflog_args(
    message: Option<Vec<u8>>,
    signer: Option<&Signature>,
) -> PyResult<Option<(String, String)>> {
    match message {
        None => Ok(None),
        Some(msg) => {
            let signer = signer.ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("message= requires signer=")
            })?;
            let ident = utf8_field("signer", signer.wire_bytes())?;
            let msg = utf8_field("reflog message", msg)?;
            Ok(Some((ident, msg)))
        }
    }
}
```

(`utf8_field` was added in Task 9. `Signature` is imported from Task 8.)

- [ ] **Step 4: Replace `update_ref` and `delete_ref` with their final forms in `src/repository.rs`**

Replace the entire `update_ref` from Task 11 with this final version (adds `message=`/`signer=`; the CAS/create block now matches on `&current` so `current` survives for the reflog `old` value):

```rust
    // AIDEV-NOTE: Create/move a ref. Three states (design §Ref safety): default overwrites;
    // create=True requires the ref be absent; expected_old=<oid> is compare-and-swap. create +
    // expected_old together is a usage error. The read-compare-write is best-effort (no atomic
    // primitive in grit-lib — see refs::read_current_oid). Ref name must be UTF-8. When message=
    // is given (with signer=), an old->new reflog entry is appended after the write.
    #[pyo3(signature = (name, target, *, expected_old=None, create=false, message=None, signer=None))]
    fn update_ref(
        &self,
        py: Python<'_>,
        name: Vec<u8>,
        target: &crate::objects::ObjectId,
        expected_old: Option<crate::objects::ObjectId>,
        create: bool,
        message: Option<Vec<u8>>,
        signer: Option<PyRef<'_, Signature>>,
    ) -> PyResult<()> {
        if create && expected_old.is_some() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "pass create=True or expected_old=, not both",
            ));
        }
        let refname = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let reflog = reflog_args(message, signer.as_deref())?;
        let git_dir = self.inner.git_dir.clone();
        let new_oid = target.inner();

        let current = py.allow_threads(|| crate::refs::read_current_oid(&git_dir, &refname));
        if create {
            if current.is_some() {
                return Err(crate::error::RefMismatchError::new_err(format!(
                    "ref {refname} already exists"
                )));
            }
        } else if let Some(exp) = &expected_old {
            let exp_oid = exp.inner();
            match &current {
                Some(cur) if *cur == exp_oid => {}
                Some(cur) => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} is {}, expected {}",
                        cur.to_hex(),
                        exp_oid.to_hex()
                    )))
                }
                None => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} does not exist, expected {}",
                        exp_oid.to_hex()
                    )))
                }
            }
        }

        let old_for_log = current.unwrap_or_else(|| crate::refs::zero_like(&new_oid));
        py.allow_threads(|| grit_lib::refs::write_ref(&git_dir, &refname, &new_oid))
            .map_err(map_err)?;
        if let Some((ident, msg)) = reflog {
            py.allow_threads(|| {
                grit_lib::refs::append_reflog(
                    &git_dir, &refname, &old_for_log, &new_oid, &ident, &msg, false,
                )
            })
            .map_err(map_err)?;
        }
        Ok(())
    }
```

Then replace the entire `delete_ref` from Task 11 with this final version (adds `message=`/`signer=`; appends an old→zero reflog entry BEFORE the delete so the log file still exists):

```rust
    #[pyo3(signature = (name, *, expected_old=None, message=None, signer=None))]
    fn delete_ref(
        &self,
        py: Python<'_>,
        name: Vec<u8>,
        expected_old: Option<crate::objects::ObjectId>,
        message: Option<Vec<u8>>,
        signer: Option<PyRef<'_, Signature>>,
    ) -> PyResult<()> {
        let refname = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let git_dir = self.inner.git_dir.clone();
        let reflog = reflog_args(message, signer.as_deref())?;
        let current = py.allow_threads(|| crate::refs::read_current_oid(&git_dir, &refname));

        if let Some(exp) = &expected_old {
            let exp_oid = exp.inner();
            match &current {
                Some(cur) if *cur == exp_oid => {}
                Some(cur) => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} is {}, expected {}",
                        cur.to_hex(),
                        exp_oid.to_hex()
                    )))
                }
                None => {
                    return Err(crate::error::RefMismatchError::new_err(format!(
                        "ref {refname} does not exist, expected {}",
                        exp_oid.to_hex()
                    )))
                }
            }
        }

        if let (Some((ident, msg)), Some(cur)) = (&reflog, &current) {
            let zero = crate::refs::zero_like(cur);
            py.allow_threads(|| {
                grit_lib::refs::append_reflog(&git_dir, &refname, cur, &zero, ident, msg, false)
            })
            .map_err(map_err)?;
        }

        py.allow_threads(|| grit_lib::refs::delete_ref(&git_dir, &refname))
            .map_err(map_err)
    }
```

- [ ] **Step 5: Add `append_reflog` to the `Repository` impl**

```rust
    // AIDEV-NOTE: Explicitly append a reflog entry: <old> <new> <identity>\t<message>. signer is
    // the full wire identity; message and identity must be UTF-8. force_create=True creates the
    // reflog file even if the repo would not auto-create it (e.g. for arbitrary refs).
    #[pyo3(signature = (name, old, new, *, signer, message, force_create=false))]
    fn append_reflog(
        &self,
        py: Python<'_>,
        name: Vec<u8>,
        old: &crate::objects::ObjectId,
        new: &crate::objects::ObjectId,
        signer: PyRef<'_, Signature>,
        message: Vec<u8>,
        force_create: bool,
    ) -> PyResult<()> {
        let refname = std::str::from_utf8(&name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let ident = utf8_field("signer", signer.wire_bytes())?;
        let msg = utf8_field("reflog message", message)?;
        let git_dir = self.inner.git_dir.clone();
        let (old_oid, new_oid) = (old.inner(), new.inner());
        py.allow_threads(|| {
            grit_lib::refs::append_reflog(
                &git_dir, &refname, &old_oid, &new_oid, &ident, &msg, force_create,
            )
        })
        .map_err(map_err)
    }
```

- [ ] **Step 6: Stubs**

In `class Repository` (`.pyi`), update `update_ref`/`delete_ref` and add `append_reflog`:

```python
    def update_ref(self, name: bytes, target: ObjectId, *,
                   expected_old: ObjectId | None = None, create: bool = False,
                   message: bytes | None = None, signer: Signature | None = None) -> None: ...
    def delete_ref(self, name: bytes, *,
                   expected_old: ObjectId | None = None,
                   message: bytes | None = None, signer: Signature | None = None) -> None: ...
    def append_reflog(self, name: bytes, old: ObjectId, new: ObjectId, *,
                      signer: Signature, message: bytes, force_create: bool = False) -> None: ...
```

- [ ] **Step 7: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_reflog.py tests/test_ref_write.py -v`
Expected: PASS (reflog: 4; ref_write still 8).

- [ ] **Step 8: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_reflog.py
git commit -m "feat: append_reflog + opt-in reflog on update_ref/delete_ref"
```

---

### Task 14: Write-path concurrency test

**Files:**
- Test: `tests/test_write_concurrency.py`

- [ ] **Step 1: Write the test**

Create `tests/test_write_concurrency.py`:

```python
import subprocess
import threading


def _init(repo, env):
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=env, check=True)


def test_parallel_blob_writes_are_sound(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    _init(repo, git_env)
    pg = pylibgrit.Repository.open(str(repo / ".git"))

    results: dict[int, str] = {}
    errors: list[Exception] = []

    def worker(n: int) -> None:
        try:
            oid = pg.odb.write(pylibgrit.ObjectKind.BLOB, f"content-{n}\n".encode())
            results[n] = oid.hex
        except Exception as exc:  # pragma: no cover - failure path
            errors.append(exc)

    threads = [threading.Thread(target=worker, args=(n,)) for n in range(50)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert not errors
    assert len(set(results.values())) == 50  # distinct contents -> distinct oids
    for n, hexoid in results.items():
        assert pg.odb.read(pylibgrit.ObjectId.from_hex(hexoid)).data == f"content-{n}\n".encode()
```

- [ ] **Step 2: Run to verify it passes**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/test_write_concurrency.py -v`
Expected: PASS (1 test). (No code change needed — writes already release the GIL through grit-lib's `Arc<Mutex>` odb; this guards against regressions.)

- [ ] **Step 3: Commit**

```bash
git add tests/test_write_concurrency.py
git commit -m "test: write-path concurrency (parallel GIL-released writes)"
```

---

### Task 15: Final gates — full suite, lint, types, stub sync

**Files:** none (verification + any fixups)

- [ ] **Step 1: Rebuild and run the FULL pytest suite**

Run: `uv run maturin develop --uv --locked && uv run pytest tests/ -v`
Expected: PASS — all pre-existing read-core tests plus the new write tests.

- [ ] **Step 2: Type-check Python**

Run: `uv run mypy python tests`
Expected: no errors. (Fix any stub/type mismatch in `python/pylibgrit/__init__.pyi`.)

- [ ] **Step 3: Stub vs runtime parity**

Run: `uv run python -m mypy.stubtest pylibgrit`
Expected: no errors. (If it reports a missing/extra member, reconcile `__init__.pyi` with the runtime — every new method/class/exception must appear in the stub with a matching signature, and `Index`/`IndexEntry`/`RefMismatchError` must be in `__init__.py`'s imports + `__all__`.)

- [ ] **Step 4: Rust + Python lint/format**

Run:
```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
uv run ruff format --check .
uv run ruff check .
```
Expected: all clean. (Run `cargo fmt` / `uv run ruff format .` to fix formatting; address any clippy warning — e.g. add `#[allow(clippy::too_many_arguments)]` where already noted.)

- [ ] **Step 5: Commit any fixups**

```bash
git add -A
git commit -m "chore: write-core Phase A — pass full gates (fmt/clippy/mypy/stubtest)"
```

- [ ] **Step 6: Final sanity — the canonical end-to-end flow**

Add `tests/test_write_smoke.py` exercising the whole §3 data-flow path, then run the suite once more:

```python
import subprocess


def test_build_a_commit_end_to_end(tmp_path, git_env):
    import pylibgrit

    repo = tmp_path / "r"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", "-b", "main", str(repo)], env=git_env, check=True)
    pg = pylibgrit.Repository.open(str(repo / ".git"))

    blob = pg.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")
    idx = pg.index()
    idx.add(b"greeting.txt", blob, 0o100644)
    idx.write()
    tree = idx.write_tree()
    sig = pylibgrit.Signature(b"Ada", b"ada@x.io", (1718000000, 0))
    commit = pg.create_commit(tree, parents=[], author=sig, committer=sig, message=b"init\n")
    pg.update_ref(b"refs/heads/main", commit, create=True, message=b"commit: init", signer=sig)

    got = subprocess.run(
        ["git", "rev-parse", "refs/heads/main"], cwd=repo, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    assert got == commit.hex
    # git agrees the tree/file are intact
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", commit.hex],
        cwd=repo, env=git_env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode()
    assert "greeting.txt" in listing
```

Run: `uv run maturin develop --uv --locked && uv run pytest tests/ -q`
Expected: PASS (entire suite).

- [ ] **Step 7: Commit**

```bash
git add tests/test_write_smoke.py
git commit -m "test: end-to-end build-a-commit smoke"
```

---

## Notes for the executor

- **Reflog `current` borrow (Task 13):** when you add `old_for_log = current.unwrap_or_else(...)`, make sure the earlier create/CAS check matches on `&current` / `&expected_old` (not by-move) so `current` is still owned at the `old_for_log` line. The Task-13 `delete_ref` block already shows the borrowing form; apply the same to `update_ref`.
- **`encoding` parameter:** charset names are ASCII; `Option<String>` is the right type. It is rarely used — tests don't require it, but it's part of the surface and must appear in the stub.
- **Best-effort CAS is intentional** (design §6). Do not try to make it atomic in Phase A; grit-lib 0.4.1 has no CAS primitive (verified against source). The atomic upgrade is Phase B.
- **Single `impl Repository` block:** all of `index`, `create_commit`, `create_tag`, `update_ref`, `delete_ref`, `set_head`, `set_symbolic_ref`, `append_reflog` go in the one `#[pymethods] impl Repository` in `src/repository.rs` (no `multiple-pymethods` feature).
```
