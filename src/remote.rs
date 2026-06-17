//! Read-path network porcelain: ls_remote / fetch / clone, plus the value-object pyclasses.

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use grit_lib::transfer::{FetchOptions, FetchOutcome, TagMode, UpdateMode};

use crate::error::net_map_err;
use crate::net_progress::PyProgress;
use crate::net_transport::{classify, git_connect, Scheme};

// AIDEV-NOTE: One advertised remote ref. `name`/`symref_target` are bytes (house style: ref names
// are bytes everywhere in the binding); `oid` is an ObjectId. HEAD is synthesized from the
// connection's head_symref + the symref target's advertised oid (advertised_refs excludes HEAD).
#[pyclass(frozen, module = "pylibgrit._pylibgrit")]
pub struct RemoteRef {
    name: Vec<u8>,
    oid: grit_lib::objects::ObjectId,
    symref_target: Option<Vec<u8>>,
}

#[pymethods]
impl RemoteRef {
    #[getter]
    fn name<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.name)
    }
    #[getter]
    fn oid(&self) -> crate::objects::ObjectId {
        crate::objects::ObjectId::from_inner(self.oid)
    }
    #[getter]
    fn symref_target<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.symref_target.as_ref().map(|t| PyBytes::new(py, t))
    }
    fn __repr__(&self) -> String {
        format!(
            "RemoteRef(name={:?}, oid='{}')",
            String::from_utf8_lossy(&self.name),
            self.oid.to_hex()
        )
    }
}

// AIDEV-NOTE: Read the v0/v1 ref advertisement from a freshly-opened connection. Returns owned
// (name, oid) pairs plus the HEAD symref target — everything is cloned out so the `!Send` connection
// is dropped inside the caller's allow_threads closure. `protocol_version: 1` forces the
// advertisement (v2 advertises nothing on connect).
#[allow(clippy::type_complexity)]
fn read_advertisement(
    url: &str,
) -> Result<(Vec<(String, grit_lib::objects::ObjectId)>, Option<String>), grit_lib::error::Error> {
    let conn = git_connect(url, 1)?;
    let refs = conn.advertised_refs().to_vec();
    let head = conn.head_symref().map(str::to_owned);
    Ok((refs, head))
}

// AIDEV-NOTE: List remote refs (== `git ls-remote`), built from the connection advertisement (grit's
// `ls_remote` is local-only). `heads`/`tags` restrict to those namespaces and drop the synthesized
// HEAD row (matching `git ls-remote --heads/--tags`). Peeled `^{}` rows are not surfaced (grit's
// advertised_refs omits them) — a documented limitation.
#[pyfunction]
#[pyo3(signature = (url, *, username=None, password=None, use_credential_helpers=true, heads=false, tags=false))]
pub fn ls_remote(
    py: Python<'_>,
    url: String,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    heads: bool,
    tags: bool,
) -> PyResult<Vec<RemoteRef>> {
    let (advertised, head_target) = match classify(&url)? {
        Scheme::Git => py
            .allow_threads(|| read_advertisement(&url))
            .map_err(net_map_err)?,
        Scheme::Http => crate::net_credentials::http_advertisement(
            py,
            &url,
            username,
            password,
            use_credential_helpers,
        )?,
    };

    let mut out: Vec<RemoteRef> = Vec::new();

    // Synthesized HEAD row (only in the unfiltered default, like `git ls-remote`).
    if !heads && !tags {
        if let Some(target) = &head_target {
            if let Some((_, oid)) = advertised.iter().find(|(n, _)| n == target) {
                out.push(RemoteRef {
                    name: b"HEAD".to_vec(),
                    oid: *oid,
                    symref_target: Some(target.clone().into_bytes()),
                });
            }
        }
    }

    for (name, oid) in advertised {
        let keep = if heads {
            name.starts_with("refs/heads/")
        } else if tags {
            name.starts_with("refs/tags/")
        } else {
            true
        };
        if keep {
            out.push(RemoteRef {
                name: name.into_bytes(),
                oid,
                symref_target: None,
            });
        }
    }
    Ok(out)
}

