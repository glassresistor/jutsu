//! Rust core for jutsu (a port of jinja2).  Currently exposes the lexer; this
//! will grow into the full template engine core.

pub mod eval;
pub mod lexer;
pub mod parser;
pub mod pyrepr;
pub mod unicode_tables;

use pyo3::prelude::*;
use pyo3::types::PyList;
use pyo3::IntoPyObjectExt;

use lexer::{Config, LexError, Tok};

fn push_tokens(
    py: Python<'_>,
    list: &Bound<'_, PyList>,
    toks: Vec<Tok>,
) -> PyResult<()> {
    for (lineno, ttype, value) in toks {
        let tup = (lineno, ttype, value).into_bound_py_any(py)?;
        list.append(&tup)?;
    }
    Ok(())
}

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
        Ok(toks) => push_tokens(py, &list, toks)?,
        Err(LexError::Fail { message, lineno, partial }) => {
            push_tokens(py, &list, partial)?;
            let tup = (0u32, "__error__", message, lineno, "fail")
                .into_bound_py_any(py)?;
            list.append(&tup)?;
        }
        Err(LexError::Syntax { message, lineno, partial }) => {
            push_tokens(py, &list, partial)?;
            let tup = (0u32, "__error__", message, lineno, "syntax")
                .into_bound_py_any(py)?;
            list.append(&tup)?;
        }
    }
    Ok(list.into())
}

use parser::{Av, PToken, RtParser};
use pyo3::types::PyTuple;
use std::sync::Arc;

/// Convert python `(lineno, type, value)` tuples into the rust token form.
fn to_ptokens(py: Python<'_>, tokens: &[(u32, String, Py<PyAny>)]) -> Vec<PToken> {
    let mut ptoks = Vec::with_capacity(tokens.len());
    for (lineno, ttype, value) in tokens {
        let pv = if ttype == "integer" || ttype == "float" {
            parser::PV::Num(Arc::new(value.clone_ref(py)))
        } else {
            match value.extract::<String>(py) {
                Ok(s) => parser::PV::Str(s),
                Err(_) => parser::PV::Num(Arc::new(value.clone_ref(py))),
            }
        };
        ptoks.push(PToken { lineno: *lineno, ttype: ttype.clone(), pv });
    }
    ptoks
}

/// Encode a parse error as the tuple forms documented in ast_rebuild.py.
fn err_tuple<'py>(
    py: Python<'py>,
    e: parser::ParseErr,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(stored) = &e.lexerr {
        let items: Vec<Bound<'py, PyAny>> = vec![
            "__lexerr__".into_bound_py_any(py)?,
            stored.clone_ref(py).into_bound(py),
        ];
        return Ok(PyTuple::new(py, items)?.into_any());
    }
    let items: Vec<Bound<'py, PyAny>> = vec![
        "__perr__".into_bound_py_any(py)?,
        e.message.into_bound_py_any(py)?,
        match e.lineno {
            Some(l) => l.into_bound_py_any(py)?,
            None => Option::<String>::None.into_bound_py_any(py)?,
        },
        e.cls.as_str().into_bound_py_any(py)?,
    ];
    Ok(PyTuple::new(py, items)?.into_any())
}

/// Parse and evaluate a whole template in rust without rebuilding python
/// nodes (slice v0 of the interpreter port, see PLAN_INTERP.md).  Returns
/// `("__okrender__", text)`, `["__unsupported__", node]`, `["__efail__",
/// message]` or a parse/lexer error tuple.
#[pyfunction]
#[pyo3(signature = (tokens, lexerr = None, ctx = None))]
fn eval_template<'py>(
    py: Python<'py>,
    tokens: Vec<(u32, String, Py<PyAny>)>,
    lexerr: Option<Py<PyAny>>,
    ctx: Option<Py<PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let ptoks = to_ptokens(py, &tokens);
    let mut rp = RtParser::new(ptoks, lexerr.map(Arc::new));
    match rp.parse() {
        Ok(av) => {
            let ctx = match ctx {
                Some(c) => c,
                None => return Err(pyo3::exceptions::PyTypeError::new_err(
                    "eval_template needs a context dict",
                )),
            };
            match eval::evaluate(py, &av, ctx) {
                eval::EvalOutcome::Rendered(s) => Ok(("__okrender__", s)
                    .into_bound_py_any(py)?.into_any()),
                eval::EvalOutcome::Unsupported(n) => Ok(("__unsupported__", n)
                    .into_bound_py_any(py)?.into_any()),
                eval::EvalOutcome::Failed(m) => Ok(("__efail__", m)
                    .into_bound_py_any(py)?.into_any()),
            }
        }
        Err(e) => err_tuple(py, e),
    }
}

