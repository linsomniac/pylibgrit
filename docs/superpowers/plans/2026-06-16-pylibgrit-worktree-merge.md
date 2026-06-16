# pylibgrit Phase B — Worktree & Merge — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local working-tree + three-way-merge surface to pylibgrit on top of grit-lib 0.4.1 plumbing — `Repository.init`, `checkout_tree`, `merge_base`/`merge_trees`/`merge_commits`, `commit_index`, tag-ref helpers — and upgrade ref compare-and-swap from best-effort to a binding-held lockfile.

**Architecture:** Each method assembles a grit-lib plumbing workflow in Rust behind the existing OO façade (methods on `Repository`; `MergeResult` is a returned value-object). No new dependencies, no Cargo features, no network. Two new modules (`src/checkout.rs`, `src/merge.rs`); the atomic-CAS helpers live in `src/refs.rs`. All disk effects are atomic (temp-file/lock + rename).

**Tech Stack:** Rust + PyO3 0.23.3 (abi3-py311), maturin, grit-lib 0.4.1, pytest with a real-`git` oracle (`tests/gitlib.py`, `tests/conftest.py`).

---

## Conventions for every task

- **Branch:** `worktree-merge-phase-b` (already created off `main`).
- **Build before pytest:** `uv run maturin develop --uv --locked`
  - If `uv run` serves a stale cached build, force a reinstall: `uv pip install -e . --reinstall-package pylibgrit`