// AIDEV-NOTE: One applied ref update from a fetch. Ref names are bytes; oids are ObjectId; `mode` is
// the lower-kebab `UpdateMode` name; `note` is grit's human-readable annotation.
#[pyclass(frozen, module = "pylibgrit._pylibgrit")]
pub struct RefUpdate {
    remote_ref: Vec<u8>,
    local_ref: Option<Vec<u8>>,
    old_oid: Option<grit_lib::objects::ObjectId>,
    new_oid: Option<grit_lib::objects::ObjectId>,
    mode: String,
    note: Option<String>,
}

#[pymethods]
impl RefUpdate {
    #[getter]
    fn remote_ref<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.remote_ref)
    }
    #[getter]
    fn local_ref<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.local_ref.as_ref().map(|r| PyBytes::new(py, r))
    }
    #[getter]
    fn old_oid(&self) -> Option<crate::objects::ObjectId> {
        self.old_oid.map(crate::objects::ObjectId::from_inner)
    }
    #[getter]
    fn new_oid(&self) -> Option<crate::objects::ObjectId> {
        self.new_oid.map(crate::objects::ObjectId::from_inner)
    }
    #[getter]
    fn mode(&self) -> &str {
        &self.mode
    }
    #[getter]
    fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

// AIDEV-NOTE: The result of a fetch: the applied ref updates + the remote's default branch (HEAD
// symref). Shallow fields are intentionally omitted (shallow deferred).
#[pyclass(frozen, module = "pylibgrit._pylibgrit")]
pub struct FetchReport {
    updates: Vec<Py<RefUpdate>>,
    default_branch: Option<Vec<u8>>,
}

#[pymethods]
impl FetchReport {
    #[getter]
    fn updates(&self, py: Python<'_>) -> Vec<Py<RefUpdate>> {
        self.updates.iter().map(|u| u.clone_ref(py)).collect()
    }
    #[getter]
    fn default_branch<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
        self.default_branch.as_ref().map(|b| PyBytes::new(py, b))
    }
}

// AIDEV-NOTE: grit's UpdateMode -> the lower-kebab string surfaced as RefUpdate.mode.
fn update_mode_str(m: UpdateMode) -> &'static str {
    match m {
        UpdateMode::New => "new",
        UpdateMode::FastForward => "fast-forward",
        UpdateMode::Forced => "forced",
        UpdateMode::UpToDate => "up-to-date",
        UpdateMode::NoChangeNeeded => "no-change-needed",
        UpdateMode::NonFastForwardRejected => "non-fast-forward-rejected",
        UpdateMode::TagUpdateRejected => "tag-update-rejected",
        UpdateMode::SourceObjectNotFound => "source-object-not-found",
        UpdateMode::Unborn => "unborn",
        UpdateMode::DeletedMissing => "deleted-missing",
    }
}

// AIDEV-NOTE: Core fetch: dispatch by scheme, return the raw FetchOutcome. BOTH
// `grit_lib::fetch::fetch_remote` (git://) and `grit_lib::transport::http::http_fetch` (https) unpack
// objects AND write the New/FastForward/Forced tracking refs (and prune) internally — verified
// against grit-lib-0.4.1 fetch.rs:1327-1360 — so the binding does NO ref application; it only maps
// the FetchOutcome to a FetchReport. The git:// connection is `!Send`, so it is constructed and
// consumed entirely inside one allow_threads closure (never crossing the boundary).
//
// Progress-callback error semantics: grit's `Progress::message` is INFALLIBLE, so a raising
// `progress` callback does NOT abort the in-flight transfer — the fetch runs to completion (and grit
// writes its refs). The binding CAPTURES the callback's exception during the transfer and, after it
// returns, surfaces it via `take_error()` and discards the report, treating the fetch as failed even
// though grit completed it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_raw(
    py: Python<'_>,
    git_dir: &std::path::Path,
    url: &str,
    opts: &FetchOptions,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    progress: Option<Py<PyAny>>,
) -> PyResult<FetchOutcome> {
    let mut prog = PyProgress::new(progress);
    let outcome = match classify(url)? {
        Scheme::Git => {
            let result = py.allow_threads(|| -> Result<FetchOutcome, grit_lib::error::Error> {
                let mut conn = git_connect(url, 0)?;
                grit_lib::fetch::fetch_remote(git_dir, &mut *conn, opts, &mut prog)
            });
            if let Some(e) = prog.take_error() {
                return Err(e);
            }
            result.map_err(net_map_err)?
        }
        Scheme::Http => {
            let (clean_url, userinfo) = crate::net_transport::split_userinfo(url);
            let client = crate::net_credentials::build_http_client(
                py,
                Some(git_dir),
                merge_user(username, &userinfo),
                merge_pass(password, &userinfo),
                use_credential_helpers,
            )?;
            let result = py.allow_threads(|| {
                grit_lib::transport::http::http_fetch(&client, git_dir, &clean_url, opts, &mut prog)
            });
            if let Some(e) = prog.take_error() {
                return Err(e);
            }
            result.map_err(net_map_err)?
        }
    };
    Ok(outcome)
}

