//! Python wrapper over `grit_lib::repo::Repository`.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::map_err;

// AIDEV-NOTE: We hold an `Arc<grit_lib::repo::Repository>` so the `.odb` accessor can
// hand out an `Odb` that clones the Arc and outlives this Python `Repository` handle
// (design §6: a child Odb keeps the repo alive). grit-lib exposes git_dir/work_tree/odb
// as PUBLIC FIELDS (no getter methods); is_bare() is the only method here.
#[pyclass(module = "pygrit._pygrit")]
pub struct Repository {
    pub(crate) inner: Arc<grit_lib::repo::Repository>,
}

#[pymethods]
impl Repository {
    // AIDEV-NOTE: discover/open release the GIL via allow_threads. grit-lib's
    // Repository and Error are Send (Odb is Arc<Mutex<..>>/PathBuf; Error wraps
    // io::Error + String), so the closure's `Result<Repository, Error>` is Send and
    // this compiles. These are not hot paths, but releasing the GIL keeps other
    // Python threads live during the filesystem walk.
    #[staticmethod]
    fn discover(py: Python<'_>, path: PathBuf) -> PyResult<Self> {
        let repo = py
            .allow_threads(|| grit_lib::repo::Repository::discover(Some(&path)))
            .map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(repo),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (git_dir, work_tree=None))]
    fn open(py: Python<'_>, git_dir: PathBuf, work_tree: Option<PathBuf>) -> PyResult<Self> {
        let repo = py
            .allow_threads(|| grit_lib::repo::Repository::open(&git_dir, work_tree.as_deref()))
            .map_err(map_err)?;
        Ok(Self {
            inner: Arc::new(repo),
        })
    }

    // AIDEV-NOTE: Paths are returned as `bytes` (not str) to preserve non-UTF-8
    // filesystem path fidelity (design §5 byte policy). `as_encoded_bytes()` is the
    // round-trippable OS-native byte form (compare with os.fsencode() on the Python
    // side).
    #[getter]
    fn git_dir<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.git_dir.as_os_str().as_encoded_bytes())
    }

    #[getter]
    fn work_tree<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.inner
            .work_tree
            .as_ref()
            .map(|p| PyBytes::new(py, p.as_os_str().as_encoded_bytes()))
    }

    #[getter]
    fn is_bare(&self) -> bool {
        self.inner.is_bare()
    }

    #[getter]
    fn odb(&self) -> crate::odb::Odb {
        crate::odb::Odb {
            repo: Arc::clone(&self.inner),
        }
    }

    // AIDEV-NOTE: Read any object then `parse_commit` over its bytes. A non-commit oid
    // parses-fail → InvalidObjectError (acceptable: the caller asked for a commit). The
    // odb read releases the GIL; parse_commit runs under the GIL (it touches Python only
    // when building Signatures). `oid.inner()` is an owned Copy, so it moves into the
    // closure with no lifetime tie to `oid`.
    fn commit(
        &self,
        py: Python<'_>,
        oid: &crate::objects::ObjectId,
    ) -> PyResult<crate::objects::Commit> {
        let want = oid.inner();
        let data = py
            .allow_threads(|| self.inner.odb.read(&want))
            .map_err(map_err)?
            .data;
        crate::objects::Commit::from_bytes(py, &data)
    }

    // AIDEV-NOTE: Read any object then `parse_tree` over its bytes. A non-tree oid
    // parses-fail → InvalidObjectError (acceptable: the caller asked for a tree). Same
    // GIL/lifetime pattern as `commit`. The returned `Tree` OWNS its entries (Arc), so it
    // outlives this Repository handle.
    fn tree(
        &self,
        py: Python<'_>,
        oid: &crate::objects::ObjectId,
    ) -> PyResult<crate::objects::Tree> {
        let want = oid.inner();
        let data = py
            .allow_threads(|| self.inner.odb.read(&want))
            .map_err(map_err)?
            .data;
        crate::objects::Tree::from_bytes(&data)
    }

    // AIDEV-NOTE: Unlike commit/tree, a blob has no parser — its payload IS the body. But
    // the caller asked specifically for a blob, so we VERIFY the read object's kind is Blob
    // and raise InvalidObjectError otherwise (rather than silently returning a tree/commit's
    // bytes). `into_boxed_slice()` moves the payload into the shared `Arc<[u8]>`.
    fn blob(
        &self,
        py: Python<'_>,
        oid: &crate::objects::ObjectId,
    ) -> PyResult<crate::objects::Blob> {
        let want = oid.inner();
        let obj = py
            .allow_threads(|| self.inner.odb.read(&want))
            .map_err(map_err)?;
        let grit_lib::objects::Object { kind, data } = obj;
        if kind != grit_lib::objects::ObjectKind::Blob {
            return Err(crate::error::InvalidObjectError::new_err(format!(
                "object {} is a {}, not a blob",
                want.to_hex(),
                kind
            )));
        }
        Ok(crate::objects::Blob::new(Arc::from(
            data.into_boxed_slice(),
        )))
    }

    // AIDEV-NOTE: Read any object then `parse_tag` over its bytes. A non-tag (or non-UTF-8)
    // object parses-fail → InvalidObjectError. Same GIL/lifetime pattern as `commit`.
    fn tag(&self, py: Python<'_>, oid: &crate::objects::ObjectId) -> PyResult<crate::objects::Tag> {
        let want = oid.inner();
        let data = py
            .allow_threads(|| self.inner.odb.read(&want))
            .map_err(map_err)?
            .data;
        crate::objects::Tag::from_bytes(py, &data)
    }

    // AIDEV-NOTE: We pass prefix="refs/" (NOT ""): in grit-lib 0.4.1, `list_refs(git_dir, "")`
    // walks `git_dir` itself and so INCLUDES top-level pseudorefs like HEAD (the spec's claim
    // that "" excludes HEAD is wrong for this version — verified against the source:
    // normalize_list_refs_prefix("") -> "" -> base == git_dir). Using "refs/" restricts the
    // walk to `refs/`, excluding HEAD/ORIG_HEAD/etc. and matching `git for-each-ref` exactly.
    // Use `head()` for HEAD. The returned ReferenceIter OWNS its data (Arc<[ReferenceData]> +
    // Arc<Repository>), so it outlives this Repository handle.
    fn references(&self, py: Python<'_>) -> PyResult<crate::refs::ReferenceIter> {
        let git_dir = self.inner.git_dir.clone();
        let refs = py
            .allow_threads(|| grit_lib::refs::list_refs(&git_dir, "refs/"))
            .map_err(map_err)?;
        let entries: Vec<crate::refs::ReferenceData> = refs
            .into_iter()
            .map(|(name, oid)| crate::refs::ReferenceData::direct(name.into_bytes(), oid))
            .collect();
        Ok(crate::refs::ReferenceIter::new(
            Arc::clone(&self.inner),
            entries,
        ))
    }
}
