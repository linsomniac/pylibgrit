//! URL-scheme dispatch for the read-path network surface: classify a remote URL and connect git://.

use grit_lib::transport::{
    is_ssh_url, ConnectOptions, Connection, GitDaemonTransport, Service, SshTransport, Transport,
};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::error::network_err;

// AIDEV-NOTE: Supported read-path schemes. file:// and ext:: are out of scope and are reported as a
// clear NetworkError rather than a deep transport failure.
pub(crate) enum Scheme {
    Git,
    Http,
    Ssh,
}

pub(crate) fn classify(url: &str) -> PyResult<Scheme> {
    if url.starts_with("git://") {
        Ok(Scheme::Git)
    } else if url.starts_with("https://") || url.starts_with("http://") {
        Ok(Scheme::Http)
    } else if is_ssh_url(url) {
        Ok(Scheme::Ssh)
    } else {
        Err(network_err(&format!(
            "unsupported transport for URL {url:?}; supported schemes: git://, http://, https://, \
             ssh:// (and scp-style host:path)"
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

// AIDEV-NOTE: Split optional `user[:pass]@` userinfo out of an http(s) authority. ureq's client does
// NOT honor URL userinfo, so we extract it for the credential provider and return the URL with
// userinfo removed for the actual request. Only the authority right after `scheme://` is examined (a
// later '@' in the path is left alone). Userinfo is used LITERALLY — not percent-decoded; callers
// with reserved characters in a token should pass `password=` instead. An EMPTY `user[:pass]@`
// (e.g. `http://@host/x`) is treated as NO userinfo (we do not feed an empty username to the
// credential provider). Returns (clean_url, Some((user, Option<pass>))) when userinfo is present.
pub(crate) fn split_userinfo(url: &str) -> (String, Option<(String, Option<String>)>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), None);
    };
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(auth_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return (url.to_owned(), None);
    };
    if userinfo.is_empty() {
        return (url.to_owned(), None);
    }
    let creds = match userinfo.split_once(':') {
        Some((u, p)) => (u.to_owned(), Some(p.to_owned())),
        None => (userinfo.to_owned(), None),
    };
    (format!("{scheme}://{host}{tail}"), Some(creds))
}

// AIDEV-NOTE: Resolve the effective HTTP(S) credentials for a request. Splits any `user[:pass]@`
// userinfo off `url` (ureq ignores URL userinfo) and merges it with the explicit kwargs: explicit
// `username`/`password` win, else fall back to the URL userinfo. Returns the userinfo-stripped URL
// plus the resolved (user, pass). Every http(s) entry point (ls_remote, fetch/clone, push) goes
// through here so the precedence rule lives in exactly one place.
pub(crate) fn resolve_url_credentials(
    url: &str,
    username: Option<String>,
    password: Option<String>,
) -> (String, Option<String>, Option<String>) {
    let (clean_url, userinfo) = split_userinfo(url);
    let user = username.or_else(|| userinfo.as_ref().map(|(u, _)| u.clone()));
    let pass = password.or_else(|| userinfo.as_ref().and_then(|(_, p)| p.clone()));
    (clean_url, user, pass)
}

// AIDEV-NOTE: Connect a git:// service for PUSH (git-receive-pack). Forces protocol v0/v1
// (`protocol_version: 0`) because grit's push rejects v2. Like `git_connect`, the returned
// `Box<dyn Connection>` is `!Send` — construct + consume it inside one `allow_threads` closure.
pub(crate) fn git_connect_receive(
    url: &str,
) -> Result<Box<dyn Connection>, grit_lib::error::Error> {
    let opts = ConnectOptions {
        protocol_version: 0,
        server_options: Vec::new(),
    };
    GitDaemonTransport::new().connect(url, Service::ReceivePack, &opts)
}

// AIDEV-NOTE: Build the ssh transport. `Some(cmd)` pins a shell command line (run via `sh -c`, like
// GIT_SSH_COMMAND); `None` is Auto — grit resolves $GIT_SSH_COMMAND, then $GIT_SSH, then `ssh`.
fn build_ssh_transport(ssh_command: Option<&str>) -> SshTransport {
    match ssh_command {
        Some(cmd) => SshTransport::with_shell_command(cmd),
        None => SshTransport::new(),
    }
}

// AIDEV-NOTE: Connect an ssh service (git-upload-pack) for ls_remote/fetch. `protocol_version` 1 for
// ls_remote (read a v0/v1 advertisement); 0 for fetch (let the server pick). The returned
// `Box<dyn Connection>` wraps a child process — it is `!Send`, so construct + consume it inside one
// `allow_threads` closure (never cross the boundary), exactly like `git_connect`.
pub(crate) fn ssh_connect(
    url: &str,
    protocol_version: u8,
    ssh_command: Option<&str>,
) -> Result<Box<dyn Connection>, grit_lib::error::Error> {
    let opts = ConnectOptions {
        protocol_version,
        server_options: Vec::new(),
    };
    build_ssh_transport(ssh_command).connect(url, Service::UploadPack, &opts)
}

// AIDEV-NOTE: ssh auth (keys/agent/known_hosts) is the ssh subprocess's job, never pylibgrit's. The
// http-only `username`/`password` kwargs do not apply to ssh URLs; passing either with an ssh URL is
// almost certainly a mistake, so fail loud. `use_credential_helpers` (http-only) is left alone.
pub(crate) fn reject_creds_for_ssh(
    url: &str,
    username: &Option<String>,
    password: &Option<String>,
) -> PyResult<()> {
    if (username.is_some() || password.is_some()) && is_ssh_url(url) {
        return Err(PyValueError::new_err(
            "username/password are not used for ssh URLs; ssh authentication is handled by the ssh \
             program (keys/agent). Put the user in the URL, e.g. ssh://user@host/path.",
        ));
    }
    Ok(())
}
