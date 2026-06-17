//! Read-path network porcelain: ls_remote / fetch / clone, plus the value-object pyclasses.

use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::error::net_map_err;
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
