//! URL-scheme dispatch for the read-path network surface: classify a remote URL and connect git://.

use grit_lib::transport::{ConnectOptions, Connection, GitDaemonTransport, Service, Transport};
use pyo3::prelude::*;

use crate::error::network_err;

// AIDEV-NOTE: Supported read-path schemes. ssh, file://, and scp-like `git@host:path` are out of
// scope (spec §1) and are reported as a clear NetworkError rather than a deep transport failure.
pub(crate) enum Scheme {
    Git,
    Http,
}

pub(crate) fn classify(url: &str) -> PyResult<Scheme> {
    if url.starts_with("git://") {
        Ok(Scheme::Git)
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Ok(Scheme::Http)
    } else {
        Err(network_err(&format!(
            "unsupported transport for URL {url:?}; supported schemes: git://, http://, https://"
        )))
    }
}

// AIDEV-NOTE: Connect a git:// service. `protocol_version` is forced to 1 for ls_remote (so the
// server sends a v0/v1 ref advertisement we can read off the Connection); fetch passes 0 (let grit
// pick). The returned `Box<dyn Connection>` is `!Send`, so callers MUST construct + consume it inside
// one `allow_threads` closure (never cross the boundary with it).
pub(crate) fn git_connect(
    url: &str,
    protocol_version: u8,
) -> Result<Box<dyn Connection>, grit_lib::error::Error> {
    let opts = ConnectOptions {
        protocol_version,
        server_options: Vec::new(),
    };
    GitDaemonTransport::new().connect(url, Service::UploadPack, &opts)
}
