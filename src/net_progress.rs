//! Bridge an optional Python `bytes` callable to grit's `fetch::Progress` (side-band-2 stream).

use pyo3::prelude::*;
use pyo3::types::PyBytes;

// AIDEV-NOTE: `grit_lib::fetch::Progress` has a single infallible method `message(&mut self, &[u8])`.
// `PyProgress` wraps an optional Python callable invoked once per side-band-2 chunk. The transfer
// runs under allow_threads (GIL released); `message` re-acquires the GIL via `Python::with_gil` for
// just the callback, so the callback never holds the GIL across the transfer. A Python exception is
// CAPTURED (grit's `message` cannot return an error / unwind through FFI) and re-raised by the caller
// via `take_error()` after the transfer returns. `Py<PyAny>` + `Option<PyErr>` are both Send, so
// `&mut PyProgress` may cross into allow_threads.
pub(crate) struct PyProgress {
    callback: Option<Py<PyAny>>,
    error: Option<PyErr>,
}

impl PyProgress {
    pub(crate) fn new(callback: Option<Py<PyAny>>) -> Self {
        Self {
            callback,
            error: None,
        }
    }
    pub(crate) fn take_error(&mut self) -> Option<PyErr> {
        self.error.take()
    }
}

impl grit_lib::fetch::Progress for PyProgress {
    fn message(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        let Some(cb) = &self.callback else {
            return;
        };
        Python::with_gil(|py| {
            let arg = PyBytes::new(py, bytes);
            if let Err(e) = cb.call1(py, (arg,)) {
                self.error = Some(e);
            }
        });
    }
}
