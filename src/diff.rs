//! Python wrappers over grit-lib's tree-to-tree diff (`diff_trees`).

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::objects::ObjectId;

// AIDEV-NOTE: Owning-iterator design (design §6), mirroring Tree/TreeIter. grit's
// `diff_trees` returns an OWNED Vec<DiffEntry>, which we copy into Arc<[DiffEntryData]>.
// A `Diff` holds that Arc; its `__iter__` clones the Arc into a `DiffIter`, so the
// iterator owns its own reference to the entry data and stays valid after the parent
// `Diff` (and the `Repository`/`Odb` it came from) is dropped. Each yielded `DiffEntry`
// clones one `DiffEntryData`, so it too is self-contained — no borrows back into grit-lib.
//
// AIDEV-NOTE: NON-UTF-8 PATH FIDELITY LIMITATION. Unlike tree-ENTRY names (TreeEntry.name,
// which grit gives us as raw Vec<u8>), grit-lib 0.4.1's DiffEntry stores paths as
// `Option<String>` (UTF-8). grit builds these via `String::from_utf8_lossy` on the tree
// entry names (see diff.rs::diff_tree_entries_opts), so a byte-exact non-UTF-8 path is NOT
// preserved here — lossy decoding has already replaced invalid bytes with U+FFFD before we
// see them. We surface `String::into_bytes()` of grit's (already-decoded) path. This is a
// grit-lib limitation we cannot work around at the binding layer.
#[derive(Clone)]
struct DiffEntryData {
    status: char,              // from DiffStatus::letter()
    old_path: Option<Vec<u8>>, // String.into_bytes(); None if absent (Added)
    new_path: Option<Vec<u8>>, // None if absent (Deleted)
    old_oid: grit_lib::objects::ObjectId,
    new_oid: grit_lib::objects::ObjectId,
}

/// A single diff entry: one changed path with a raw status letter and old/new ids.
#[pyclass(frozen, module = "pygrit._pygrit")]
pub struct DiffEntry {
    data: DiffEntryData,
}

#[pymethods]
impl DiffEntry {
    /// The single-char raw status letter: `A`/`D`/`M`/`R`/`C`/`T`/`U`.
    #[getter]
    fn status(&self) -> String {
        self.data.status.to_string()
    }

    /// The path on the old side as raw bytes, or `None` when absent (e.g. for an add).
    #[getter]
    fn old_path<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.data.old_path.as_ref().map(|p| PyBytes::new(py, p))
    }

    /// The path on the new side as raw bytes, or `None` when absent (e.g. for a delete).
    #[getter]
    fn new_path<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.data.new_path.as_ref().map(|p| PyBytes::new(py, p))
    }

    /// The old-side object id (the zero oid for an added path).
    #[getter]
    fn old_id(&self) -> ObjectId {
        ObjectId::from_inner(self.data.old_oid)
    }

    /// The new-side object id (the zero oid for a deleted path).
    #[getter]
    fn new_id(&self) -> ObjectId {
        ObjectId::from_inner(self.data.new_oid)
    }
}

// AIDEV-NOTE: DIFFSTAT COMPUTATION. DiffStats is computed EAGERLY at `diff()` time (in
// src/repository.rs::compute_diff_stats) by re-reading each changed entry's old/new blobs and
// counting line changes. We do NOT use grit-lib's `diffstat` module: that module only LAYS
// OUT a `--stat` block (column widths, name truncation, bar scaling) from PRE-COMPUTED
// per-file insertion/deletion counts (`FileStatInput`); it does not derive counts from a tree
// diff. So the binding layer does the numstat-style line counting itself (via
// grit_lib::diff::count_changes + merge_file::is_binary). `frozen` (immutable).
//
// AIDEV-NOTE: --numstat PARITY LIMITATION. These counts match `git --numstat` for normal
// `\n`-terminated text, but diverge for files containing a bare `\r` (CR not in a CRLF) as
// content: `count_changes` (via `similar`) treats `\r` as a line break while git splits on
// `\n` only. See compute_diff_stats in src/repository.rs and the xfail in tests/test_diff.py.
#[derive(Clone)]
struct DiffStatsData {
    files_changed: usize,
    insertions: usize,
    deletions: usize,
}

/// Summary line counts for a `Diff`: files changed, insertions, deletions.
#[pyclass(frozen, module = "pygrit._pygrit")]
pub struct DiffStats {
    data: DiffStatsData,
}

#[pymethods]
impl DiffStats {
    /// Number of files changed (every diff entry, matching `git --numstat` row count).
    #[getter]
    fn files_changed(&self) -> usize {
        self.data.files_changed
    }

    /// Total inserted lines across all text files (binary files contribute 0).
    #[getter]
    fn insertions(&self) -> usize {
        self.data.insertions
    }

    /// Total deleted lines across all text files (binary files contribute 0).
    #[getter]
    fn deletions(&self) -> usize {
        self.data.deletions
    }

    fn __repr__(&self) -> String {
        format!(
            "DiffStats(files_changed={}, insertions={}, deletions={})",
            self.data.files_changed, self.data.insertions, self.data.deletions
        )
    }
}

impl DiffStats {
    pub fn new(files_changed: usize, insertions: usize, deletions: usize) -> Self {
        Self {
            data: DiffStatsData {
                files_changed,
                insertions,
                deletions,
            },
        }
    }
}

/// A parsed tree diff: an iterable, len-able collection of `DiffEntry` plus `.stats`.
#[pyclass(module = "pygrit._pygrit")]
pub struct Diff {
    entries: Arc<[DiffEntryData]>,
    stats: DiffStatsData,
}

#[pymethods]
impl Diff {
    fn __len__(&self) -> usize {
        self.entries.len()
    }

    fn __iter__(slf: PyRef<'_, Self>) -> DiffIter {
        // Clone the Arc so the iterator owns its own reference -> outlives this Diff.
        DiffIter {
            entries: Arc::clone(&slf.entries),
            idx: 0,
        }
    }

    /// The diffstat summary (`DiffStats`) for this diff.
    #[getter]
    fn stats(&self) -> DiffStats {
        DiffStats {
            data: self.stats.clone(),
        }
    }
}

impl Diff {
    // AIDEV-NOTE: Map grit's owned Vec<DiffEntry> into our Arc<[DiffEntryData]>. status via
    // `DiffStatus::letter()`; paths via `Option<String>` -> `Option<Vec<u8>>` (into_bytes).
    // The stats are computed separately (in repository.rs, which has the odb) and passed in.
    pub fn from_entries(entries: Vec<grit_lib::diff::DiffEntry>, stats: DiffStats) -> Self {
        let v: Vec<DiffEntryData> = entries
            .into_iter()
            .map(|e| DiffEntryData {
                status: e.status.letter(),
                old_path: e.old_path.map(String::into_bytes),
                new_path: e.new_path.map(String::into_bytes),
                old_oid: e.old_oid,
                new_oid: e.new_oid,
            })
            .collect();
        Self {
            entries: Arc::from(v),
            stats: stats.data,
        }
    }
}

/// Iterator over a `Diff`'s entries; owns its own `Arc` so it outlives the `Diff`.
#[pyclass(module = "pygrit._pygrit")]
pub struct DiffIter {
    entries: Arc<[DiffEntryData]>,
    idx: usize,
}

#[pymethods]
impl DiffIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self) -> Option<DiffEntry> {
        let e = self.entries.get(self.idx)?.clone();
        self.idx += 1;
        Some(DiffEntry { data: e })
    }
}
