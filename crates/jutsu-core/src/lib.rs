//! Rust core for jutsu (a port of jinja2).  Currently exposes the lexer; this
//! will grow into the full template engine core.

// Re-export all modules for use in pure API when pyo3 is disabled
pub mod eval;
pub mod lexer;
pub mod parser;
pub mod pyrepr;
pub mod unicode_tables;

/// Pure Rust API functions that can be used without Python dependencies
#[cfg(not(feature = "pyo3"))]
pub mod pure_api {
    use super::lexer::{Config, LexError, Tok};
    use super::parser::{Av, PToken, RtParser};
    use std::sync::Arc;
    
    /// Tokenize a template source with the rust lexer.
    /// Returns a list of `(lineno, token_type, value)` tuples.
    pub fn tokenize(cfg: &Config, source: &str, state: Option<&str>) -> Result<Vec<Tok>, LexError> {
        super::lexer::tokenize(cfg, source, state)
    }
    
    /// Parse a template source into an AST
    pub fn parse(tokens: Vec<PToken>) -> Result<Av, parser::ParseErr> {
        let mut rp = RtParser::new(tokens, None);
        rp.parse()
    }
    
    /// Evaluate a parsed AST against a context (pure rust implementation)
    pub fn evaluate(av: &Av) -> Result<String, String> {
        // This would be the pure Rust evaluation - placeholder for now
        Ok("evaluated_result".to_string())
    }
}

#[cfg(feature = "pyo3")]
mod pyo3_impl {
    use pyo3::prelude::*;
    use pyo3::types::PyList;
    use pyo3::IntoPyObjectExt;

    use lexer::{Config, LexError, Tok};