- **Gate suite (all must pass before a task's final commit):**
  ```bash
  uv run maturin develop --uv --locked
  uv run pytest -q
  uv run mypy python tests
  uv run python -m mypy.stubtest pylibgrit            # NO allowlist
  cargo fmt --check
  cargo clippy --all-targets --locked -- -D warnings
  uv run ruff format --check . && uv run ruff check .
  ```
- **Stubs are part of the change.** Any new Python-visible method/class REQUIRES a matching `python/pylibgrit/__init__.pyi` entry in the *same* task, or `stubtest` fails. PyO3 `#[new]` → stub as `def __new__`. Non-trivial defaults (bytes, tuples) → stub the default as `= ...`.
- **Anchor comments:** add `AIDEV-NOTE:` comments on the non-obvious bits (atomic-CAS lock protocol, checkout overlay policy, merge conflict derivation), per the repo's existing style. Do not remove existing `AIDEV-` anchors.
- **Format Python with `ruff format`; annotate types; keep mypy clean.**

## Verified grit-lib 0.4.1 facts the code below depends on

- `grit_lib::repo::init_repository(path: &Path, bare: bool, initial_branch: &str, template_dir: Option<&Path>, ref_storage: &str) -> Result<Repository>`.
- `grit_lib::porcelain::checkout::write_to_worktree(work_tree: &Path, rel_path: &str, data: &[u8], mode: u32) -> Result<()>` — **always overwrites** the target; natively creates symlinks (mode `0o120000`) and sets the exec bit (mode `0o100755`). `rel_path` must be `&str` (UTF-8).
- Git tree modes (`grit_lib::index::MODE_*`): `MODE_TREE = 0o040000`, `MODE_GITLINK = 0o160000`, `MODE_SYMLINK = 0o120000`, `MODE_EXECUTABLE = 0o100755`, `MODE_REGULAR = 0o100644`.
- `grit_lib::objects::parse_tree(&[u8]) -> Result<Vec<TreeEntry>>`; `TreeEntry { mode: u32, name: Vec<u8>, oid: ObjectId }`. `serialize_tree(&[TreeEntry]) -> Vec<u8>` (`serialize_tree(&[])` is the empty tree).
- `grit_lib::objects::parse_commit(&[u8]) -> Result<CommitData>`; `CommitData.tree: ObjectId`.
- `grit_lib::merge_base::merge_bases_all(repo: &Repository, commits: &[ObjectId]) -> Result<Vec<ObjectId>>`.
- `grit_lib::merge_trees::merge_trees_three_way(repo, base_tree, ours_tree, theirs_tree, favor: MergeFavor, ws: WhitespaceMergeOptions, diff_algorithm: Option<&str>, presentation: TreeMergeConflictPresentation) -> Result<TreeMergeOutput>`; `TreeMergeOutput { index: grit_lib::index::Index, conflict_content: BTreeMap<Vec<u8>, ObjectId> }`. `WhitespaceMergeOptions::default()` and `TreeMergeConflictPresentation::default()` exist.
- `grit_lib::merge_file::MergeFavor` = `None | Ours | Theirs | Union`.
- `grit_lib::index::IndexEntry::stage(&self) -> u16` (entry conflict stage; `0` = merged). `Index.entries: Vec<IndexEntry>` is public.
- `grit_lib::write_tree::write_tree_from_index(odb: &Odb, index: &Index, prefix: &str) -> Result<ObjectId>`.
- `grit_lib::index::entry_from_stat(abs: &Path, rel: &[u8], oid: ObjectId, mode: u32) -> Result<IndexEntry>`.
- Ref path + lock (all **public**): `grit_lib::worktree_ref::resolve_ref_storage(git_dir, refname) -> (PathBuf, String)` (the `.0` is exactly grit's private `ref_storage_dir`), `grit_lib::ref_namespace::storage_ref_name(refname) -> String`, `grit_lib::refs::lock_path_for_ref(path: &Path) -> PathBuf`. grit's own `write_ref` computes `path = resolve_ref_storage(git_dir, refname).0.join(storage_ref_name(refname))` and locks `lock_path_for_ref(&path)` with `O_CREAT|O_EXCL`.
- `grit_lib::refs::read_raw_ref(git_dir, refname) -> Result<RawRefLookup>` (`Exists | NotFound | IsDirectory`); `grit_lib::refs::resolve_ref(git_dir, refname) -> Result<ObjectId>`; `grit_lib::refs::read_head(git_dir) -> Result<Option<String>>` (symbolic target, or `None` if detached); `grit_lib::refs::packed_refs_entry_exists(git_dir, refname) -> Result<bool>`; `grit_lib::refs::append_reflog(git_dir, refname, old, new, ident, msg, force_create) -> Result<()>`.
- Existing pylibgrit helpers to reuse (all in `src/repository.rs` unless noted): `extract_path`, `validate_ref_name`, `reject_wire_control`, `reflog_args`, `utf8_field`; `crate::objects::{resolve_ident, py_to_kind, ObjectId, Signature}`; `crate::refs::{read_current_oid, zero_like}`; `crate::index::Index::new_loaded`; `crate::error::{map_err, invalid_ref, RefMismatchError, RepositoryError, InvalidObjectError}`. `ObjectId`: `.inner() -> grit_lib::objects::ObjectId`, `::from_inner`, `.to_hex()`, `Display` writes hex, `PartialEq`. The `Repository` pyclass holds `inner: Arc<grit_lib::repo::Repository>` with public fields `inner.git_dir: PathBuf`, `inner.work_tree: Option<PathBuf>`, `inner.odb`, and methods `inner.load_index()`, `inner.write_index(&mut Index)`.

---

## Task 1: `Repository.init`

**Files:**
- Modify: `src/repository.rs` (add an `init` staticmethod inside the existing `#[pymethods] impl Repository` block, next to `open`/`discover`)
- Modify: `python/pylibgrit/__init__.pyi` (Repository class)
- Test: `tests/test_init.py` (create)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_init.py
import subprocess


def _git_text(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def test_init_non_bare_recognized_by_git(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work), initial_branch=b"main")
    assert repo.is_bare is False
    # git recognizes it and HEAD is the symbolic ref refs/heads/main (unborn)
    assert _git_text(work, git_env, "rev-parse", "--is-bare-repository") == "false"
    assert _git_text(work, git_env, "symbolic-ref", "HEAD") == "refs/heads/main"


def test_init_bare(tmp_path, git_env):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True, initial_branch=b"trunk")
    assert repo.is_bare is True
    assert _git_text(gd, git_env, "rev-parse", "--is-bare-repository") == "true"
    assert _git_text(gd, git_env, "symbolic-ref", "HEAD") == "refs/heads/trunk"


def test_init_default_branch_is_main(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "d"
    pylibgrit.Repository.init(str(work))
    assert _git_text(work, git_env, "symbolic-ref", "HEAD") == "refs/heads/main"
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_init.py -q`
Expected: FAIL — `Repository` has no attribute `init`.

- [ ] **Step 3: Implement**

Add inside `#[pymethods] impl Repository` (after `open`):

```rust
// AIDEV-NOTE: Initialize (or reinitialize) a repository like `git init`. Wraps
// grit_lib::repo::init_repository with template_dir=None, ref_storage="files" (the default
// loose-ref backend; reftable is out of scope for Phase B). `initial_branch` becomes the
// symbolic HEAD target refs/heads/<branch>; we validate it as a ref so a bad name cannot
// corrupt HEAD. initial_branch=None defaults to "main" (matches our own default branch).
#[staticmethod]
#[pyo3(signature = (path, *, bare=false, initial_branch=None))]
fn init(
    py: Python<'_>,
    path: &Bound<'_, PyAny>,
    bare: bool,
    initial_branch: Option<Vec<u8>>,
) -> PyResult<Self> {
    let path = extract_path(path)?;
    let branch = match initial_branch {
        Some(b) => utf8_field("initial_branch", b)?,
        None => "main".to_owned(),
    };
    // Validate the resulting branch ref name (refs/heads/<branch>) before init writes HEAD.
    let mut full = b"refs/heads/".to_vec();
    full.extend_from_slice(branch.as_bytes());
    validate_ref_name(&full)?;
    let repo = py
        .allow_threads(|| {
            grit_lib::repo::init_repository(&path, bare, &branch, None, "files")
        })
        .map_err(map_err)?;
    Ok(Self {
        inner: Arc::new(repo),
    })
}
```

Add to the Repository stub in `python/pylibgrit/__init__.pyi` (right after the class line, before `discover`):

```python
    @staticmethod
    def init(
        path: str | bytes | os.PathLike[str],
        *,
        bare: bool = False,
        initial_branch: bytes = ...,
    ) -> Repository: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_init.py -q`
Expected: PASS (3 tests).

- [ ] **Step 5: Run the full gate suite** (commands in "Conventions"). All green.

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_init.py
git commit -m "feat: Repository.init (worktree & bare) over grit init_repository"
```

---

## Task 2: `write_to_worktree` (low-level) + share `validate_index_path`

**Files:**
- Modify: `src/index.rs:14` — change `fn validate_index_path` to `pub(crate) fn validate_index_path`
- Modify: `src/repository.rs` (add `write_to_worktree` method)
- Modify: `python/pylibgrit/__init__.pyi` (Repository class)
- Test: `tests/test_checkout.py` (create — Task 3 adds more to this file)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_checkout.py
import os

import pytest


def test_write_to_worktree_writes_file(tmp_path):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    repo.write_to_worktree(b"sub/greeting.txt", b"hello\n", 0o100644)
    assert (work / "sub" / "greeting.txt").read_bytes() == b"hello\n"


def test_write_to_worktree_executable_bit(tmp_path):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    repo.write_to_worktree(b"run.sh", b"#!/bin/sh\n", 0o100755)
    assert os.access(work / "run.sh", os.X_OK)


def test_write_to_worktree_bare_raises(tmp_path):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True)
    with pytest.raises(pylibgrit.RepositoryError):
        repo.write_to_worktree(b"x.txt", b"y", 0o100644)
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_checkout.py -q`
Expected: FAIL — no attribute `write_to_worktree`.

- [ ] **Step 3: Implement**

In `src/index.rs`, change the function visibility (line ~14):

```rust
pub(crate) fn validate_index_path(path: &[u8]) -> PyResult<()> {
```

In `src/repository.rs`, add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Low-level single-file working-tree write (escape hatch under checkout_tree).
// Wraps porcelain::checkout::write_to_worktree, which ALWAYS overwrites and natively handles
// symlinks (mode 0o120000) and the exec bit (mode 0o100755). Requires a non-bare repo with a
// work tree; rel_path must be a clean relative path (validate_index_path) and UTF-8 (grit's
// primitive takes &str).
fn write_to_worktree(
    &self,
    py: Python<'_>,
    rel_path: Vec<u8>,
    data: Vec<u8>,
    mode: u32,
) -> PyResult<()> {
    crate::index::validate_index_path(&rel_path)?;
    let rel = std::str::from_utf8(&rel_path).map_err(|_| {
        pyo3::exceptions::PyValueError::new_err("rel_path must be valid UTF-8")
    })?;
    let work_tree = self.inner.work_tree.clone().ok_or_else(|| {
        crate::error::invalid_ref("cannot write to a bare repository (no work tree)")
    })?;
    let rel_owned = rel.to_owned();
    py.allow_threads(|| {
        grit_lib::porcelain::checkout::write_to_worktree(&work_tree, &rel_owned, &data, mode)
    })
    .map_err(map_err)
}
```

Add to the Repository stub in `__init__.pyi` (after `init`):

```python
    def write_to_worktree(self, rel_path: bytes, data: bytes, mode: int) -> None: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_checkout.py -q`
Expected: PASS (3 tests).

- [ ] **Step 5: Full gate suite.** All green. (`invalid_ref` maps to `RepositoryError` — confirm `test_write_to_worktree_bare_raises` passes.)

- [ ] **Step 6: Commit**

```bash
git add src/index.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_checkout.py
git commit -m "feat: Repository.write_to_worktree + share validate_index_path"
```

---

## Task 3: `checkout_tree` (non-destructive overlay)

**Files:**
- Create: `src/checkout.rs`
- Modify: `src/lib.rs` (add `mod checkout;`)
- Modify: `src/repository.rs` (add `checkout_tree` method)
- Modify: `python/pylibgrit/__init__.pyi` (Repository class)
- Test: `tests/test_checkout.py` (extend)

- [ ] **Step 1: Write the failing test**

```python
# append to tests/test_checkout.py
import subprocess


def _commit_one_file(work, git_env, path, content):
    """Make a commit in a fresh git repo and return (tree_hex,) via the oracle."""
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / path).parent.mkdir(parents=True, exist_ok=True)
    (work / path).write_bytes(content)
    subprocess.run(["git", "add", "-A"], cwd=work, env=git_env, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "c"], cwd=work, env=git_env, check=True)
    tree = subprocess.run(
        ["git", "rev-parse", "HEAD^{tree}"], cwd=work, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode().strip()
    return tree


def test_checkout_tree_materializes_files(tmp_path, git_env):
    import pylibgrit

    src = tmp_path / "src"
    tree_hex = _commit_one_file(src, git_env, "dir/a.txt", b"alpha\n")

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    repo.odb  # ensure open
    # Pull the blob into dst's odb by writing it, then checkout the tree.
    # Simplest: open src as odb source is out of scope; re-create the blob in dst.
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"dir/a.txt", blob, 0o100644)
    tree = idx.write_tree()
    repo.checkout_tree(tree)
    assert (dst / "dir" / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_overlay_preserves_untracked(tmp_path):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    (dst / "keep.txt").write_bytes(b"mine\n")
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    repo.checkout_tree(idx.write_tree())
    assert (dst / "keep.txt").read_bytes() == b"mine\n"  # untracked survives
    assert (dst / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_no_clobber_without_force(tmp_path):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    (dst / "a.txt").write_bytes(b"existing\n")
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    tree = idx.write_tree()
    with pytest.raises(FileExistsError):
        repo.checkout_tree(tree)
    assert (dst / "a.txt").read_bytes() == b"existing\n"  # untouched
    repo.checkout_tree(tree, force=True)
    assert (dst / "a.txt").read_bytes() == b"alpha\n"


def test_checkout_tree_updates_index(tmp_path, git_env):
    import pylibgrit

    dst = tmp_path / "dst"
    repo = pylibgrit.Repository.init(str(dst))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"alpha\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    repo.checkout_tree(idx.write_tree(), update_index=True)
    staged = subprocess.run(
        ["git", "ls-files", "--stage"], cwd=dst, env=git_env,
        stdout=subprocess.PIPE, check=True,
    ).stdout.decode()
    assert "a.txt" in staged


def test_checkout_tree_bare_raises(tmp_path):
    import pylibgrit

    gd = tmp_path / "b.git"
    repo = pylibgrit.Repository.init(str(gd), bare=True)
    empty = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"x")
    idx = repo.index()
    idx.add(b"a.txt", empty, 0o100644)
    tree = idx.write_tree()
    with pytest.raises(pylibgrit.RepositoryError):
        repo.checkout_tree(tree)
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_checkout.py -q -k checkout_tree`
Expected: FAIL — no attribute `checkout_tree`.

- [ ] **Step 3: Implement `src/checkout.rs`**

```rust
//! Working-tree materialization: walk a tree and lay its blobs down (non-destructive overlay).

use std::path::Path;
use std::sync::Arc;

use pyo3::prelude::*;

use crate::objects::ObjectId;

// AIDEV-NOTE: Local error type so the whole checkout can run inside one allow_threads block
// (no Python touched). The caller maps this to a PyErr UNDER the GIL via to_pyerr. PyErr cannot
// be constructed without a Python token mid-flight cleanly, hence the deferred mapping.
pub(crate) enum CheckoutError {
    NotATree(String),
    NonUtf8Path,
    Clobber(String),
    Grit(grit_lib::error::Error),
}

pub(crate) fn to_pyerr(e: CheckoutError) -> PyErr {
    match e {
        CheckoutError::NotATree(h) => {
            crate::error::InvalidObjectError::new_err(format!("object {h} is not a tree"))
        }
        CheckoutError::NonUtf8Path => pyo3::exceptions::PyValueError::new_err(
            "checkout path is not valid UTF-8 (unsupported by the worktree primitive)",
        ),
        CheckoutError::Clobber(p) => pyo3::exceptions::PyFileExistsError::new_err(format!(
            "refusing to overwrite existing work-tree path '{p}' (pass force=True)"
        )),
        CheckoutError::Grit(err) => crate::error::map_err(err),
    }
}

// AIDEV-NOTE: Recursively collect (rel_path, blob_oid, mode) for every blob/symlink/exec entry
// under `tree_oid`. Subtrees (MODE_TREE) recurse; gitlinks (MODE_GITLINK, submodule commit
// pointers) are SKIPPED (we have no submodule to populate). Names must be UTF-8 (grit's
// write_to_worktree takes &str).
fn collect(
    repo: &grit_lib::repo::Repository,
    tree_oid: &grit_lib::objects::ObjectId,
    prefix: &str,
    out: &mut Vec<(String, grit_lib::objects::ObjectId, u32)>,
) -> Result<(), CheckoutError> {
    let obj = repo.odb.read(tree_oid).map_err(CheckoutError::Grit)?;
    if obj.kind != grit_lib::objects::ObjectKind::Tree {
        return Err(CheckoutError::NotATree(tree_oid.to_hex()));
    }
    for e in grit_lib::objects::parse_tree(&obj.data).map_err(CheckoutError::Grit)? {
        let name = std::str::from_utf8(&e.name).map_err(|_| CheckoutError::NonUtf8Path)?;
        let rel = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };
        match e.mode {
            grit_lib::index::MODE_TREE => collect(repo, &e.oid, &rel, out)?,
            grit_lib::index::MODE_GITLINK => { /* submodule pointer: skip */ }
            _ => out.push((rel, e.oid, e.mode)),
        }
    }
    Ok(())
}

