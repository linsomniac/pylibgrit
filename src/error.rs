//! Exception hierarchy and error mapping for pylibgrit.
//!
//! Maps `grit_lib::error::Error` onto a mutually-exclusive Python exception
//! hierarchy (design §7): a base `GritError` with three direct, mutually
//! exclusive subclasses (`RepositoryError`, `ObjectNotFoundError`,
//! `InvalidObjectError`). `GritError` is the always-reachable fallback.

use pyo3::prelude::*;
use pyo3::{
    create_exception,
    exceptions::{PyException, PyOSError, PyValueError},
};

create_exception!(
    _pylibgrit,
    GritError,
    PyException,
    "Base class for all pylibgrit errors."
);
create_exception!(
    _pylibgrit,
    RepositoryError,
    GritError,
    "Repository discover/open/config failure."
);
create_exception!(
    _pylibgrit,
    ObjectNotFoundError,
    GritError,
    "A requested object or ref id does not exist."
);
create_exception!(
    _pylibgrit,
    InvalidObjectError,
    GritError,
    "Malformed id or corrupt/undecodable object."
);
create_exception!(
    _pylibgrit,
    RefMismatchError,
    GritError,
    "A ref's current value did not match the expected value (compare-and-swap/create-only)."
);
create_exception!(
    _pylibgrit,
    NetworkError,
    GritError,
    "A transport, protocol, or transfer failure while talking to a remote."
);
create_exception!(
    _pylibgrit,
    AuthenticationError,
    GritError,
    "The remote rejected the supplied (or absent) credentials."
);

// AIDEV-NOTE: We CANNOT write `impl From<grit_lib::error::Error> for PyErr` — the
// orphan rule forbids it because BOTH `grit_lib::error::Error` and `PyErr` are
// foreign types to this crate. So error mapping is a free function instead.
//
// AIDEV-NOTE: `grit_lib::error::Error` is `#[non_exhaustive]`, so the match MUST
// end with a catch-all `_ =>` arm that maps to the base `GritError`; without it
// the crate would fail to compile against any future grit-lib that adds a variant.
// Each arm formats the source error (`format!("{e}")`) into the message so the
// offending path/OID/message is preserved for the caller.
pub fn map_err(e: grit_lib::error::Error) -> PyErr {
    use grit_lib::error::Error;
    let msg = format!("{e}");
    match e {
        // Repository discover/open/config/ref failures.
        Error::NotARepository(_)
        | Error::ForbiddenBareRepository(_)
        | Error::DubiousOwnership(_)
        | Error::UnsupportedRepositoryFormatVersion(_)
        | Error::UnsupportedRepositoryExtension(_)
        | Error::InvalidRef(_)
        | Error::ConfigError(_) => RepositoryError::new_err(msg),

        // A requested object/ref id does not exist.
        Error::ObjectNotFound(_) => ObjectNotFoundError::new_err(msg),

        // Malformed id / corrupt or undecodable object.
        Error::InvalidObjectId(_)
        | Error::CorruptObject(_)
        | Error::UnknownObjectType(_)
        | Error::ObjectHeaderTooLong { .. }
        | Error::Zlib(_)
        | Error::LooseHashMismatch { .. } => InvalidObjectError::new_err(msg),

        // Underlying I/O — surface as OSError, preserving errno where available.
        Error::Io(io_err) => match io_err.raw_os_error() {
            Some(errno) => PyOSError::new_err((errno, format!("{io_err}"))),
            None => PyOSError::new_err(format!("{io_err}")),
        },

        // Bad path argument shape.
        Error::PathError(_) => PyValueError::new_err(msg),

        // Remote authentication failure (HTTP 401, helper rejected, etc.).
        Error::Auth(_) => AuthenticationError::new_err(msg),

        // Everything else (index, cache-tree, signing, push-options, generic
        // message) plus any future `#[non_exhaustive]` variant.
        _ => GritError::new_err(msg),
    }
}

// AIDEV-NOTE: Small helper for binding-layer ref errors that do not originate from a
// `grit_lib::error::Error` (e.g. a non-UTF-8 ref name we refuse to pass to grit-lib's
// `&str`-typed APIs). Maps to `RepositoryError`, the subclass for ref/repository faults.
pub fn invalid_ref(msg: &str) -> PyErr {
    RepositoryError::new_err(msg.to_owned())
}

// AIDEV-NOTE: Network-context error mapping for the fetch/clone/ls_remote paths. grit's broad
// `Error::Message` (transport/protocol failures) and transfer-time `Error::Io` (connection
// refused, reset, …) become NetworkError; every other variant — including `Error::Auth` →
// AuthenticationError — defers to `map_err`, so object/ref/repo faults keep their normal class.
// Note: `Error::Io` here becomes NetworkError (NOT the errno-preserving PyOSError that `map_err`
// produces) — the errno is intentionally dropped because transport-layer I/O errors in a network
// context are better surfaced as NetworkError than as a raw OSError with an errno.
pub fn net_map_err(e: grit_lib::error::Error) -> PyErr {
    use grit_lib::error::Error;
    match e {
        Error::Message(_) | Error::Io(_) => NetworkError::new_err(format!("{e}")),
        other => map_err(other),
    }
}

// AIDEV-NOTE: Construct a NetworkError directly from a binding-layer message (e.g. an
// unsupported URL scheme) that does not originate from a `grit_lib::error::Error`.
pub fn network_err(msg: &str) -> PyErr {
    NetworkError::new_err(msg.to_owned())
}

/// Registers the exception types on the native module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("GritError", m.py().get_type::<GritError>())?;
    m.add("RepositoryError", m.py().get_type::<RepositoryError>())?;
    m.add(
        "ObjectNotFoundError",
        m.py().get_type::<ObjectNotFoundError>(),
    )?;
    m.add(
        "InvalidObjectError",
        m.py().get_type::<InvalidObjectError>(),
    )?;
    m.add("RefMismatchError", m.py().get_type::<RefMismatchError>())?;
    m.add("NetworkError", m.py().get_type::<NetworkError>())?;
    m.add(
        "AuthenticationError",
        m.py().get_type::<AuthenticationError>(),
    )?;
    Ok(())
}