    use parser::{Av, PToken, RtParser};
    use pyo3::types::PyTuple;
    use std::sync::Arc;

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
                Ok(list.into())
            }
            Err(LexError::Fail { message, lineno, partial }) => {
                for (lineno, ttype, value) in partial {
                    let tup = (lineno, ttype, value).into_bound_py_any(py)?;
                    list.append(&tup)?;
                }
                let tup = (0u32, "__error__", message, lineno, "fail")
                    .into_bound_py_any(py)?;
                list.append(&tup)?;
                Ok(list.into())
            }
            Err(LexError::Syntax { message, lineno, partial }) => {
                for (lineno, ttype, value) in partial {
                    let tup = (lineno, ttype, value).into_bound_py_any(py)?;
                    list.append(&tup)?;
                }
                let tup = (0u32, "__error__", message, lineno, "syntax")
                    .into_bound_py_any(py)?;
                list.append(&tup)?;
                Ok(list.into())
            }
        }
    }

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
        e: &parser::ParseErr,
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
            e.message.as_str().into_bound_py_any(py)?,
            match e.lineno {
                Some(l) => l.into_bound_py_any(py)?,
                None => Option::<String>::None.into_bound_py_any(py)?,
            },
            e.cls.as_str().into_bound_py_any(py)?,
        ];
        Ok(PyTuple::new(py, items)?.into_any())
    }

    /// Parse and evaluate a whole template in rust without rebuilding python
    /// nodes (slice v1 of the interpreter port, see PLAN_INTERP.md).  `ctx` is a
    /// live python `Context`, `rt` the `jutsu.runtime` module and `name` the
    /// optional template name.  Returns `["__okrender__", text]` or
    /// `["__unsupported__", node]`; runtime exceptions are raised into python
    /// directly so they propagate untouched (matching how the compiled engine
    /// re-raises them through `Environment.handle_exception`).
    #[pyfunction]
    #[pyo3(signature = (tokens, lexerr = None, ctx = None, rt = None, name = None, sources = None))]
    fn eval_template<'py>(
        py: Python<'py>,
        tokens: Vec<(u32, String, Py<PyAny>)>,
        lexerr: Option<Py<PyAny>>,
        ctx: Option<Py<PyAny>>,
        rt: Option<Py<PyAny>>,
        name: Option<String>,
        sources: Option<std::collections::HashMap<String, Option<Vec<(u32, String, Py<PyAny>)>>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let ptoks = to_ptokens(py, &tokens);
        let mut rp = RtParser::new(ptoks, lexerr.map(Arc::new));
        match rp.parse() {
            Ok(av) => render_av(py, &av, ctx, rt, name, sources),
            Err(e) => err_tuple(py, &e),
        }
    }

    /// Evaluate an already-parsed AST against a live context: the tail of
    /// `eval_template` shared with the cached entry point below.
    fn render_av<'py>(
        py: Python<'py>,
        av: &Av,
        ctx: Option<Py<PyAny>>,
        rt: Option<Py<PyAny>>,
        name: Option<String>,
        sources: Option<std::collections::HashMap<String, Option<Vec<(u32, String, Py<PyAny>)>>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let srcs: std::collections::HashMap<String, Option<Vec<PToken>>> = match sources {
            Some(map) => map
                .into_iter()
                .map(|(k, v)| (
                    k,
                    match v {
                        Some(toks) => Some(to_ptokens(py, &toks)),
                        None => None,
                    },
                ))
                .collect(),
            None => Default::default(),
        };
        let (cbox, rtbox) = match (ctx, rt) {
            (Some(c), Some(r)) => (c.bind(py).clone(), r.bind(py).clone()),
            _ => {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "eval needs a context and the runtime module",
                ))
            }
        };
        match eval::evaluate_with_sources(py, av, cbox, rtbox, name, srcs) {
            eval::EvalOutcome::Rendered(v) => {
                let items: Vec<Bound<'py, PyAny>> = vec![
                    "__okrender__".into_bound_py_any(py)?,
                    v,
                ];
                Ok(PyTuple::new(py, items)?.into_any())
            }
            eval::EvalOutcome::Unsupported(n) => {
                Ok(("__unsupported__", n).into_bound_py_any(py)?.into_any())
            }
            eval::EvalOutcome::Exc(e) => Err(e),
        }
    }

    /// What a cached compile call remembers: the parsed tree or the deterministic
    /// parse failure instead of it.
    enum AstOutcome {
        Ok(Av),
        LexErr(parser::ParseErr),
        PErr(parser::ParseErr),
    }

    /// A template parsed once for repeated evaluation (performance pass, see
    /// PLAN_INTERP.md).  The python driver caches these handles per source and
    /// lexer settings; `eval_parsed` then reproduces exactly what a fresh
    /// tokenize+parse+evaluate would have returned -- including re-raising stored
    /// lexer errors with their original python arguments -- so cached and cold
    /// paths are indistinguishable.  Evaluation never mutates the tree (the
    /// interpreter takes it by reference), which is what makes sharing safe.
    #[pyclass(unsendable)]
    pub struct ParsedAst {
        outcome: AstOutcome,
    }

    /// Parse a wrapped token stream and keep the result alive for reuse.  Unlike
    /// `parse` this never serializes the tree to python tuples; errors are stored
    /// rather than returned, so the call itself cannot fail.
    #[pyfunction]
    #[pyo3(signature = (tokens, lexerr = None))]
    fn compile_ast<'py>(
        py: Python<'py>,
        tokens: Vec<(u32, String, Py<PyAny>)>,
        lexerr: Option<Py<PyAny>>,
    ) -> ParsedAst {
        let ptoks = to_ptokens(py, &tokens);
        let mut rp = RtParser::new(ptoks, lexerr.map(Arc::new));
        match rp.parse() {
            Ok(av) => ParsedAst { outcome: AstOutcome::Ok(av) },
            Err(e) if e.lexerr.is_some() => ParsedAst { outcome: AstOutcome::LexErr(e) },
            Err(e) => ParsedAst { outcome: AstOutcome::PErr(e) },
        }
    }

    /// Evaluate a `ParsedAst` handle.  Same result tuples as `eval_template`, and
    /// the same exception objects re-raised for stored parse failures.
    #[pyfunction]
    #[pyo3(signature = (ast, ctx = None, rt = None, name = None, sources = None))]
    fn eval_parsed<'py>(
        py: Python<'py>,
        ast: &ParsedAst,
        ctx: Option<Py<PyAny>>,
        rt: Option<Py<PyAny>>,
        name: Option<String>,
        sources: Option<std::collections::HashMap<String, Option<Vec<(u32, String, Py<PyAny>)>>>>,
    ) -> PyResult<Bound<'py, PyAny>> {
        match &ast.outcome {
            AstOutcome::Ok(av) => render_av(py, av, ctx, rt, name, sources),
            // Both failure kinds carry their own encoding; rebuild the tuple from
            // stored fields exactly as a fresh parse would have.
            AstOutcome::LexErr(e) | AstOutcome::PErr(e) => err_tuple(py, e),
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

    /// Decide whether a parsed template may be served from rust against a live
    /// context, running only the compile-time analyses (never executing).  Returns
    /// `(supported, reason)`.  See `eval::check_supported` -- slice C-v8a.
    #[pyfunction]
    #[pyo3(signature = (ast, ctx, rt, name = None))]
    fn check_supported<'py>(
        py: Python<'py>,
        ast: &ParsedAst,
        ctx: Py<PyAny>,
        rt: Py<PyAny>,
        name: Option<String>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        match &ast.outcome {
            AstOutcome::Ok(av) => {
                let verdict = eval::check_supported(
                    py,
                    av,
                    ctx.bind(py).clone(),
                    rt.bind(py).clone(),
                    name,
                );
                let (ok, reason): (bool, Option<String>) = match verdict {
                    eval::ServeVerdict::Supported => (true, None),
                    eval::ServeVerdict::Refused(r) => (false, Some(r)),
                };
                Ok(PyTuple::new(
                    py,
                    [
                        ok.into_bound_py_any(py)?,
                        match reason {
                            Some(r) => r.into_bound_py_any(py)?,
                            None => py.None().into_bound(py).into_any(),
                        },
                    ],
                )?)
            }
            // A stored parse failure means the template cannot even be evaluated:
            // refuse service and let python codegen raise the real error.
            AstOutcome::LexErr(_) | AstOutcome::PErr(_) => Ok(PyTuple::new(
                py,
                [
                    false.into_bound_py_any(py)?,
                    "parse error".into_bound_py_any(py)?,
                ],
            )?),
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
        m.add_function(wrap_pyfunction!(compile_ast, m)?)?;
        m.add_function(wrap_pyfunction!(eval_parsed, m)?)?;
        m.add_function(wrap_pyfunction!(check_supported, m)?)?;
        m.add_class::<ParsedAst>()?;
        m.add_class::<eval::MacroFunc>()?;
        Ok(())
    }
}
