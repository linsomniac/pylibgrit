//! Python wrappers over grit-lib's index (`Index`, `IndexEntry`) write surface.

use std::sync::{Arc, Mutex};

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::map_err;
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

// AIDEV-NOTE: `Index` owns a grit_lib::index::Index behind a Mutex (binding-owned mutable
// value; grit's Index mutators take &mut self) plus an Arc<Repository> so write_tree (Task 5) can
// reach the odb and write() can target the repo's default index path. Index methods run UNDER the
// GIL: a std MutexGuard is !Send and cannot be held across allow_threads, and Phase A index ops
// are fast enough that this is fine.
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