// AIDEV-NOTE: Overlay checkout. Steps (all under the caller's allow_threads):
//   1. Walk the tree into a flat (rel, oid, mode) list.
//   2. If !force, pre-scan for any existing work-tree path that would be clobbered and FAIL
//      before writing anything (no partial overwrite on the no-force path).
//   3. Write each blob via porcelain::checkout::write_to_worktree (handles symlink/exec).
//   4. If update_index, rebuild matching index entries from the freshly-written files
//      (entry_from_stat) and persist. Overlay semantics: we never delete entries/files.
pub(crate) fn checkout_tree(
    repo: &Arc<grit_lib::repo::Repository>,
    work_tree: &Path,
    tree_oid: &grit_lib::objects::ObjectId,
    force: bool,
    update_index: bool,
) -> Result<(), CheckoutError> {
    let mut entries: Vec<(String, grit_lib::objects::ObjectId, u32)> = Vec::new();
    collect(repo, tree_oid, "", &mut entries)?;

    if !force {
        for (rel, _, _) in &entries {
            if std::fs::symlink_metadata(work_tree.join(rel)).is_ok() {
                return Err(CheckoutError::Clobber(rel.clone()));
            }
        }
    }

    for (rel, oid, mode) in &entries {
        let blob = repo.odb.read(oid).map_err(CheckoutError::Grit)?;
        grit_lib::porcelain::checkout::write_to_worktree(work_tree, rel, &blob.data, *mode)
            .map_err(CheckoutError::Grit)?;
    }

    if update_index {
        let mut index = repo.load_index().map_err(CheckoutError::Grit)?;
        for (rel, oid, mode) in &entries {
            let abs = work_tree.join(rel);
            let rel_bytes = rel.as_bytes().to_vec();
            let entry = grit_lib::index::entry_from_stat(&abs, &rel_bytes, *oid, *mode)
                .map_err(CheckoutError::Grit)?;
            index.add_or_replace(entry);
        }
        repo.write_index(&mut index).map_err(CheckoutError::Grit)?;
    }
    Ok(())
}

// AIDEV-NOTE: Thin Repository method wrapper (called from src/repository.rs). Lives here next to
// the policy it guards. Returns RepositoryError on a bare repo (no work tree).
pub(crate) fn checkout_tree_method(
    repo: &Arc<grit_lib::repo::Repository>,
    py: Python<'_>,
    tree: &ObjectId,
    force: bool,
    update_index: bool,
) -> PyResult<()> {
    let work_tree = repo.work_tree.clone().ok_or_else(|| {
        crate::error::invalid_ref("cannot checkout into a bare repository (no work tree)")
    })?;
    let repo = Arc::clone(repo);
    let tree_oid = tree.inner();
    py.allow_threads(|| checkout_tree(&repo, &work_tree, &tree_oid, force, update_index))
        .map_err(to_pyerr)
}
```

In `src/lib.rs`, add the module declaration alongside the other `mod` lines (e.g. after `mod checkout;` is alphabetical-ish — place near `mod config;`):

```rust
mod checkout;
```

In `src/repository.rs`, add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Non-destructive overlay checkout (design §Checkout safety). Writes the tree's
// blobs into the work tree, never deletes files absent from the tree, and refuses to overwrite
// an existing path unless force=True. update_index=True rebuilds matching index entries.
#[pyo3(signature = (tree, *, force=false, update_index=true))]
fn checkout_tree(
    &self,
    py: Python<'_>,
    tree: &crate::objects::ObjectId,
    force: bool,
    update_index: bool,
) -> PyResult<()> {
    crate::checkout::checkout_tree_method(&self.inner, py, tree, force, update_index)
}
```

Add to the Repository stub in `__init__.pyi` (after `write_to_worktree`):

```python
    def checkout_tree(
        self, tree: ObjectId, *, force: bool = False, update_index: bool = True
    ) -> None: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_checkout.py -q`
Expected: PASS (all checkout tests).

- [ ] **Step 5: Full gate suite.** All green.

- [ ] **Step 6: Commit**

```bash
git add src/checkout.rs src/lib.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_checkout.py
git commit -m "feat: checkout_tree non-destructive overlay over grit worktree primitives"
```

---

## Task 4: `merge_base`