// AIDEV-NOTE: Credentials precedence: explicit kwargs win, else fall back to URL userinfo.
fn merge_user(
    explicit: Option<String>,
    userinfo: &Option<(String, Option<String>)>,
) -> Option<String> {
    explicit.or_else(|| userinfo.as_ref().map(|(u, _)| u.clone()))
}
fn merge_pass(
    explicit: Option<String>,
    userinfo: &Option<(String, Option<String>)>,
) -> Option<String> {
    explicit.or_else(|| userinfo.as_ref().and_then(|(_, p)| p.clone()))
}

// AIDEV-NOTE: Build FetchOptions from the Python kwargs (default refspec fetches all heads into
// refs/remotes/origin/*). `tags` maps to grit's TagMode.
pub(crate) fn build_fetch_options(
    refspecs: Option<Vec<String>>,
    tags: &str,
    prune: bool,
) -> PyResult<FetchOptions> {
    let tagmode = match tags {
        "none" => TagMode::None,
        "following" => TagMode::Following,
        "all" => TagMode::All,
        other => {
            return Err(pyo3::exceptions::PyValueError::new_err(format!(
                "tags must be 'none', 'following', or 'all' (got {other:?})"
            )))
        }
    };
    let refspecs =
        refspecs.unwrap_or_else(|| vec!["+refs/heads/*:refs/remotes/origin/*".to_owned()]);
    Ok(FetchOptions {
        refspecs,
        tags: tagmode,
        prune,
        ..Default::default()
    })
}

// AIDEV-NOTE: Build a FetchReport (Python objects) from a raw FetchOutcome.
fn build_report(py: Python<'_>, outcome: FetchOutcome) -> PyResult<FetchReport> {
    let mut updates = Vec::with_capacity(outcome.updates.len());
    for u in outcome.updates {
        let ru = RefUpdate {
            remote_ref: u.remote_ref.into_bytes(),
            local_ref: u.local_ref.map(String::into_bytes),
            old_oid: u.old_oid,
            new_oid: u.new_oid,
            mode: update_mode_str(u.mode).to_owned(),
            note: u.note,
        };
        updates.push(Py::new(py, ru)?);
    }
    Ok(FetchReport {
        updates,
        default_branch: outcome.default_branch.map(String::into_bytes),
    })
}

// AIDEV-NOTE: Repository.fetch entry point (called from src/repository.rs).
#[allow(clippy::too_many_arguments)]
pub(crate) fn fetch_method(
    py: Python<'_>,
    repo: &Arc<grit_lib::repo::Repository>,
    url: String,
    refspecs: Option<Vec<String>>,
    tags: &str,
    prune: bool,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
    progress: Option<Py<PyAny>>,
) -> PyResult<FetchReport> {
    let opts = build_fetch_options(refspecs, tags, prune)?;
    let git_dir = repo.git_dir.clone();
    let outcome = fetch_raw(
        py,
        &git_dir,
        &url,
        &opts,
        username,
        password,
        use_credential_helpers,
        progress,
    )?;
    build_report(py, outcome)
}