/// Serialize an `Av` into the tuple encoding documented in ast_rebuild.py.
fn av_to_py<'py>(py: Python<'py>, av: &Av) -> PyResult<Bound<'py, PyAny>> {
    let items: Vec<Bound<'py, PyAny>> = match av {
        Av::Node(name, lineno, args) => {
            let mut v: Vec<Bound<'py, PyAny>> = vec![
                "__node__".into_bound_py_any(py)?,
                (*name).into_bound_py_any(py)?,
                match lineno {
                    Some(l) => l.into_bound_py_any(py)?,
                    None => Option::<String>::None.into_bound_py_any(py)?,
                },
            ];
            for a in args {
                v.push(av_to_py(py, a)?);
            }
            v
        }
        Av::List(inner) => {
            let mut v: Vec<Bound<'py, PyAny>> = vec!["__list__".into_bound_py_any(py)?];
            for i in inner {
                v.push(av_to_py(py, i)?);
            }
            v
        }
        Av::Tuple(inner) => {
            let mut v: Vec<Bound<'py, PyAny>> = vec!["__tup__".into_bound_py_any(py)?];
            for i in inner {
                v.push(av_to_py(py, i)?);
            }
            v
        }
        Av::Join(parts) => {
            let mut v: Vec<Bound<'py, PyAny>> = vec!["__joinstr__".into_bound_py_any(py)?];
            for p in parts {
                v.push(av_to_py(py, p)?);
            }
            v
        }
        Av::Str(s) => {
            vec!["__val__".into_bound_py_any(py)?, s.as_str().into_bound_py_any(py)?]
        }
        Av::Bool(b) => vec!["__val__".into_bound_py_any(py)?, b.into_bound_py_any(py)?],
        Av::NoneV => vec!["__val__".into_bound_py_any(py)?, Option::<String>::None.into_bound_py_any(py)?],
        Av::Val(obj) => {
            vec!["__val__".into_bound_py_any(py)?, obj.clone_ref(py).into_bound(py)]
        }
    };
    Ok(PyTuple::new(py, items)?.into_any())
}

/// Parse a wrapped token stream into the serialized AST encoding.
///
/// `tokens` is a list of `(lineno, token_type, value)` tuples as produced by
/// `Lexer.wrap`.  Returns either the AST tuple tree or a single-element style
/// error tuple ("__perr__", message, lineno, kind) -- see ast_rebuild.py.
#[pyfunction]
#[pyo3(signature = (tokens, lexerr = None))]
fn parse<'py>(
    py: Python<'py>,
    tokens: Vec<(u32, String, Py<PyAny>)>,
    lexerr: Option<Py<PyAny>>,
) -> PyResult<Bound<'py, PyAny>> {
    let ptoks = to_ptokens(py, &tokens);
    let mut rp = RtParser::new(ptoks, lexerr.map(std::sync::Arc::new));
    match rp.parse() {
        Ok(av) => av_to_py(py, &av),
        Err(e) if e.lexerr.is_some() => {
            // Re-raise the stored python exception args verbatim.
            let stored = e.lexerr.as_ref().unwrap();
            let items: Vec<Bound<'py, PyAny>> = vec![
                "__lexerr__".into_bound_py_any(py)?,
                stored.clone_ref(py).into_bound(py),
            ];
            Ok(PyTuple::new(py, items)?.into_any())
        }
        Err(e) => {
            let items: Vec<Bound<'py, PyAny>> = vec![
                "__perr__".into_bound_py_any(py)?,
                e.message.into_bound_py_any(py)?,
                match e.lineno {
                    Some(l) => l.into_bound_py_any(py)?,
                    None => Option::<String>::None.into_bound_py_any(py)?,
                },
                e.cls.as_str().into_bound_py_any(py)?,
            ];
            Ok(PyTuple::new(py, items)?.into_any())
        }
    }
}

#[pyfunction]
fn ping() -> PyResult<String> {
    Ok("pong".to_string())
}

#[pymodule]
fn _jutsu(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(ping, m)?)?;
    m.add_function(wrap_pyfunction!(tokenize, m)?)?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(eval_template, m)?)?;
    Ok(())
}
