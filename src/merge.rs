//! Three-way merge surface: `MergeResult` value-object + favor parsing.

use std::collections::BTreeMap;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};

use crate::objects::ObjectId;

// AIDEV-NOTE: Resolve a commit oid to its tree oid (for commit-level merge). We gate on the read
// object's kind FIRST (like commit()/tree() in repository.rs) so a tag/blob oid cannot be silently
// misparsed as a commit. The kind mismatch is a grit `Error::CorruptObject` (NOT a PyErr — this runs
// inside the allow_threads closure); map_err sends that to InvalidObjectError, mirroring the
// "object X is a Y, not a commit" InvalidObjectError raised by Repository::commit.
pub(crate) fn tree_of_commit(
    repo: &grit_lib::repo::Repository,
    oid: grit_lib::objects::ObjectId,
) -> Result<grit_lib::objects::ObjectId, grit_lib::error::Error> {
    let obj = repo.odb.read(&oid)?;
    if obj.kind != grit_lib::objects::ObjectKind::Commit {
        return Err(grit_lib::error::Error::CorruptObject(format!(
            "object {} is a {}, not a commit",
            oid.to_hex(),
            obj.kind
        )));
    }
    let c = grit_lib::objects::parse_commit(&obj.data)?;
    Ok(c.tree)
}

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
    fn conflicts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(py, self.conflicts.iter().map(|p| PyBytes::new(py, p)))
    }

    /// The conflict-marker blob oid for `path`, or None if that path has no marker blob.
    fn conflict_blob(&self, path: Vec<u8>) -> Option<ObjectId> {
        self.conflict_map
            .get(&path)
            .map(|o| ObjectId::from_inner(*o))
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
