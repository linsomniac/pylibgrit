//! HTTPS credential wiring: explicit/userinfo creds chained to git's credential helpers, plus the
//! UreqHttpClient builder and the http(s) ref-advertisement reader for ls_remote.

use std::path::Path;

use grit_lib::config::ConfigSet;
use grit_lib::credentials::{Credential, CredentialProvider, HelperCredentialProvider};
use grit_lib::transport::http::ureq_client::UreqHttpClient;
use grit_lib::transport::{ConnectOptions, Service, Transport};
use pyo3::prelude::*;

use crate::error::net_map_err;

// AIDEV-NOTE: The ref advertisement read off a Connection: the (name, oid) pairs plus the optional
// HEAD symref target. Aliased to keep clippy::type_complexity quiet on the http_advertisement
// signature + closure return type.
type Advertisement = (Vec<(String, grit_lib::objects::ObjectId)>, Option<String>);

// AIDEV-NOTE: A CredentialProvider that returns fixed username/password (from explicit kwargs or URL
// userinfo). `fill` supplies only the fields that are `None` (an explicit empty password from
// userinfo, e.g. `http://user:@host`, is treated as present and is NOT a helper trigger); if a field
// is still `None` after that and a helper is present, it delegates to the helper (so a user's
// configured `credential.helper` fills the rest). approve/reject delegate to the helper (a successful
// login may be stored), else no-op. We build the Credential explicitly (not via Clone) so this does
// not depend on `Credential: Clone`.
pub(crate) struct StaticCredentialProvider {
    username: Option<String>,
    password: Option<String>,
    helper: Option<HelperCredentialProvider>,
}

impl StaticCredentialProvider {
    pub(crate) fn new(
        username: Option<String>,
        password: Option<String>,
        helper: Option<HelperCredentialProvider>,
    ) -> Self {
        Self {
            username,
            password,
            helper,
        }
    }
}

impl CredentialProvider for StaticCredentialProvider {
    fn fill(&self, input: &Credential) -> grit_lib::error::Result<Credential> {
        let cred = Credential {
            protocol: input.protocol.clone(),
            host: input.host.clone(),
            path: input.path.clone(),
            username: input.username.clone().or_else(|| self.username.clone()),
            password: input.password.clone().or_else(|| self.password.clone()),
            url: input.url.clone(),
            extra: input.extra.clone(),
        };
        if cred.username.is_none() || cred.password.is_none() {
            if let Some(helper) = self.helper.as_ref() {
                return helper.fill(&cred);
            }
        }
        Ok(cred)
    }
    fn approve(&self, cred: &Credential) -> grit_lib::error::Result<()> {
        match &self.helper {
            Some(h) => h.approve(cred),
            None => Ok(()),
        }
    }
    fn reject(&self, cred: &Credential) -> grit_lib::error::Result<()> {
        match &self.helper {
            Some(h) => h.reject(cred),
            None => Ok(()),
        }
    }
}

// AIDEV-NOTE: Build a UreqHttpClient configured from the repo's cascaded git config (proxy, cookies,
// extra headers via from_config), with our credential provider attached. `git_dir = None` loads
// global/system config only (ls_remote without a repo). Helpers are wired only when requested. The
// config load (filesystem) runs under allow_threads; `from_config` only borrows it, so the same
// `ConfigSet` is then moved into the helper — one load, not two.
pub(crate) fn build_http_client(
    py: Python<'_>,
    git_dir: Option<&Path>,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
) -> PyResult<UreqHttpClient> {
    let (client, helper) = py
        .allow_threads(
            || -> Result<(UreqHttpClient, Option<HelperCredentialProvider>), grit_lib::error::Error> {
                let config = ConfigSet::load(git_dir, true)?;
                let client = UreqHttpClient::from_config(&config)?;
                let helper = use_credential_helpers.then(|| HelperCredentialProvider::new(config));
                Ok((client, helper))
            },
        )
        .map_err(net_map_err)?;
    let provider = StaticCredentialProvider::new(username, password, helper);
    Ok(client.with_credential_provider(Box::new(provider)))
}

// AIDEV-NOTE: Read the http(s) v0/v1 ref advertisement for ls_remote. Builds the client (with creds),
// connects via SmartHttpTransport forcing protocol v1, and copies the advertised refs + HEAD symref
// out before the `!Send` connection is dropped inside the allow_threads closure. Userinfo is split
// off the URL here too (ureq ignores URL userinfo); explicit kwargs win over userinfo.
pub(crate) fn http_advertisement(
    py: Python<'_>,
    url: &str,
    username: Option<String>,
    password: Option<String>,
    use_credential_helpers: bool,
) -> PyResult<Advertisement> {
    let (clean_url, userinfo) = crate::net_transport::split_userinfo(url);
    let user = username.or_else(|| userinfo.as_ref().map(|(u, _)| u.clone()));
    let pass = password.or_else(|| userinfo.as_ref().and_then(|(_, p)| p.clone()));
    let client = build_http_client(py, None, user, pass, use_credential_helpers)?;
    py.allow_threads(|| -> Result<Advertisement, grit_lib::error::Error> {
        let transport = grit_lib::transport::http::SmartHttpTransport::new(client);
        let opts = ConnectOptions {
            protocol_version: 1,
            server_options: Vec::new(),
        };
        let conn = transport.connect(&clean_url, Service::UploadPack, &opts)?;
        Ok((
            conn.advertised_refs().to_vec(),
            conn.head_symref().map(str::to_owned),
        ))
    })
    .map_err(net_map_err)
}