**Files:**
- Modify: `src/repository.rs` (add `merge_base`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_merge.py` (create)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_merge.py
import subprocess

import pytest


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def _diamond(work, git_env):
    """base -> (A on main) and (B on feat); return (repo_path, oid_A, oid_B, oid_base)."""
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "f.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base = _git(work, git_env, "rev-parse", "HEAD")
    (work / "a.txt").write_text("a\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    oid_a = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base)
    (work / "b.txt").write_text("b\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    oid_b = _git(work, git_env, "rev-parse", "HEAD")
    return work, oid_a, oid_b, base


def test_merge_base_matches_git(tmp_path, git_env):
    import pylibgrit

    work, a, b, base = _diamond(tmp_path / "r", git_env)
    repo = pylibgrit.Repository.open(str(work / ".git"))
    mb = repo.merge_base(pylibgrit.ObjectId.from_hex(a), pylibgrit.ObjectId.from_hex(b))
    assert mb is not None
    assert mb.hex == base
    assert mb.hex == _git(work, git_env, "merge-base", a, b)


def test_merge_base_unrelated_is_none(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "x").write_text("x\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "one")
    one = _git(work, git_env, "rev-parse", "HEAD")
    # Orphan root with no relation.
    _git(work, git_env, "checkout", "-q", "--orphan", "orphan")
    (work / "y").write_text("y\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "two")
    two = _git(work, git_env, "rev-parse", "HEAD")
    repo = pylibgrit.Repository.open(str(work / ".git"))
    assert (
        repo.merge_base(
            pylibgrit.ObjectId.from_hex(one), pylibgrit.ObjectId.from_hex(two)
        )
        is None
    )
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_merge.py -q`
Expected: FAIL — no attribute `merge_base`.

- [ ] **Step 3: Implement**

Add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: First merge base of two commits (== `git merge-base`), or None if unrelated.
// Phase B uses the FIRST base only (no recursive/virtual base for criss-cross histories;
// documented limitation). grit's merge_bases_all returns all bases; we take the first.
fn merge_base(
    &self,
    py: Python<'_>,
    a: &crate::objects::ObjectId,
    b: &crate::objects::ObjectId,
) -> PyResult<Option<crate::objects::ObjectId>> {
    let repo = Arc::clone(&self.inner);
    let (ao, bo) = (a.inner(), b.inner());
    let bases = py
        .allow_threads(|| grit_lib::merge_base::merge_bases_all(&repo, &[ao, bo]))
        .map_err(map_err)?;
    Ok(bases
        .into_iter()
        .next()
        .map(crate::objects::ObjectId::from_inner))
}
```

Add to the Repository stub in `__init__.pyi`:

```python
    def merge_base(self, a: ObjectId, b: ObjectId) -> ObjectId | None: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_merge.py -q`
Expected: PASS (2 tests).

- [ ] **Step 5: Full gate suite.** All green.

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_merge.py
git commit -m "feat: Repository.merge_base (first base; == git merge-base)"
```

---

## Task 5: `MergeResult` + `merge_trees`

**Files:**
- Create: `src/merge.rs`
- Modify: `src/lib.rs` (add `mod merge;` and `m.add_class::<merge::MergeResult>()?;`)
- Modify: `src/index.rs` (add `pub(crate) fn has_unmerged` to `impl Index`; make `write_tree` `pub(crate)`)
- Modify: `src/repository.rs` (add `merge_trees`)
- Modify: `python/pylibgrit/__init__.{py,pyi}` (export `MergeResult`)
- Test: `tests/test_merge.py` (extend)

- [ ] **Step 1: Write the failing test**

```python
# append to tests/test_merge.py
def test_merge_trees_clean_matches_git(tmp_path, git_env):
    import pylibgrit

    # base has f.txt; ours adds a.txt; theirs adds b.txt -> clean merge.
    work = tmp_path / "r"
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "f.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "a.txt").write_text("a\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    ours_commit = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "b.txt").write_text("b\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    theirs_commit = _git(work, git_env, "rev-parse", "HEAD")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
    )
    assert res.has_conflicts is False
    assert res.conflicts == []
    got = res.write_tree().hex

    # Oracle: git merge-tree --write-tree (git >= 2.38).
    oracle = subprocess.run(
        ["git", "merge-tree", "--write-tree", ours_commit, theirs_commit],
        cwd=work, env=git_env, stdout=subprocess.PIPE,
    )
    if oracle.returncode != 0:
        pytest.skip("git merge-tree --write-tree unavailable (<2.38)")
    assert got == oracle.stdout.decode().strip()


def test_merge_trees_conflict_reports_paths(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
    )
    assert res.has_conflicts is True
    assert b"c.txt" in res.conflicts
    assert res.conflict_blob(b"c.txt") is not None
    with pytest.raises(pylibgrit.RepositoryError):
        res.write_tree()


def test_merge_trees_favor_ours(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    base_commit = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    ours_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base_commit)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    theirs_tree = _git(work, git_env, "rev-parse", "HEAD^{tree}")

    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_trees(
        pylibgrit.ObjectId.from_hex(base_tree),
        pylibgrit.ObjectId.from_hex(ours_tree),
        pylibgrit.ObjectId.from_hex(theirs_tree),
        favor="ours",
    )
    assert res.has_conflicts is False  # favor=ours auto-resolves
    tree = res.write_tree()
    blob = repo.tree(tree)  # read back via read-core
    # The resolved c.txt blob equals "ours\n".
    c_oid = next(e.oid for e in blob.entries if e.name == b"c.txt")
    assert repo.blob(c_oid).data == b"ours\n"


def test_merge_trees_bad_favor_raises(tmp_path, git_env):
    import pylibgrit

    work, *_ = _diamond(tmp_path / "r", git_env)
    repo = pylibgrit.Repository.open(str(work / ".git"))
    head_tree = pylibgrit.ObjectId.from_hex(_git(work, git_env, "rev-parse", "HEAD^{tree}"))
    with pytest.raises(ValueError):
        repo.merge_trees(head_tree, head_tree, head_tree, favor="bogus")
```

Note: `repo.tree(...).entries` and `TreeEntry.name`/`.oid`, `repo.blob(...).data` are existing read-core APIs (see `__init__.pyi` `Tree`/`TreeEntry`/`Blob`).

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_merge.py -q -k merge_trees`
Expected: FAIL — no attribute `merge_trees`.

- [ ] **Step 3: Implement**

In `src/index.rs`, add to `impl Index` (the non-`#[pymethods]` inherent block that holds `new_loaded`; place next to `new_loaded`):

```rust
    // AIDEV-NOTE: True when the index holds any unmerged (conflict stage != 0) entry. Used by
    // MergeResult.write_tree to refuse writing a tree from a conflicted merge.
    pub(crate) fn has_unmerged(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.stage() != 0)
    }
```

Also in `src/index.rs`, change the existing `write_tree` method's visibility so `MergeResult` (in `src/merge.rs`) can call it — inside `#[pymethods] impl Index`, change `fn write_tree(&self)` to:

```rust
    pub(crate) fn write_tree(&self) -> PyResult<ObjectId> {
```

(A `#[pymethods]` method may carry a `pub(crate)` modifier; it stays exposed to Python *and* becomes callable from other modules in this crate.)

Create `src/merge.rs`:

```rust
//! Three-way merge surface: `MergeResult` value-object + favor parsing.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};

use crate::objects::ObjectId;

// AIDEV-NOTE: Map the public `favor` string to grit's MergeFavor. None => leave conflict markers
// (default); "ours"/"theirs"/"union" auto-resolve. Anything else is a ValueError.
pub(crate) fn parse_favor(favor: Option<&str>) -> PyResult<grit_lib::merge_file::MergeFavor> {
    use grit_lib::merge_file::MergeFavor;
    Ok(match favor {
        None => MergeFavor::None,
        Some("ours") => MergeFavor::Ours,
        Some("theirs") => MergeFavor::Theirs,
        Some("union") => MergeFavor::Union,
        Some(other) => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "favor must be None, 'ours', 'theirs', or 'union' (got {other:?})"
            )))
        }
    })
}

// AIDEV-NOTE: Returned value-object for a three-way merge. Holds ONE shared Index pyobject (the
// merged index, possibly with unmerged stage entries) so a caller can inspect/resolve it and then
// call write_tree(). `conflicts` is the sorted union of {paths with a stage!=0 index entry} and
// {conflict_content keys}; conflict_map gives the conflict-marker blob per path. has_conflicts is
// the ORIGINAL merge outcome; write_tree re-checks the CURRENT index dynamically.
#[pyclass(module = "pylibgrit._pylibgrit")]
pub struct MergeResult {
    index: Py<crate::index::Index>,
    conflicts: Vec<Vec<u8>>,
    conflict_map: BTreeMap<Vec<u8>, grit_lib::objects::ObjectId>,
    has_conflicts: bool,
}

impl MergeResult {
    pub(crate) fn from_output(
        py: Python<'_>,
        repo: Arc<grit_lib::repo::Repository>,
        output: grit_lib::merge_trees::TreeMergeOutput,
    ) -> PyResult<Self> {
        // Compute conflicted paths BEFORE moving output.index into the Index pyclass.
        let mut paths: std::collections::BTreeSet<Vec<u8>> = std::collections::BTreeSet::new();
        for e in &output.index.entries {
            if e.stage() != 0 {
                paths.insert(e.path.clone());
            }
        }
        for k in output.conflict_content.keys() {
            paths.insert(k.clone());
        }
        let conflicts: Vec<Vec<u8>> = paths.into_iter().collect();
        let has_conflicts = !conflicts.is_empty();
        let index = Py::new(py, crate::index::Index::new_loaded(output.index, repo))?;
        Ok(Self {
            index,
            conflicts,
            conflict_map: output.conflict_content,
            has_conflicts,
        })
    }
}

#[pymethods]
impl MergeResult {
    /// The merged index (may contain unmerged stage entries). Returns the shared Index object.
    #[getter]
    fn index(&self, py: Python<'_>) -> Py<crate::index::Index> {
        self.index.clone_ref(py)
    }

    #[getter]
    fn has_conflicts(&self) -> bool {
        self.has_conflicts
    }

    #[getter]
    fn conflicts<'py>(&self, py: Python<'py>) -> Bound<'py, PyList> {
        PyList::new(py, self.conflicts.iter().map(|p| PyBytes::new(py, p)))
            .expect("PyList::new from owned bytes")
    }

    /// The conflict-marker blob oid for `path`, or None if that path has no marker blob.
    fn conflict_blob(&self, path: Vec<u8>) -> Option<ObjectId> {
        self.conflict_map.get(&path).map(|o| ObjectId::from_inner(*o))
    }

    // AIDEV-NOTE: Write a tree from the (possibly caller-resolved) index. Re-checks the CURRENT
    // index for unmerged entries so a resolved index can succeed and an unresolved one raises.
    fn write_tree(&self, py: Python<'_>) -> PyResult<ObjectId> {
        let idx = self.index.bind(py).borrow();
        if idx.has_unmerged() {
            return Err(crate::error::RepositoryError::new_err(
                "cannot write tree: index has unmerged (conflicted) entries",
            ));
        }
        idx.write_tree()
    }
}
```

Note: `crate::index::Index::write_tree` (made `pub(crate)` above) takes `&self` and returns `PyResult<ObjectId>` — callable directly from Rust as `idx.write_tree()` (where `idx` is the `PyRef<Index>` from `self.index.bind(py).borrow()`).

In `src/repository.rs`, add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Raw three-way tree merge (design §Merge). Produces a MergeResult (merged index +
// conflict report); touches no ref and no work tree. favor: None|"ours"|"theirs"|"union".
#[pyo3(signature = (base, ours, theirs, *, favor=None))]
fn merge_trees(
    &self,
    py: Python<'_>,
    base: &crate::objects::ObjectId,
    ours: &crate::objects::ObjectId,
    theirs: &crate::objects::ObjectId,
    favor: Option<&str>,
) -> PyResult<crate::merge::MergeResult> {
    let fav = crate::merge::parse_favor(favor)?;
    let repo = Arc::clone(&self.inner);
    let (b, o, t) = (base.inner(), ours.inner(), theirs.inner());
    let output = py
        .allow_threads(|| {
            grit_lib::merge_trees::merge_trees_three_way(
                &repo,
                b,
                o,
                t,
                fav,
                grit_lib::merge_trees::WhitespaceMergeOptions::default(),
                None,
                grit_lib::merge_trees::TreeMergeConflictPresentation::default(),
            )
        })
        .map_err(map_err)?;
    crate::merge::MergeResult::from_output(py, Arc::clone(&self.inner), output)
}
```

In `src/lib.rs`: add `mod merge;` (near `mod checkout;`) and `m.add_class::<merge::MergeResult>()?;` (next to the other `add_class` calls).

In `python/pylibgrit/__init__.py`: add `MergeResult` to BOTH the `from pylibgrit._pylibgrit import (...)` block and `__all__` (keep alphabetical: after `IndexEntry`/before `InvalidObjectError` for the import; mirror in `__all__`).

In `python/pylibgrit/__init__.pyi`: add the class (place near `Index`) and the `merge_trees` method on `Repository`:

```python
@final
class MergeResult:
    @property
    def index(self) -> Index: ...
    @property
    def has_conflicts(self) -> bool: ...
    @property
    def conflicts(self) -> list[bytes]: ...
    def conflict_blob(self, path: bytes) -> ObjectId | None: ...
    def write_tree(self) -> ObjectId: ...
```

```python
    def merge_trees(
        self, base: ObjectId, ours: ObjectId, theirs: ObjectId, *, favor: str | None = None
    ) -> MergeResult: ...
```

Ensure `from typing import final` is already imported in the `.pyi` (it is — `IndexEntry` uses `@final`).

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_merge.py -q`
Expected: PASS (merge_trees tests; the oracle test skips if git < 2.38).

- [ ] **Step 5: Full gate suite.** All green (including `stubtest` seeing the new `MergeResult`).

- [ ] **Step 6: Commit**

```bash
git add src/merge.rs src/index.rs src/lib.rs src/repository.rs python/pylibgrit/__init__.py python/pylibgrit/__init__.pyi tests/test_merge.py
git commit -m "feat: MergeResult + merge_trees (three-way tree merge)"
```

---

## Task 6: `merge_commits`

**Files:**
- Modify: `src/merge.rs` (add `tree_of_commit` helper)
- Modify: `src/repository.rs` (add `merge_commits`)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_merge.py` (extend)

- [ ] **Step 1: Write the failing test**

```python
# append to tests/test_merge.py
def test_merge_commits_clean_matches_git(tmp_path, git_env):
    import pylibgrit

    work, a, b, _base = _diamond(tmp_path / "r", git_env)
    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_commits(
        pylibgrit.ObjectId.from_hex(a), pylibgrit.ObjectId.from_hex(b)
    )
    assert res.has_conflicts is False
    got = res.write_tree().hex
    oracle = subprocess.run(
        ["git", "merge-tree", "--write-tree", a, b],
        cwd=work, env=git_env, stdout=subprocess.PIPE,
    )
    if oracle.returncode != 0:
        pytest.skip("git merge-tree --write-tree unavailable (<2.38)")
    assert got == oracle.stdout.decode().strip()


def test_merge_commits_conflict(tmp_path, git_env):
    import pylibgrit

    # Same-file divergent edits on both sides -> conflict.
    work = tmp_path / "r"
    subprocess.run(["git", "init", "-q", "-b", "main", str(work)], env=git_env, check=True)
    (work / "c.txt").write_text("base\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "base")
    base = _git(work, git_env, "rev-parse", "HEAD")
    (work / "c.txt").write_text("ours\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "A")
    a = _git(work, git_env, "rev-parse", "HEAD")
    _git(work, git_env, "checkout", "-q", "-b", "feat", base)
    (work / "c.txt").write_text("theirs\n")
    _git(work, git_env, "add", "-A")
    _git(work, git_env, "commit", "-q", "-m", "B")
    b = _git(work, git_env, "rev-parse", "HEAD")
    repo = pylibgrit.Repository.open(str(work / ".git"))
    res = repo.merge_commits(
        pylibgrit.ObjectId.from_hex(a), pylibgrit.ObjectId.from_hex(b)
    )
    assert res.has_conflicts is True
    assert b"c.txt" in res.conflicts
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_merge.py -q -k merge_commits`
Expected: FAIL — no attribute `merge_commits`.

- [ ] **Step 3: Implement**

In `src/merge.rs`, add:

```rust
// AIDEV-NOTE: Resolve a commit oid to its tree oid (for commit-level merge). Errors (via map_err
// at the call site) if the object is not a commit.
pub(crate) fn tree_of_commit(
    repo: &grit_lib::repo::Repository,
    oid: grit_lib::objects::ObjectId,
) -> Result<grit_lib::objects::ObjectId, grit_lib::error::Error> {
    let obj = repo.odb.read(&oid)?;
    let c = grit_lib::objects::parse_commit(&obj.data)?;
    Ok(c.tree)
}
```

In `src/repository.rs`, add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Commit-level three-way merge. base = first merge_base(ours, theirs); when the two
// are unrelated (no base), use the empty tree as base (additive merge, git's
// --allow-unrelated-histories behaviour). Returns a MergeResult exactly like merge_trees.
#[pyo3(signature = (ours, theirs, *, favor=None))]
fn merge_commits(
    &self,
    py: Python<'_>,
    ours: &crate::objects::ObjectId,
    theirs: &crate::objects::ObjectId,
    favor: Option<&str>,
) -> PyResult<crate::merge::MergeResult> {
    let fav = crate::merge::parse_favor(favor)?;
    let repo = Arc::clone(&self.inner);
    let (oc, tc) = (ours.inner(), theirs.inner());
    let output = py
        .allow_threads(|| -> Result<_, grit_lib::error::Error> {
            let bases = grit_lib::merge_base::merge_bases_all(&repo, &[oc, tc])?;
            let base_tree = match bases.into_iter().next() {
                Some(base_commit) => crate::merge::tree_of_commit(&repo, base_commit)?,
                None => repo.odb.write(
                    grit_lib::objects::ObjectKind::Tree,
                    &grit_lib::objects::serialize_tree(&[]),
                )?,
            };
            let ours_tree = crate::merge::tree_of_commit(&repo, oc)?;
            let theirs_tree = crate::merge::tree_of_commit(&repo, tc)?;
            grit_lib::merge_trees::merge_trees_three_way(
                &repo,
                base_tree,
                ours_tree,
                theirs_tree,
                fav,
                grit_lib::merge_trees::WhitespaceMergeOptions::default(),
                None,
                grit_lib::merge_trees::TreeMergeConflictPresentation::default(),
            )
        })
        .map_err(map_err)?;
    crate::merge::MergeResult::from_output(py, Arc::clone(&self.inner), output)
}
```

Add to the Repository stub in `__init__.pyi` (after `merge_trees`):

```python
    def merge_commits(
        self, ours: ObjectId, theirs: ObjectId, *, favor: str | None = None
    ) -> MergeResult: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_merge.py -q`
Expected: PASS.

- [ ] **Step 5: Full gate suite.** All green.

- [ ] **Step 6: Commit**

```bash
git add src/merge.rs src/repository.rs python/pylibgrit/__init__.pyi tests/test_merge.py
git commit -m "feat: merge_commits (auto merge-base; empty-tree base for unrelated)"
```

---

## Task 7: Atomic-CAS upgrade (`src/refs.rs`) + rewire `update_ref`/`delete_ref`

**Files:**
- Modify: `src/refs.rs` (add `CasError`, `cas_to_pyerr`, `atomic_cas_write`, `atomic_cas_delete`)
- Modify: `src/repository.rs` (rewire the CAS/create paths of `update_ref`/`delete_ref`)
- Test: `tests/test_atomic_cas.py` (create)

No Python-visible signature changes — `update_ref`/`delete_ref` keep their stubs. This task changes the *guarantee* (true atomicity for loose refs) and adds a contention error.

- [ ] **Step 1: Write the failing test**

```python
# tests/test_atomic_cas.py
import threading

import pytest


def _repo_with_main(tmp_path):
    import pylibgrit

    repo = pylibgrit.Repository.init(str(tmp_path / "r"))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    tree = idx.write_tree()
    sig = pylibgrit.Signature(b"A", b"a@x", (1700000000, 0))
    c1 = repo.create_commit(tree, parents=[], author=sig, committer=sig, message=b"c1\n")
    repo.update_ref(b"refs/heads/main", c1, create=True)
    blob2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"y\n")
    idx.add(b"a.txt", blob2, 0o100644)
    tree2 = idx.write_tree()
    c2 = repo.create_commit(tree2, parents=[c1], author=sig, committer=sig, message=b"c2\n")
    return repo, c1, c2


def test_cas_mismatch_raises_and_leaves_ref(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    # expected_old=c2 but ref is c1 -> mismatch.
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.update_ref(b"refs/heads/main", c2, expected_old=c2)
    assert repo.resolve("refs/heads/main") == c1


def test_cas_success_advances(tmp_path):
    repo, c1, c2 = _repo_with_main(tmp_path)
    repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
    assert repo.resolve("refs/heads/main") == c2


def test_create_only_on_existing_raises(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.update_ref(b"refs/heads/main", c2, create=True)


def test_preexisting_lock_is_contention_error(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    git_dir = tmp_path / "r" / ".git"
    lock = git_dir / "refs" / "heads" / "main.lock"
    lock.write_text("")  # simulate another writer holding the lock
    with pytest.raises(pylibgrit.RepositoryError):
        repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
    lock.unlink()
    # No stale lock from our failed attempt (we only created the contention one).
    assert not lock.exists()


def test_threaded_race_exactly_one_winner(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    # Many threads try to advance main from c1 -> c2 with CAS; exactly one wins.
    results = []
    barrier = threading.Barrier(8)

    def attempt():
        barrier.wait()
        try:
            repo.update_ref(b"refs/heads/main", c2, expected_old=c1)
            results.append(True)
        except pylibgrit.RefMismatchError:
            results.append(False)
        except pylibgrit.RepositoryError:
            results.append(False)  # lost the lock race; acceptable

    threads = [threading.Thread(target=attempt) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    assert results.count(True) == 1
    assert repo.resolve("refs/heads/main") == c2


def test_cas_delete_loose(tmp_path):
    import pylibgrit

    repo, c1, c2 = _repo_with_main(tmp_path)
    repo.update_ref(b"refs/tags/v1", c1, create=True)
    # CAS-delete with wrong expected -> mismatch, ref stays.
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.delete_ref(b"refs/tags/v1", expected_old=c2)
    assert repo.resolve("refs/tags/v1") == c1
    # CAS-delete with right expected -> gone.
    repo.delete_ref(b"refs/tags/v1", expected_old=c1)
    with pytest.raises(pylibgrit.GritError):
        repo.resolve("refs/tags/v1")
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_atomic_cas.py -q`
Expected: FAIL — `test_preexisting_lock_is_contention_error` fails (Phase A's best-effort path ignores the lock and succeeds), and possibly the threaded test is flaky (best-effort allows >1 winner).

- [ ] **Step 3: Implement the atomic helpers in `src/refs.rs`**

Add near the top (after the imports) the error type and mapper:

```rust
// AIDEV-NOTE: ATOMIC compare-and-swap over a binding-held ref lockfile (design §4). grit-lib 0.4.1
// exposes no atomic CAS primitive, but it DOES expose the pieces to replicate its own lock
// protocol from outside the crate: resolve_ref_storage (== private ref_storage_dir) + storage_ref_name
// give the exact loose-ref path; lock_path_for_ref + O_CREAT|O_EXCL give the same `<ref>.lock` that
// git and grit's write_ref take. Holding that lock, we read the current value (read_raw_ref for
// existence, resolve_ref for the oid) and write the new value under it — truly atomic against any
// lock-respecting writer. The plain overwrite path (no expected_old, no create) still uses grit's
// write_ref; only create-only/CAS go through here.
pub(crate) enum CasError {
    Mismatch(String),
    Locked(String),
    Grit(grit_lib::error::Error),
    Io(std::io::Error),
}

pub(crate) fn cas_to_pyerr(e: CasError) -> pyo3::PyErr {
    match e {
        CasError::Mismatch(m) => crate::error::RefMismatchError::new_err(m),
        CasError::Locked(m) => crate::error::invalid_ref(&m),
        CasError::Grit(err) => crate::error::map_err(err),
        CasError::Io(io) => match io.raw_os_error() {
            Some(errno) => pyo3::exceptions::PyOSError::new_err((errno, format!("{io}"))),
            None => pyo3::exceptions::PyOSError::new_err(format!("{io}")),
        },
    }
}

// AIDEV-NOTE: Compute the on-disk loose-ref path the SAME way grit's write_ref does (verified:
// ref_storage_dir == worktree_ref::resolve_ref_storage(..).0).
fn loose_ref_path(git_dir: &std::path::Path, refname: &str) -> std::path::PathBuf {
    let (store, _stor) = grit_lib::worktree_ref::resolve_ref_storage(git_dir, refname);
    store.join(grit_lib::ref_namespace::storage_ref_name(refname))
}

// AIDEV-NOTE: Read the current oid UNDER the held lock. NotFound -> None; otherwise resolve.
fn current_under_lock(
    git_dir: &std::path::Path,
    refname: &str,
) -> Result<Option<grit_lib::objects::ObjectId>, CasError> {
    match grit_lib::refs::read_raw_ref(git_dir, refname).map_err(CasError::Grit)? {
        grit_lib::refs::RawRefLookup::NotFound => Ok(None),
        _ => Ok(Some(
            grit_lib::refs::resolve_ref(git_dir, refname).map_err(CasError::Grit)?,
        )),
    }
}

// AIDEV-NOTE: Acquire the `<ref>.lock` with O_CREAT|O_EXCL (the same protocol git + grit's
// write_ref use). A pre-existing lock means another writer holds it -> Locked (contention).
fn acquire_ref_lock(lock: &std::path::Path, refname: &str) -> Result<std::fs::File, CasError> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock)
    {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(CasError::Locked(format!("cannot lock ref '{refname}'")))
        }
        Err(e) => Err(CasError::Io(e)),
    }
}

// AIDEV-NOTE: Verify create-only / compare-and-swap against the value read UNDER the held lock,
// then write the new oid into the still-held lock file. Named (not an inline closure) to keep
// clippy happy under -D warnings and to centralize the verify+write. Returns the previous oid.
fn cas_verify_and_write(
    file: &mut std::fs::File,
    git_dir: &std::path::Path,
    refname: &str,
    new_oid: &grit_lib::objects::ObjectId,
    expected_old: Option<&grit_lib::objects::ObjectId>,
    create_only: bool,
) -> Result<Option<grit_lib::objects::ObjectId>, CasError> {
    let current = current_under_lock(git_dir, refname)?;
    if create_only {
        if current.is_some() {
            return Err(CasError::Mismatch(format!("ref {refname} already exists")));
        }
    } else if let Some(exp) = expected_old {
        match &current {
            Some(cur) if cur == exp => {}
            Some(cur) => {
                return Err(CasError::Mismatch(format!(
                    "ref {refname} is {}, expected {}",
                    cur.to_hex(),
                    exp.to_hex()
                )))
            }
            None => {
                return Err(CasError::Mismatch(format!(
                    "ref {refname} does not exist, expected {}",
                    exp.to_hex()
                )))
            }
        }
    }
    use std::io::Write as _;
    file.write_all(format!("{new_oid}\n").as_bytes())
        .map_err(CasError::Io)?;
    file.sync_all().map_err(CasError::Io)?;
    Ok(current)
}

// AIDEV-NOTE: Atomic create-only / compare-and-swap / overwrite write. Returns the PREVIOUS oid
// (None if the ref was absent) so the caller can log old->new. Lock contention surfaces as Locked.
// Never leaves a stale lock: any error after acquiring removes it.
pub(crate) fn atomic_cas_write(
    git_dir: &std::path::Path,
    refname: &str,
    new_oid: &grit_lib::objects::ObjectId,
    expected_old: Option<&grit_lib::objects::ObjectId>,
    create_only: bool,
) -> Result<Option<grit_lib::objects::ObjectId>, CasError> {
    let path = loose_ref_path(git_dir, refname);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CasError::Io)?;
    }
    let lock = grit_lib::refs::lock_path_for_ref(&path);
    let mut file = acquire_ref_lock(&lock, refname)?;

    let outcome = cas_verify_and_write(&mut file, git_dir, refname, new_oid, expected_old, create_only);
    // Close the handle before renaming/removing.
    drop(file);
    match outcome {
        Ok(prev) => {
            std::fs::rename(&lock, &path).map_err(CasError::Io)?;
            Ok(prev)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&lock);
            Err(e)
        }
    }
}

// AIDEV-NOTE: Atomic compare-and-swap DELETE. Verifies current==expected under the held lock.
// Loose-only refs are deleted atomically (unlink under lock). If a packed-refs entry also exists,
// the packed removal is delegated to grit's delete_ref AFTER the verify (a small documented
// residual window for the packed case — grit's packed deletion takes its own lock).
pub(crate) fn atomic_cas_delete(
    git_dir: &std::path::Path,
    refname: &str,
    expected_old: &grit_lib::objects::ObjectId,
) -> Result<(), CasError> {
    let path = loose_ref_path(git_dir, refname);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CasError::Io)?;
    }
    let lock = grit_lib::refs::lock_path_for_ref(&path);
    let file = acquire_ref_lock(&lock, refname)?;

    // Verify under the lock; on any failure, drop the lock and report.
    let current = match current_under_lock(git_dir, refname) {
        Ok(c) => c,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&lock);
            return Err(e);
        }
    };
    let matches = matches!(&current, Some(cur) if cur == expected_old);
    if !matches {
        drop(file);
        let _ = std::fs::remove_file(&lock);
        let msg = match &current {
            Some(cur) => format!(
                "ref {refname} is {}, expected {}",
                cur.to_hex(),
                expected_old.to_hex()
            ),
            None => format!(
                "ref {refname} does not exist, expected {}",
                expected_old.to_hex()
            ),
        };
        return Err(CasError::Mismatch(msg));
    }

    // Verified. Remove the loose file (atomic under lock) if present.
    let loose_existed = std::fs::symlink_metadata(&path).is_ok();
    if loose_existed {
        if let Err(e) = std::fs::remove_file(&path) {
            drop(file);
            let _ = std::fs::remove_file(&lock);
            return Err(CasError::Io(e));
        }
    }
    // Release our lock, then clean up any packed entry via grit (its own lock).
    drop(file);
    let _ = std::fs::remove_file(&lock);
    let packed = grit_lib::refs::packed_refs_entry_exists(git_dir, refname).unwrap_or(false);
    if packed || !loose_existed {
        grit_lib::refs::delete_ref(git_dir, refname).map_err(CasError::Grit)?;
    }
    Ok(())
}
```

Rewire `update_ref` in `src/repository.rs` — replace the body from the `let current = ...` read through the `write_ref` call (keep the `if create && expected_old.is_some()` guard, the `validate_ref_name`, `reflog_args`, `git_dir`, `new_oid` lines, and the trailing reflog block):

```rust
    let old_for_log = if create || expected_old.is_some() {
        // AIDEV-NOTE: Atomic path — create-only or compare-and-swap via a held lockfile.
        let exp = expected_old.as_ref().map(|o| o.inner());
        let prev = py
            .allow_threads(|| {
                crate::refs::atomic_cas_write(&git_dir, &refname, &new_oid, exp.as_ref(), create)
            })
            .map_err(crate::refs::cas_to_pyerr)?;
        prev.unwrap_or_else(|| crate::refs::zero_like(&new_oid))
    } else {
        // Plain overwrite (plumbing-faithful, unchanged): grit's write_ref locks atomically.
        let current = py.allow_threads(|| crate::refs::read_current_oid(&git_dir, &refname));
        py.allow_threads(|| grit_lib::refs::write_ref(&git_dir, &refname, &new_oid))
            .map_err(map_err)?;
        current.unwrap_or_else(|| crate::refs::zero_like(&new_oid))
    };
```

(The existing `if let Some((ident, msg)) = reflog { ... append_reflog ... }` block stays and now uses this `old_for_log`.)

Rewire `delete_ref` in `src/repository.rs` — replace the manual `expected_old` compare block and the final `delete_ref` call. Keep `validate_ref_name`, `git_dir`, `reflog_args`, the `current` read, and the reflog-before-delete block. Replace the comparison + delete tail with:

```rust
    // AIDEV-NOTE: CAS delete goes through the atomic held-lock path; an unconditional delete uses
    // grit's delete_ref directly. (The reflog-before-delete block above already ran when requested.)
    match &expected_old {
        Some(exp) => {
            let exp_oid = exp.inner();
            py.allow_threads(|| crate::refs::atomic_cas_delete(&git_dir, &refname, &exp_oid))
                .map_err(crate::refs::cas_to_pyerr)
        }
        None => py
            .allow_threads(|| grit_lib::refs::delete_ref(&git_dir, &refname))
            .map_err(map_err),
    }
```

Remove the now-unused manual `if let Some(exp) = &expected_old { ... RefMismatchError ... }` block that preceded the reflog block in `delete_ref` (the atomic path enforces it). Keep the `current` read (the reflog block still uses it).

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_atomic_cas.py tests/test_ref_write.py tests/test_reflog.py -q`
Expected: PASS (new atomic tests + existing ref/reflog tests still green — the public behavior is unchanged except the added contention error and stronger atomicity).

- [ ] **Step 5: Full gate suite.** All green. Watch clippy: prefer `&cur == exp` over `*cur == *exp`; the immediately-invoked closures are idiomatic but if clippy flags `redundant_closure_call`, extract to a named inner `fn`/block.

- [ ] **Step 6: Commit**

```bash
git add src/refs.rs src/repository.rs tests/test_atomic_cas.py
git commit -m "feat: atomic ref CAS via binding-held lockfile (update/create + loose delete)"
```

---

## Task 8: `commit_index` (commit-and-advance-branch porcelain)

**Files:**
- Modify: `src/repository.rs` (add `commit_index` + a `first_line` helper)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_commit_index.py` (create)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_commit_index.py
import subprocess

import pytest


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def test_commit_index_first_commit_unborn(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"hello\n")
    idx = repo.index()
    idx.add(b"a.txt", blob, 0o100644)
    idx.write()
    sig = pylibgrit.Signature(b"Test Author", b"author@example.com", (1112911993, 0))
    com = pylibgrit.Signature(b"Test Committer", b"committer@example.com", (1112911993, 0))
    oid = repo.commit_index(message=b"initial\n", author=sig, committer=com)
    # Branch advanced from unborn.
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == oid.hex
    # No parents on the first commit.
    parents = subprocess.run(
        ["git", "rev-list", "--parents", "-n", "1", oid.hex],
        cwd=work, env=git_env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().split()
    assert parents == [oid.hex]  # just the commit, no parents
    # Reflog entry exists.
    reflog = _git(work, git_env, "reflog", "show", "refs/heads/main")
    assert "initial" in reflog


def test_commit_index_advances_with_parent(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    b2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"two\n")
    idx.add(b"a.txt", b2, 0o100644)
    idx.write()
    c2 = repo.commit_index(message=b"two\n", author=sig, committer=sig)
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == c2.hex
    parents = subprocess.run(
        ["git", "rev-list", "--parents", "-n", "1", c2.hex],
        cwd=work, env=git_env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().split()
    assert parents == [c2.hex, c1.hex]


def test_commit_index_merge_extra_parents(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    # A side commit (not on the branch) to use as an extra parent.
    side = repo.create_commit(
        repo.commit(c1).tree, parents=[c1], author=sig, committer=sig, message=b"side\n"
    )
    b2 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"merged\n")
    idx.add(b"a.txt", b2, 0o100644)
    idx.write()
    merge = repo.commit_index(message=b"merge\n", parents=[side], author=sig, committer=sig)
    parents = subprocess.run(
        ["git", "rev-list", "--parents", "-n", "1", merge.hex],
        cwd=work, env=git_env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().split()
    assert parents == [merge.hex, c1.hex, side.hex]  # branch tip first, then extra


def test_commit_index_detached_head_raises(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"one\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    repo.set_head(c1.hex.encode())  # detach HEAD onto the oid directly is not symbolic;
    # set_head writes a symbolic ref, so instead detach via git:
    subprocess.run(["git", "checkout", "-q", "--detach"], cwd=work, env=git_env, check=True)
    with pytest.raises(pylibgrit.RepositoryError):
        repo.commit_index(message=b"x\n", author=sig, committer=sig)
```

Note: drop the misleading `set_head` line if it does not detach — the `git checkout --detach` is the real detach. (Implementer: keep only the `git checkout --detach` step; remove the `repo.set_head(...)` line.)

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_commit_index.py -q`
Expected: FAIL — no attribute `commit_index`.

- [ ] **Step 3: Implement**

Add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Commit-and-advance-branch porcelain (== `git commit`). Resolves HEAD's branch
// (must be symbolic — detached HEAD raises), writes a tree from the repo index, builds a commit
// whose FIRST parent is the current branch tip (none if the branch is unborn) plus any `parents=`
// extras (merge commits), writes it, atomically advances the branch (create-only if unborn, CAS
// on the tip otherwise), and appends a `commit:`/`commit (initial):` reflog entry with the
// committer identity. Identity rules match create_commit (Signature XOR *_raw).
#[pyo3(signature = (*, message, parents=None, author=None, committer=None,
                    author_raw=None, committer_raw=None, encoding=None))]
#[allow(clippy::too_many_arguments)]
fn commit_index(
    &self,
    py: Python<'_>,
    message: Vec<u8>,
    parents: Option<Vec<crate::objects::ObjectId>>,
    author: Option<PyRef<'_, crate::objects::Signature>>,
    committer: Option<PyRef<'_, crate::objects::Signature>>,
    author_raw: Option<Vec<u8>>,
    committer_raw: Option<Vec<u8>>,
    encoding: Option<String>,
) -> PyResult<crate::objects::ObjectId> {
    let author_bytes = crate::objects::resolve_ident("author", author.as_deref(), author_raw)?;
    let committer_bytes =
        crate::objects::resolve_ident("committer", committer.as_deref(), committer_raw)?;
    let git_dir = self.inner.git_dir.clone();
    let repo = Arc::clone(&self.inner);

    // Resolve HEAD -> branch (symbolic required).
    let branch = py
        .allow_threads(|| grit_lib::refs::read_head(&git_dir))
        .map_err(map_err)?
        .ok_or_else(|| {
            crate::error::invalid_ref("HEAD is detached; commit_index requires HEAD on a branch")
        })?;

    // Current branch tip (None if unborn).
    let tip = py
        .allow_threads(|| -> Result<Option<grit_lib::objects::ObjectId>, grit_lib::error::Error> {
            match grit_lib::refs::read_raw_ref(&git_dir, &branch)? {
                grit_lib::refs::RawRefLookup::NotFound => Ok(None),
                _ => grit_lib::refs::resolve_ref(&git_dir, &branch).map(Some),
            }
        })
        .map_err(map_err)?;

    // Tree from the repo index.
    let tree = py
        .allow_threads(|| -> Result<grit_lib::objects::ObjectId, grit_lib::error::Error> {
            let index = repo.load_index()?;
            grit_lib::write_tree::write_tree_from_index(&repo.odb, &index, "")
        })
        .map_err(map_err)?;

    // Parents: branch tip first (if any), then caller extras.
    let mut parent_oids: Vec<grit_lib::objects::ObjectId> = Vec::new();
    if let Some(t) = &tip {
        parent_oids.push(*t);
    }
    if let Some(extra) = &parents {
        parent_oids.extend(extra.iter().map(|p| p.inner()));
    }

    let cdata = grit_lib::objects::CommitData {
        tree,
        parents: parent_oids,
        author: String::new(),
        committer: String::new(),
        author_raw: author_bytes,
        committer_raw: committer_bytes.clone(),
        encoding,
        message: String::new(),
        raw_message: Some(message.clone()),
    };
    let raw = grit_lib::objects::serialize_commit(&cdata);
    let new_oid = py
        .allow_threads(|| repo.odb.write(grit_lib::objects::ObjectKind::Commit, &raw))
        .map_err(map_err)?;

    // Atomically advance the branch: create-only if unborn, else CAS on the tip we read.
    let exp = tip;
    let create = exp.is_none();
    py.allow_threads(|| {
        crate::refs::atomic_cas_write(&git_dir, &branch, &new_oid, exp.as_ref(), create)
    })
    .map_err(crate::refs::cas_to_pyerr)?;

    // Reflog: porcelain always logs (force_create). "commit (initial): <subject>" when unborn.
    let subject = first_line(&message);
    let prefix = if exp.is_none() {
        "commit (initial): "
    } else {
        "commit: "
    };
    let log_msg = format!("{prefix}{subject}");
    let ident = utf8_field("committer", committer_bytes)?;
    let old_for_log = exp.unwrap_or_else(|| crate::refs::zero_like(&new_oid));
    py.allow_threads(|| {
        grit_lib::refs::append_reflog(
            &git_dir,
            &branch,
            &old_for_log,
            &new_oid,
            &ident,
            &log_msg,
            true,
        )
    })
    .map_err(map_err)?;

    Ok(crate::objects::ObjectId::from_inner(new_oid))
}
```

Add the `first_line` free helper near `utf8_field` at the bottom of `src/repository.rs`:

```rust
// AIDEV-NOTE: First line of a commit message (the reflog subject). UTF-8-lossy and single-line by
// construction (we cut at the first '\n'), so it is safe as a one-line reflog record.
fn first_line(msg: &[u8]) -> String {
    let end = msg.iter().position(|&b| b == b'\n').unwrap_or(msg.len());
    String::from_utf8_lossy(&msg[..end]).into_owned()
}
```

Add to the Repository stub in `__init__.pyi` (after `create_tag` or near `create_commit`):

```python
    def commit_index(
        self,
        *,
        message: bytes,
        parents: list[ObjectId] | None = None,
        author: Signature | None = None,
        committer: Signature | None = None,
        author_raw: bytes | None = None,
        committer_raw: bytes | None = None,
        encoding: bytes | None = None,
    ) -> ObjectId: ...
```

(Note: `encoding` is `Option<String>` in Rust → stub as `bytes | None`? Use `str | None` to match `Option<String>`. The existing `create_commit` stub uses `encoding: str | None = None` — match that: use `encoding: str | None = None`.)

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_commit_index.py -q`
Expected: PASS.

- [ ] **Step 5: Full gate suite.** All green.

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_commit_index.py
git commit -m "feat: commit_index (commit + atomic branch advance + reflog)"
```

---

## Task 9: Tag-ref porcelain (`create_lightweight_tag`, `create_annotated_tag`)

**Files:**
- Modify: `src/repository.rs` (add the two methods + a `tag_refname` helper)
- Modify: `python/pylibgrit/__init__.pyi`
- Test: `tests/test_tag_ref.py` (create)

- [ ] **Step 1: Write the failing test**

```python
# tests/test_tag_ref.py
import subprocess

import pytest


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def _repo_one_commit(tmp_path, git_env):
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work))
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))
    b1 = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"x\n")
    idx = repo.index()
    idx.add(b"a.txt", b1, 0o100644)
    idx.write()
    c1 = repo.commit_index(message=b"one\n", author=sig, committer=sig)
    return repo, work, c1, sig


def test_lightweight_tag(tmp_path, git_env):
    import pylibgrit

    repo, work, c1, _sig = _repo_one_commit(tmp_path, git_env)
    repo.create_lightweight_tag(b"v1", c1)
    assert _git(work, git_env, "rev-parse", "refs/tags/v1") == c1.hex
    # create again without force -> RefMismatchError.
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.create_lightweight_tag(b"v1", c1)


def test_annotated_tag(tmp_path, git_env):
    import pylibgrit

    repo, work, c1, sig = _repo_one_commit(tmp_path, git_env)
    tag_oid = repo.create_annotated_tag(
        b"v2", c1, pylibgrit.ObjectKind.COMMIT, message=b"release 2\n", tagger=sig
    )
    # refs/tags/v2 points at the tag OBJECT, which peels to c1.
    assert _git(work, git_env, "rev-parse", "refs/tags/v2") == tag_oid.hex
    assert _git(work, git_env, "rev-parse", "refs/tags/v2^{commit}") == c1.hex
    assert _git(work, git_env, "cat-file", "-t", tag_oid.hex) == "tag"


def test_annotated_tag_force_moves(tmp_path, git_env):
    import pylibgrit

    repo, work, c1, sig = _repo_one_commit(tmp_path, git_env)
    repo.create_lightweight_tag(b"v3", c1)
    with pytest.raises(pylibgrit.RefMismatchError):
        repo.create_annotated_tag(
            b"v3", c1, pylibgrit.ObjectKind.COMMIT, message=b"m\n", tagger=sig
        )
    tag_oid = repo.create_annotated_tag(
        b"v3", c1, pylibgrit.ObjectKind.COMMIT, message=b"m\n", tagger=sig, force=True
    )
    assert _git(work, git_env, "rev-parse", "refs/tags/v3") == tag_oid.hex
```

- [ ] **Step 2: Run to verify it fails**

Run: `uv run pytest tests/test_tag_ref.py -q`
Expected: FAIL — no attribute `create_lightweight_tag`.

- [ ] **Step 3: Implement**

Add inside `#[pymethods] impl Repository`:

```rust
// AIDEV-NOTE: Lightweight tag = a plain ref refs/tags/<name> -> target oid. Atomic create-only by
// default; force=True overwrites (moves the tag). No tag object is created.
#[pyo3(signature = (name, target, *, force=false))]
fn create_lightweight_tag(
    &self,
    py: Python<'_>,
    name: Vec<u8>,
    target: &crate::objects::ObjectId,
    force: bool,
) -> PyResult<()> {
    let refname = tag_refname(&name)?;
    let git_dir = self.inner.git_dir.clone();
    let new_oid = target.inner();
    py.allow_threads(|| crate::refs::atomic_cas_write(&git_dir, &refname, &new_oid, None, !force))
        .map_err(crate::refs::cas_to_pyerr)?;
    Ok(())
}

// AIDEV-NOTE: Annotated tag = create the tag OBJECT (via create_tag) then point refs/tags/<name>
// at it. Atomic create-only unless force=True. Returns the tag-object oid.
#[pyo3(signature = (name, target, target_kind, *, message, tagger=None, tagger_raw=None, force=false))]
#[allow(clippy::too_many_arguments)]
fn create_annotated_tag(
    &self,
    py: Python<'_>,
    name: Vec<u8>,
    target: &crate::objects::ObjectId,
    target_kind: &Bound<'_, PyAny>,
    message: Vec<u8>,
    tagger: Option<PyRef<'_, crate::objects::Signature>>,
    tagger_raw: Option<Vec<u8>>,
    force: bool,
) -> PyResult<crate::objects::ObjectId> {
    // Validate the ref name up front so a bad tag name fails before writing the object.
    let refname = tag_refname(&name)?;
    let tag_oid = self.create_tag(py, target, target_kind, name, message, tagger, tagger_raw)?;
    let git_dir = self.inner.git_dir.clone();
    let oid = tag_oid.inner();
    py.allow_threads(|| crate::refs::atomic_cas_write(&git_dir, &refname, &oid, None, !force))
        .map_err(crate::refs::cas_to_pyerr)?;
    Ok(tag_oid)
}
```

Add the `tag_refname` free helper near `validate_ref_name` at the bottom of `src/repository.rs`:

```rust
// AIDEV-NOTE: Build and validate refs/tags/<name>. Reuses validate_ref_name (UTF-8 + git ref
// format), so e.g. a name with spaces or ".." is rejected before any write.
fn tag_refname(name: &[u8]) -> PyResult<String> {
    let mut full = b"refs/tags/".to_vec();
    full.extend_from_slice(name);
    validate_ref_name(&full)
}
```

Add to the Repository stub in `__init__.pyi` (near `create_tag`):

```python
    def create_lightweight_tag(
        self, name: bytes, target: ObjectId, *, force: bool = False
    ) -> None: ...
    def create_annotated_tag(
        self,
        name: bytes,
        target: ObjectId,
        target_kind: ObjectKind,
        *,
        message: bytes,
        tagger: Signature | None = None,
        tagger_raw: bytes | None = None,
        force: bool = False,
    ) -> ObjectId: ...
```

- [ ] **Step 4: Run to verify it passes**

Run: `uv run pytest tests/test_tag_ref.py -q`
Expected: PASS.

- [ ] **Step 5: Full gate suite.** All green.

- [ ] **Step 6: Commit**

```bash
git add src/repository.rs python/pylibgrit/__init__.pyi tests/test_tag_ref.py
git commit -m "feat: lightweight + annotated tag-ref porcelain"
```

---

## Task 10: End-to-end smoke + holistic gate pass

**Files:**
- Test: `tests/test_worktree_merge_smoke.py` (create)

- [ ] **Step 1: Write the smoke test**

```python
# tests/test_worktree_merge_smoke.py
import subprocess


def _git(repo, env, *args):
    return (
        subprocess.run(
            ["git", *args], cwd=repo, env=env, stdout=subprocess.PIPE, check=True
        )
        .stdout.decode()
        .strip()
    )


def test_init_commit_checkout_merge_end_to_end(tmp_path, git_env):
    """init -> stage -> commit_index -> checkout -> branch -> merge -> commit merge."""
    import pylibgrit

    work = tmp_path / "r"
    repo = pylibgrit.Repository.init(str(work), initial_branch=b"main")
    sig = pylibgrit.Signature(b"A", b"a@x", (1112911993, 0))

    # base commit on main
    blob = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"base\n")
    idx = repo.index()
    idx.add(b"f.txt", blob, 0o100644)
    idx.write()
    base = repo.commit_index(message=b"base\n", author=sig, committer=sig)

    # materialize the work tree, git agrees it is the committed content
    repo.checkout_tree(repo.commit(base).tree)
    assert (work / "f.txt").read_bytes() == b"base\n"

    # ours: add a.txt on main
    ba = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"a\n")
    idx.add(b"a.txt", ba, 0o100644)
    idx.write()
    ours = repo.commit_index(message=b"A\n", author=sig, committer=sig)

    # theirs: a side branch off base that adds b.txt
    repo.update_ref(b"refs/heads/feat", base, create=True)
    bb = repo.odb.write(pylibgrit.ObjectKind.BLOB, b"b\n")
    # build theirs tree from base + b.txt directly via a fresh index state
    fidx = repo.index()
    fidx.add(b"f.txt", blob, 0o100644)
    fidx.add(b"b.txt", bb, 0o100644)
    theirs_tree = fidx.write_tree()
    theirs = repo.create_commit(
        theirs_tree, parents=[base], author=sig, committer=sig, message=b"B\n"
    )
    repo.update_ref(b"refs/heads/feat", theirs, expected_old=base)

    # merge feat into main (clean)
    res = repo.merge_commits(ours, theirs)
    assert res.has_conflicts is False
    merged_tree = res.write_tree()

    # commit the merge on main (HEAD is on main, tip = ours)
    # write the merged tree into the index first so commit_index uses it
    # (simplest: checkout merged tree updates the index, then commit_index)
    repo.checkout_tree(merged_tree, force=True, update_index=True)
    merge_commit = repo.commit_index(
        message=b"merge feat\n", parents=[theirs], author=sig, committer=sig
    )

    # git sees a real merge commit with both parents on main
    assert _git(work, git_env, "rev-parse", "refs/heads/main") == merge_commit.hex
    parents = subprocess.run(
        ["git", "rev-list", "--parents", "-n", "1", merge_commit.hex],
        cwd=work, env=git_env, stdout=subprocess.PIPE, check=True,
    ).stdout.decode().split()
    assert parents == [merge_commit.hex, ours.hex, theirs.hex]
    # all three files present in the merged tree
    names = _git(work, git_env, "ls-tree", "--name-only", merge_commit.hex).split()
    assert set(names) == {"a.txt", "b.txt", "f.txt"}
```

- [ ] **Step 2: Run to verify it fails** (before this task's deps exist it would, but all deps are done — so it should pass directly)

Run: `uv run pytest tests/test_worktree_merge_smoke.py -q`
Expected: PASS (all prior tasks complete).

- [ ] **Step 3: No new implementation** — this task verifies integration only. If the smoke test reveals a gap, fix it in the owning module and note it.

- [ ] **Step 4: Run the entire suite + every gate**

```bash
uv run maturin develop --uv --locked
uv run pytest -q
uv run mypy python tests
uv run python -m mypy.stubtest pylibgrit
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
uv run ruff format --check . && uv run ruff check .
```
Expected: all green; the full prior suite (Phase A) still passes unchanged.

- [ ] **Step 5: Commit**

```bash
git add tests/test_worktree_merge_smoke.py
git commit -m "test: end-to-end init->commit->checkout->merge smoke"
```

---

## Plan self-review

**Spec coverage (every §2 surface has a task):**
- `Repository.init` → Task 1 ✓
- `write_to_worktree` → Task 2 ✓
- `checkout_tree` (overlay, force, update_index, gitlink skip, bare guard) → Task 3 ✓
- `merge_base` → Task 4 ✓
- `merge_trees` + `MergeResult` (index/has_conflicts/conflicts/conflict_blob/write_tree) → Task 5 ✓
- `merge_commits` → Task 6 ✓
- Atomic-CAS upgrade (update/create + delete; contention error) → Task 7 ✓
- `commit_index` (unborn, parent, merge parents, reflog, detached-HEAD guard) → Task 8 ✓
- `create_lightweight_tag` / `create_annotated_tag` (force) → Task 9 ✓
- End-to-end integration → Task 10 ✓
- Error handling: `FileExistsError` (Task 3), `RefMismatchError`/contention `RepositoryError` (Task 7), `ValueError` bad favor (Task 5), detached-HEAD `RepositoryError` (Task 8) ✓
- Testing strategy (git oracle, `merge-tree --write-tree` ≥2.38 skip) ✓

**Type/name consistency:** `MergeResult` props (`index`/`has_conflicts`/`conflicts`/`conflict_blob`/`write_tree`) identical across the `.pyi`, the Rust pyclass, and tests. `atomic_cas_write`/`atomic_cas_delete`/`cas_to_pyerr`/`CasError` names consistent between `refs.rs` and the `repository.rs` call sites. `checkout_tree_method`/`checkout_tree`/`to_pyerr`/`CheckoutError` consistent. `MODE_TREE`/`MODE_GITLINK` used by name (grit constants). `favor` strings `None|"ours"|"theirs"|"union"` consistent between `parse_favor`, `.pyi`, and tests.

**No placeholders:** every code step has complete code; every run step has an exact command and expected result. One intentional in-test note (Task 8 `set_head` line) is called out for the implementer to remove — the real detach uses `git checkout --detach`.

**Known follow-up (NOT in this plan):** README "Worktree & merge" docs, CHANGELOG entry, and the version bump to **0.3.0** + release are a separate "ship it" step after merge (mirrors how Phase A shipped 0.2.0 as a follow-up).
