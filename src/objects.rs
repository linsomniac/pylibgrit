//! Python wrappers over grit-lib object-model primitives (`ObjectId`).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use pyo3::basic::CompareOp;
use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
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

// AIDEV-NOTE: `inner()` is now used by the odb read/exists bindings (task 2.6).
// `from_inner` is still consumed by later tasks (2.7: parsed object views, refs) that
// produce an ObjectId from a grit-lib oid, hence the `#[allow(dead_code)]`. Remove the
// allow once from_inner lands a caller.
#[allow(dead_code)]
impl ObjectId {
    pub fn from_inner(inner: grit_lib::objects::ObjectId) -> Self {
        Self { inner }
    }

    pub fn inner(&self) -> grit_lib::objects::ObjectId {
        self.inner
    }
}

// AIDEV-NOTE: ObjectKind is a Python enum.IntEnum defined in python/pygrit/__init__.py.
// Native PyO3 enums lack .name and type-iteration, so kind getters return the IntEnum
// member instead. We cache the class once and construct members by integer value.
// The discriminants here MUST match the IntEnum values in __init__.py (asserted by a test).
static OBJECT_KIND_CLS: GILOnceCell<Py<PyAny>> = GILOnceCell::new();

fn object_kind_discriminant(k: grit_lib::objects::ObjectKind) -> i32 {
    match k {
        grit_lib::objects::ObjectKind::Commit => 0,
        grit_lib::objects::ObjectKind::Tree => 1,
        grit_lib::objects::ObjectKind::Blob => 2,
        grit_lib::objects::ObjectKind::Tag => 3,
    }
}

/// Convert a grit-lib object kind into the public `pygrit.ObjectKind` IntEnum member.
pub fn kind_to_py(py: Python<'_>, k: grit_lib::objects::ObjectKind) -> PyResult<Py<PyAny>> {
    let cls = OBJECT_KIND_CLS.get_or_try_init(py, || -> PyResult<Py<PyAny>> {
        Ok(py.import("pygrit")?.getattr("ObjectKind")?.unbind())
    })?;
    let member = cls.bind(py).call1((object_kind_discriminant(k),))?;
    Ok(member.unbind())
}

// AIDEV-NOTE: `Object` is the value `Odb::read` returns, surfaced to Python. It is
// `frozen` (immutable). `kind` is stored as the already-constructed pygrit.ObjectKind
// IntEnum member (built once at read time via kind_to_py) so the getter can hand back
// the singleton (identity-comparable: `obj.kind is pygrit.ObjectKind.BLOB`). `data` is
// an `Arc<[u8]>` so the payload can later be shared with typed views without copying.
#[pyclass(frozen, module = "pygrit._pygrit")]
pub struct Object {
    id: ObjectId,
    kind: Py<PyAny>,
    data: Arc<[u8]>,
}

#[pymethods]
impl Object {
    #[getter]
    fn id(&self) -> ObjectId {
        self.id.clone()
    }

    #[getter]
    fn kind(&self, py: Python<'_>) -> Py<PyAny> {
        self.kind.clone_ref(py)
    }

    #[getter]
    fn data<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.data)
    }
}

impl Object {
    pub fn new(id: ObjectId, kind: Py<PyAny>, data: Arc<[u8]>) -> Self {
        Self { id, kind, data }
    }
}
