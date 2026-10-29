use pyo3::prelude::*;

#[pyfunction] fn ping() -> PyResult<String> {
    Ok("pong".to_string())
}

#[pymodule]
fn _jutsu(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ping, m)?)?;
    Ok(())
}
