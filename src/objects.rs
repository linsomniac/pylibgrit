//! Python wrappers over grit-lib object-model primitives (`ObjectId`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use pyo3::basic::CompareOp;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::map_err;

// AIDEV-NOTE: We wrap grit-lib's own `ObjectId` (which derives
// Clone/Copy/Eq/Ord/Hash and provides to_hex/as_bytes/from_hex/from_bytes/algo)
// rather than reimplementing hex parsing — grit-lib owns the canonical SHA-1/256
// width logic. `frozen` makes the Python object immutable, matching the Copy oid.
#[pyclass(frozen, module = "pygrit._pygrit")]
#[derive(Clone)]
pub struct ObjectId {
    pub(crate) inner: grit_lib::objects::ObjectId,
}

#[pymethods]
impl ObjectId {
    /// Parses an `ObjectId` from a 40- (SHA-1) or 64-char (SHA-256) hex string.
    #[staticmethod]
    fn from_hex(hex: &str) -> PyResult<Self> {
        grit_lib::objects::ObjectId::from_hex(hex)
            .map(|inner| Self { inner })
            .map_err(map_err)
    }

    /// The lowercase hex digest (40 chars for SHA-1, 64 for SHA-256).
    #[getter]
    fn hex(&self) -> String {
        self.inner.to_hex()
    }

    /// The raw digest bytes (20 for SHA-1, 32 for SHA-256).
    #[getter]
    fn raw<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.as_bytes())
    }

    /// The hash algorithm name (`"sha1"` or `"sha256"`), inferred from length.
    #[getter]
    fn hash_algorithm(&self) -> &'static str {
        self.inner.algo().name()
    }

    fn __richcmp__(&self, other: &ObjectId, op: CompareOp) -> bool {
        match op {
            CompareOp::Eq => self.inner == other.inner,
            CompareOp::Ne => self.inner != other.inner,
            _ => false,
        }
    }

    fn __hash__(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.inner.hash(&mut h);
        h.finish()
    }

    fn __repr__(&self) -> String {
        format!("ObjectId('{}')", self.inner.to_hex())
    }
}

// AIDEV-NOTE: `#[allow(dead_code)]` is intentional — these conversion helpers are
// consumed by later tasks (2.6/2.7: odb read, parsed object views, refs) that
// produce ObjectId from a grit-lib oid and pass it back to grit-lib. Remove the
// allow once those bindings land and call from_inner/inner.
#[allow(dead_code)]
impl ObjectId {
    pub fn from_inner(inner: grit_lib::objects::ObjectId) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> grit_lib::objects::ObjectId {
        self.inner
    }
}

// AIDEV-NOTE: Mirror of grit-lib's `ObjectKind { Blob, Tree, Commit, Tag }` as a
// Python enum. `eq` + `eq_int` make members comparable and give each a stable int
// discriminant; the Python-facing names are uppercased (COMMIT/TREE/BLOB/TAG).
// `#[allow(clippy::upper_case_acronyms)]` is required because those member names are
// the deliberate Python-facing identifiers (and BLOB/TAG read as acronyms to clippy);
// renaming them would change the public enum API. Variant declaration order also
// fixes the eq_int discriminants (COMMIT=0..TAG=3) that python/pygrit/__init__.py's
// IntEnum facade mirrors — keep the two in sync.
#[pyclass(eq, eq_int, module = "pygrit._pygrit")]
#[derive(Clone, PartialEq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ObjectKind {
    COMMIT,
    TREE,
    BLOB,
    TAG,
}

#[pymethods]
impl ObjectKind {
    fn __repr__(&self) -> &'static str {
        match self {
            ObjectKind::COMMIT => "ObjectKind.COMMIT",
            ObjectKind::TREE => "ObjectKind.TREE",
            ObjectKind::BLOB => "ObjectKind.BLOB",
            ObjectKind::TAG => "ObjectKind.TAG",
        }
    }
}

// AIDEV-NOTE: `#[allow(dead_code)]` is intentional — `from_grit` is consumed by the
// odb read / parsed-object-view bindings (task 2.6+) that surface a grit-lib
// ObjectKind to Python. Remove the allow once those callers land.
#[allow(dead_code)]
impl ObjectKind {
    pub fn from_grit(k: grit_lib::objects::ObjectKind) -> Self {
        match k {
            grit_lib::objects::ObjectKind::Commit => ObjectKind::COMMIT,
            grit_lib::objects::ObjectKind::Tree => ObjectKind::TREE,
            grit_lib::objects::ObjectKind::Blob => ObjectKind::BLOB,
            grit_lib::objects::ObjectKind::Tag => ObjectKind::TAG,
        }
    }
}
