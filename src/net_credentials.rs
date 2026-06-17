//! HTTPS credential wiring (filled in by a later task). For now: a placeholder advertisement reader
//! so git:// ls_remote compiles; https is wired later.

use pyo3::prelude::*;

use crate::error::network_err;

// AIDEV-NOTE: PLACEHOLDER (replaced when the http client + credential provider land). Until then an
// http(s) URL is a clear NetworkError.
#[allow(clippy::type_complexity)]
pub(crate) fn http_advertisement(
    _py: Python<'_>,
    url: &str,
    _username: Option<String>,
    _password: Option<String>,
    _use_credential_helpers: bool,
) -> PyResult<(Vec<(String, grit_lib::objects::ObjectId)>, Option<String>)> {
    Err(network_err(&format!(
        "https transport not yet available for {url:?} (implemented in a later task)"
    )))
}
