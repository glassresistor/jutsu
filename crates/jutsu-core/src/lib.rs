//! Rust core for jutsu (a port of jinja2).  Currently exposes the lexer; this
//! will grow into the full template engine core.

pub mod lexer;
pub mod unicode_tables;

use pyo3::prelude::*;
use pyo3::types::PyList;
use pyo3::IntoPyObjectExt;

use lexer::{Config, LexError};

/// Tokenize a template source with the rust lexer.
///
/// Returns a list of `(lineno, token_type, value)` tuples.  When lexing fails
/// the result is instead a single-element list `[0, "__error__", message,
/// lineno, kind]` where `kind` is `"fail"` (jinja's `Failure`, which puts the
/// filename into the exception's `name` slot) or `"syntax"`.  The python layer
/// converts that back into a real `TemplateSyntaxError`; this avoids needing
/// to construct python exceptions inside rust.
#[pyfunction]
#[pyo3(signature = (
    source,
    block_start,
    block_end,
    variable_start,
    variable_end,
    comment_start,
    comment_end,
    line_statement_prefix = None,
    line_comment_prefix = None,
    trim_blocks = false,
    lstrip_blocks = false,
    keep_trailing_newline = false,
    state = None
))]
fn tokenize(
    py: Python<'_>,
    source: &str,
    block_start: String,
    block_end: String,
    variable_start: String,
    variable_end: String,
    comment_start: String,
    comment_end: String,
    line_statement_prefix: Option<String>,
    line_comment_prefix: Option<String>,
    trim_blocks: bool,
    lstrip_blocks: bool,
    keep_trailing_newline: bool,
    state: Option<String>,
) -> PyResult<Py<PyList>> {
    let cfg = Config {
        block_start,
        block_end,
        variable_start,
        variable_end,
        comment_start,
        comment_end,
        line_statement_prefix,
        line_comment_prefix,
        trim_blocks,
        lstrip_blocks,
        keep_trailing_newline,
    };
    let list = PyList::empty(py);
    match lexer::tokenize(&cfg, source, state.as_deref()) {
        Ok(toks) => {
            for (lineno, ttype, value) in toks {
                let tup = (lineno, ttype, value).into_bound_py_any(py)?;
                list.append(&tup)?;
            }
        }
        Err(LexError::Fail { message, lineno }) => {
            let tup = (0u32, "__error__", message, lineno, "fail")
                .into_bound_py_any(py)?;
            list.append(&tup)?;
        }
        Err(LexError::Syntax { message, lineno }) => {
            let tup = (0u32, "__error__", message, lineno, "syntax")
                .into_bound_py_any(py)?;
            list.append(&tup)?;
        }
    }
    Ok(list.into())
}

#[pyfunction]
fn ping() -> PyResult<String> {
    Ok("pong".to_string())
}

#[pymodule]
fn _jutsu(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ping, m)?)?;
    m.add_function(wrap_pyfunction!(tokenize, m)?)?;
    Ok(())
}
