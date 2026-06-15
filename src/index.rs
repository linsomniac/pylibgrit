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
