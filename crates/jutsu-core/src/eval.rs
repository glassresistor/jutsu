//! Tree-walking evaluator, slice v0 (see PLAN_INTERP.md).
//!
//! Evaluates the rust parser's `Av` AST directly -- no python node rebuild --
//! against a plain python dict of variables.  Supported subset: `Template`,
//! `Output`, `TemplateData`, `Const`, `Name` (missing names render as empty
//! string, mirroring jinja's default `Undefined`).  Anything else is reported
//! as unsupported so callers can fall back to the python engine.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

use crate::parser::Av;

/// Result of an evaluation attempt.
#[derive(Debug)]
pub enum EvalOutcome {
    /// Fully rendered template text.
    Rendered(String),
    /// A node this slice cannot evaluate (named) -- caller should fall back.
    Unsupported(String),
    /// Evaluation hit a runtime failure (str()/lookup).
    Failed(String),
}

enum Step {
    Done(String),
    Nope(String),
    Fail(String),
}

fn str_of(py: Python<'_>, obj: &Bound<PyAny>) -> Result<String, String> {
    if let Ok(s) = obj.downcast::<PyString>() {
        return Ok(s.to_string());
    }
    match obj.call_method0("__str__") {
        Ok(v) => match v.extract::<String>() {
            Ok(s) => Ok(s),
            Err(_) => Err("str() returned non-str".into()),
        },
        Err(_) => Err("str() failed".into()),
    }
}

fn value_str(py: Python<'_>, av: &Av) -> Result<String, String> {
    match av {
        Av::Str(s) => Ok(s.clone()),
        Av::Bool(b) => Ok(if *b { "True".to_string() } else { "False".to_string() }),
        Av::NoneV => Ok("None".to_string()),
        Av::Val(o) => {
            let b = o.bind(py);
            str_of(py, &b)
        }
        _ => Err("not a value".into()),
    }
}

fn get_name_str(av: &Av) -> Result<String, String> {
    match av {
        Av::Str(s) => Ok(s.clone()),
        _ => Err("bad name".into()),
    }
}

struct Ev<'py> {
    py: Python<'py>,
    dict: Bound<'py, PyDict>,
}

impl<'py> Ev<'py> {
    fn body(&self, items: &[Av]) -> Step {
        let mut out = String::new();
        for it in items {
            match self.node(it) {
                Step::Done(s) => out.push_str(&s),
                Step::Nope(n) => return Step::Nope(n),
                Step::Fail(f) => return Step::Fail(f),
            }
        }
        Step::Done(out)
    }

    fn node(&self, av: &Av) -> Step {
        let (name, args): (&str, &[Av]) = match av {
            Av::Node(name, _, args) => (*name, args),
            _ => return Step::Nope("non-node".into()),
        };
        match name {
            "Template" => match &args[0] {
                Av::List(items) => self.body(items),
                _ => Step::Fail("bad template body".into()),
            },
            "Output" => match &args[0] {
                Av::List(items) => self.body(items),
                _ => Step::Fail("bad output".into()),
            },
            "TemplateData" | "Const" => match args.first() {
                Some(a) => match value_str(self.py, a) {
                    Ok(s) => Step::Done(s),
                    Err(e) => Step::Fail(e),
                },
                None => Step::Fail("valueless node".into()),
            },
            "Name" => {
                let nm = match get_name_str(&args[0]) {
                    Ok(n) => n,
                    Err(_) => return Step::Nope("odd name".into()),
                };
                match self.dict.contains(&nm) {
                    Ok(true) => {}
                    Ok(false) => return Step::Done(String::new()),
                    Err(_) => return Step::Fail("ctx lookup failed".into()),
                }
                let v = match self.dict.get_item(&nm) {
                    Ok(Some(v)) => v,
                    Ok(None) => return Step::Done(String::new()),
                    Err(_) => return Step::Fail("ctx get failed".into()),
                };
                match str_of(self.py, &v) {
                    Ok(s) => Step::Done(s),
                    Err(e) => Step::Fail(e),
                }
            }
            other => Step::Nope(other.to_string()),
        }
    }
}

/// Evaluate a parsed template against `ctx` (must be a python dict).
pub fn evaluate(py: Python<'_>, av: &Av, ctx: Py<PyAny>) -> EvalOutcome {
    let dict = match ctx.bind(py).downcast::<PyDict>() {
        Ok(d) => d.clone(),
        Err(_) => return EvalOutcome::Failed("context is not a dict".into()),
    };
    let ev = Ev { py, dict };
    match ev.node(av) {
        Step::Done(s) => EvalOutcome::Rendered(s),
        Step::Nope(n) => EvalOutcome::Unsupported(n),
        Step::Fail(f) => EvalOutcome::Failed(f),
    }
}
