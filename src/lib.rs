use pyo3::prelude::*;

/// Returns the pygrit version string. Smoke-test entry point for the spike.
#[pyfunction]
fn _hello() -> &'static str {
    "pygrit"
}

#[pymodule]
fn _pygrit(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(_hello, m)?)?;
    Ok(())
}
