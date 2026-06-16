//! Reference views and an owning iterator over a repository's refs.

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::map_err;
use crate::objects::ObjectId;

// AIDEV-NOTE: Owning-iterator design (design §6), mirroring objects::Tree/TreeIter. grit's
// `list_refs` returns OWNED `(String, ObjectId)` pairs, which we copy into
// `Arc<[ReferenceData]>`. A `ReferenceIter` holds that Arc plus an `Arc` of the repo; each
// yielded `Reference` clones one `ReferenceData` and an `Arc<Repository>`. So a `Reference`
// (and the iterator) own ALL their data and stay valid after the parent `Repository` is
// dropped. The repo Arc is kept so `peel()` can resolve symbolic refs against `git_dir`.
//
// AIDEV-NOTE: HEAD/symbolic handling: a ref is EITHER direct (`target` is Some, holding the
// resolved oid) OR symbolic (`symbolic_target` is Some, holding e.g. b"refs/heads/main");
// exactly one is set. `list_refs` only yields direct refs (it resolves and excludes HEAD).
// Symbolic `Reference`s are produced only by `Repository::head()` (see repository.rs), which
// reads HEAD via `read_head`. `peel()` follows a symbolic ref to its final oid.
#[derive(Clone)]
pub struct ReferenceData {
    name: Vec<u8>,
    target: Option<grit_lib::objects::ObjectId>, // direct oid; None for symbolic
    symbolic_target: Option<Vec<u8>>,            // e.g. b"refs/heads/main"; None for direct
}

impl ReferenceData {
    /// A direct reference: `name` resolved to `oid`.
    pub fn direct(name: Vec<u8>, oid: grit_lib::objects::ObjectId) -> Self {
        Self {
            name,
            target: Some(oid),
            symbolic_target: None,
        }
    }

    /// A symbolic reference: `name` points at another ref `symbolic_target`.
    pub fn symbolic(name: Vec<u8>, symbolic_target: Vec<u8>) -> Self {
        Self {
            name,
            target: None,
            symbolic_target: Some(symbolic_target),
        }
    }
}

/// A single Git reference: a name plus either a direct oid or a symbolic target.
#[pyclass(frozen, module = "pylibgrit._pylibgrit")]
pub struct Reference {
    repo: Arc<grit_lib::repo::Repository>, // so peel() can resolve symbolic refs
    data: ReferenceData,
}

impl Reference {
    pub fn new(repo: Arc<grit_lib::repo::Repository>, data: ReferenceData) -> Self {
        Self { repo, data }
    }
}

#[pymethods]
impl Reference {
    /// The full ref name as raw bytes (e.g. `b"refs/heads/main"`, `b"HEAD"`; design §5).
    #[getter]
    fn name<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.data.name)
    }

    /// The direct target oid, or `None` for a symbolic ref.
    #[getter]
    fn target(&self) -> Option<ObjectId> {
        self.data.target.map(ObjectId::from_inner)
    }

    /// The symbolic target ref name as raw bytes, or `None` for a direct ref.
    #[getter]
    fn symbolic_target<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.data
            .symbolic_target
            .as_ref()
            .map(|s| PyBytes::new(py, s))
    }

    /// Whether this reference is symbolic (points at another ref).
    #[getter]
    fn is_symbolic(&self) -> bool {
        self.data.symbolic_target.is_some()
    }

    /// Resolve to a final object id (follows symbolic refs).
    fn peel(&self, py: Python<'_>) -> PyResult<ObjectId> {
        if let Some(oid) = self.data.target {
            return Ok(ObjectId::from_inner(oid));
        }
        // Symbolic: resolve via the ref name. Ref names are UTF-8 in practice.
        let name = std::str::from_utf8(&self.data.name)
            .map_err(|_| crate::error::invalid_ref("non-UTF-8 ref name"))?
            .to_owned();
        let git_dir = self.repo.git_dir.clone();
        let oid = py
            .allow_threads(|| grit_lib::refs::resolve_ref(&git_dir, &name))
            .map_err(map_err)?;
        Ok(ObjectId::from_inner(oid))
    }
}

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
// reflog "old" value when creating a previously-absent ref (update_ref/delete_ref reflog wiring).
pub(crate) fn zero_like(like: &grit_lib::objects::ObjectId) -> grit_lib::objects::ObjectId {
    grit_lib::objects::ObjectId::from_bytes(&vec![0u8; like.as_bytes().len()])
        .expect("all-zero buffer of valid width is a valid ObjectId")
}

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

    let outcome = cas_verify_and_write(
        &mut file,
        git_dir,
        refname,
        new_oid,
        expected_old,
        create_only,
    );
    // Close the handle before renaming/removing.
    drop(file);
    match outcome {
        Ok(prev) => {
            // AIDEV-NOTE: A rename failure must NOT leave a stale `<ref>.lock` (the "never leave a
            // stale lock on any error path" invariant): remove the lock before propagating.
            if let Err(e) = std::fs::rename(&lock, &path) {
                let _ = std::fs::remove_file(&lock);
                return Err(CasError::Io(e));
            }
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

    let loose_existed = std::fs::symlink_metadata(&path).is_ok();
    if loose_existed {
        if let Err(e) = std::fs::remove_file(&path) {
            drop(file);
            let _ = std::fs::remove_file(&lock);
            return Err(CasError::Io(e));
        }
    }
    drop(file);
    let _ = std::fs::remove_file(&lock);
    let packed = grit_lib::refs::packed_refs_entry_exists(git_dir, refname).unwrap_or(false);
    if packed || !loose_existed {
        grit_lib::refs::delete_ref(git_dir, refname).map_err(CasError::Grit)?;
    }
    Ok(())
}

/// Iterator over a repository's references; owns its own `Arc`s so it outlives the parent.
#[pyclass(module = "pylibgrit._pylibgrit")]
pub struct ReferenceIter {
    repo: Arc<grit_lib::repo::Repository>,
    entries: Arc<[ReferenceData]>,
    idx: usize,
}

impl ReferenceIter {
    pub fn new(repo: Arc<grit_lib::repo::Repository>, entries: Vec<ReferenceData>) -> Self {
        Self {
            repo,
            entries: Arc::from(entries),
            idx: 0,
        }
    }
}

#[pymethods]
impl ReferenceIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> Option<Reference> {
        let d = self.entries.get(self.idx)?.clone();
        self.idx += 1;
        Some(Reference {
            repo: Arc::clone(&self.repo),
            data: d,
        })
    }
}
