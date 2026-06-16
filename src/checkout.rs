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
#[allow(clippy::type_complexity)]
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
