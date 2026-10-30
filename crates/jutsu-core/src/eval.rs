//! Tree-walking evaluator (Phase C, see PLAN_INTERP.md).
//!
//! Slice v1: expressions.  The rust parser's `Av` tree is evaluated directly
//! against a live python `Context`, mirroring the python code generator
//! operation by operation: name lookups go through `Context.resolve`,
//! attribute/item access through the `Environment` helpers, calls through
//! `Context.call`, filters and tests through `Environment._filter_test_common`.
//! Values are always real python objects, so runtime behaviour (including the
//! exceptions raised by operators on `Undefined`) matches the compiled engine.
//! Anything outside the subset -- statements, dynamic call arguments,
//! volatile eval contexts, environment finalize functions -- is reported as
//! unsupported so callers fall back to the python engine.

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::parser::Av;
use crate::pyrepr::py_repr_str;

/// Bail-out from an evaluation attempt.
pub enum EvalErr {
    /// Feature outside the supported subset -- caller should fall back.
    Unsupported(String),
    /// A genuine python exception raised while evaluating.
    Exc(PyErr),
}

impl From<PyErr> for EvalErr {
    fn from(err: PyErr) -> Self {
        EvalErr::Exc(err)
    }
}

type ERes<T> = Result<T, EvalErr>;

fn nope(what: impl Into<String>) -> EvalErr {
    EvalErr::Unsupported(what.into())
}

/// Result of a whole-template evaluation.
pub enum EvalOutcome<'py> {
    /// Fully rendered text (a python str object).
    Rendered(Bound<'py, PyAny>),
    /// Node or feature that forced a fallback to the python engine.
    Unsupported(String),
    /// Runtime exception raised during evaluation.
    Exc(PyErr),
}

fn parts<'a>(av: &'a Av) -> Option<(&'static str, &'a [Av])> {
    match av {
        Av::Node(n, _, a) => Some((*n, a)),
        _ => None,
    }
}

fn arg_str(av: &Av) -> Option<&str> {
    match av {
        Av::Str(s) => Some(s.as_str()),
        _ => None,
    }
}


pub struct Ev<'py> {
    py: Python<'py>,
    env: Bound<'py, PyAny>,
    ctx: Bound<'py, PyAny>,
    evalctx: Bound<'py, PyAny>,
    rt: Bound<'py, PyAny>,
    filters: Bound<'py, PyDict>,
    tests: Bound<'py, PyDict>,
    slice_cls: Bound<'py, PyAny>,
    autoescape: bool,
    name: Option<String>,
}

impl<'py> Ev<'py> {
    fn new(
        py: Python<'py>,
        ctx: Bound<'py, PyAny>,
        rt: Bound<'py, PyAny>,
        name: Option<String>,
    ) -> Result<Ev<'py>, EvalErr> {
        let env = ctx.getattr("environment")?;
        let evalctx = ctx.getattr("eval_ctx")?;
        let volatile: bool = evalctx.getattr("volatile")?.extract()?;
        if volatile {
            return Err(nope("volatile eval context"));
        }
        let autoescape: bool = evalctx.getattr("autoescape")?.extract()?;
        if autoescape {
            // The python code generator folds constant output children at
            // compile time with plain joins (`Concat.as_const`), which is not
            // markup aware; reproducing that together with `markup_join`
            // semantics is the dedicated autoescape slice (see TODO.md).
            return Err(nope("autoescape environment"));
        }
        let filters = match env.getattr("filters")?.downcast_into::<PyDict>() {
            Ok(d) => d,
            Err(_) => return Err(nope("environment.filters is not a dict")),
        };
        let tests = match env.getattr("tests")?.downcast_into::<PyDict>() {
            Ok(d) => d,
            Err(_) => return Err(nope("environment.tests is not a dict")),
        };
        let slice_cls = py.import("builtins")?.getattr("slice")?;
        Ok(Ev {
            py,
            env,
            ctx,
            evalctx,
            rt,
            filters,
            tests,
            slice_cls,
            autoescape,
            name,
        })
    }


    fn resolve(&self, name: &str) -> ERes<Bound<'py, PyAny>> {
        Ok(self.ctx.call_method1("resolve", (name,))?)
    }

    fn none(&self) -> Bound<'py, PyAny> {
        self.py.None().bind(self.py).clone()
    }

    fn lit(&self, av: &Av) -> ERes<Bound<'py, PyAny>> {
        match av {
            Av::Str(s) => Ok(PyString::new(self.py, s).into_any()),
            Av::Bool(b) => Ok((*b).into_bound_py_any(self.py)?),
            Av::NoneV => Ok(self.none()),
            Av::Val(o) => Ok(o.bind(self.py).clone()),
            Av::Join(items) => {
                let mut buf = String::new();
                for it in items {
                    match arg_str(it) {
                        Some(s) => buf.push_str(s),
                        None => return Err(nope("non-string join part")),
                    }
                }
                Ok(PyString::new(self.py, &buf).into_any())
            }
            _ => Err(nope("literal expected")),
        }
    }

    fn out_str(&self, v: &Bound<'py, PyAny>) -> ERes<Bound<'py, PyAny>> {
        if self.autoescape {
            let esc = self.rt.getattr("escape")?;
            Ok(esc.call1((v,))?.str()?.into_any())
        } else {
            Ok(v.str()?.into_any())
        }
    }

    /// Concatenation of a `Concat` node (`~`), which is markup aware.  The
    /// python code generator uses `markup_join` when autoescape is on and
    /// `str_join` otherwise.
    fn concat_values(&self, parts: Vec<Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
        let tup = PyTuple::new(self.py, parts)?;
        if self.autoescape {
            let mj = self.rt.getattr("markup_join")?;
            Ok(mj.call1((tup,))?)
        } else {
            let sj = self.rt.getattr("str_join")?;
            Ok(sj.call1((tup,))?)
        }
    }

    /// Join of already finalized output pieces: the compiled engine yields
    /// strings and joins them with `Environment.concat` (plain `"".join`), so
    /// static template text is never escaped here.
    fn out_join(&self, chunks: Vec<Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
        let lst = PyList::new(self.py, chunks)?;
        let empty = PyString::new(self.py, "");
        Ok(empty.call_method1("join", (&lst,))?)
    }

    /// Subscript of a `Getitem` node.  Slices bypass `Environment.getitem`
    /// exactly like the generated code does (`obj[start:stop]`).
    fn subscript(&self, obj: &Bound<'py, PyAny>, arg: &Av) -> ERes<Bound<'py, PyAny>> {
        if let Some(("Slice", sa)) = parts(arg) {
            let mut vals: Vec<Bound<'py, PyAny>> = Vec::with_capacity(3);
            for s in sa.iter().take(3) {
                if matches!(s, Av::NoneV) {
                    vals.push(self.none())
                } else {
                    vals.push(self.node(s)?)
                }
            }
            while vals.len() < 3 {
                vals.push(self.none())
            }
            let sl = self.slice_cls.call(
                (vals[0].clone(), vals[1].clone(), vals[2].clone()),
                None,
            )?;
            return Ok(obj.get_item(sl)?);
        }
        let key = self.node(arg)?;
        Ok(self.env.call_method1("getitem", (obj, key))?)
    }

    /// Positional and keyword arguments of a call, filter or test.  `prefix`
    /// is the value node of a filter/test target placed before them.
    fn sig(
        &self,
        al: &[Av],
        kl: &[Av],
        prefix: Option<Bound<'py, PyAny>>,
    ) -> ERes<(Vec<Bound<'py, PyAny>>, Option<Bound<'py, PyDict>>)> {
        let mut args: Vec<Bound<PyAny>> = Vec::new();
        if let Some(p) = prefix {
            args.push(p);
        }
        for a in al {
            args.push(self.node(a)?);
        }
        if kl.is_empty() {
            return Ok((args, None));
        }
        let kw = PyDict::new(self.py);
        for k in kl {
            let (kn, ka) = match parts(k) {
                Some(x) => x,
                None => return Err(nope("keyword node expected")),
            };
            if kn != "Keyword" || ka.len() < 2 {
                return Err(nope("bad keyword node"));
            }
            let key = match arg_str(&ka[0]) {
                Some(s) => s,
                None => return Err(nope("keyword name")),
            };
            kw.set_item(key, self.node(&ka[1])?)?;
        }
        Ok((args, Some(kw)))
    }

    /// A `Filter` or `Test` node.  Argument indices follow the node fields
    /// (`node`, `name`, `args`, `kwargs`, `dyn_args`, `dyn_kwargs`).  Missing
    /// filters and tests are reported as unsupported so the python engine
    /// decides whether that is a compile-time or a runtime error.
    fn filter_test(&self, na: &[Av], is_filter: bool) -> ERes<Bound<'py, PyAny>> {
        if na.len() < 6 {
            return Err(nope("bad filter/test node"));
        }
        if !matches!(&na[4], Av::NoneV) || !matches!(&na[5], Av::NoneV) {
            return Err(nope("dynamic filter/test arguments"));
        }
        let name = match arg_str(&na[1]) {
            Some(s) => s,
            None => return Err(nope("filter/test name")),
        };
        let map = if is_filter { &self.filters } else { &self.tests };
        if map.get_item(name)?.is_none() {
            let kind = if is_filter { "filter" } else { "test" };
            return Err(nope(format!("missing {} {:?}", kind, name)));
        }
        if matches!(&na[0], Av::NoneV) {
            return Err(nope("implicit filter block target"));
        }
        let value = self.node(&na[0])?;
        // the target value is passed separately; `_filter_test_common` puts it
        // in front of the argument list itself.
        let (alist, kw) = match (&na[2], &na[3]) {
            (Av::List(a), Av::List(k)) => self.sig(a, k, None)?,
            _ => return Err(nope("filter/test argument lists")),
        };
        let args_list = PyList::new(self.py, alist)?;
        let kwargs_arg = match kw {
            Some(d) => d.into_any(),
            None => self.none(),
        };
        let flag = is_filter.into_bound_py_any(self.py)?;
        Ok(self.env.call_method(
            "_filter_test_common",
            (name, &value, &args_list, &kwargs_arg, &self.ctx, &self.evalctx, &flag),
            None,
        )?)
    }

    /// A `Call` node: the compiled code calls through `Context.call`, which
    /// injects the context/environment for `pass_context` callables.
    fn call_node(&self, na: &[Av]) -> ERes<Bound<'py, PyAny>> {
        if na.len() < 5 {
            return Err(nope("bad call node"));
        }
        if !matches!(&na[3], Av::NoneV) || !matches!(&na[4], Av::NoneV) {
            return Err(nope("dynamic call arguments"));
        }
        let func = self.node(&na[0])?;
        let (alist, kw) = match (&na[1], &na[2]) {
            (Av::List(a), Av::List(k)) => self.sig(a, k, Some(func.clone()))?,
            _ => return Err(nope("call argument lists")),
        };
        let tup = PyTuple::new(self.py, alist)?;
        Ok(self.ctx.call_method("call", tup, kw.as_ref())?)
    }

    /// Chained comparison, mirroring python's semantics: operands evaluate
    /// left to right and the whole thing short-circuits on the first false
    /// comparison without evaluating the remaining operands.
    fn compare(&self, expr: &Av, ops: &[Av]) -> ERes<Bound<'py, PyAny>> {
        let mut cur = self.node(expr)?;
        for op in ops {
            let (on, oa) = match parts(op) {
                Some(x) => x,
                None => return Err(nope("operand node expected")),
            };
            if on != "Operand" || oa.len() < 2 {
                return Err(nope("bad operand node"));
            }
            let opc = match arg_str(&oa[0]) {
                Some(s) => s,
                None => return Err(nope("comparison operator")),
            };
            let rhs = self.node(&oa[1])?;
            let ok: bool = match opc {
                "eq" => cur.eq(&rhs)?,
                "ne" => cur.ne(&rhs)?,
                "lt" => cur.lt(&rhs)?,
                "lteq" => cur.le(&rhs)?,
                "gt" => cur.gt(&rhs)?,
                "gteq" => cur.ge(&rhs)?,
                "in" => rhs.contains(&cur)?,
                "notin" => !rhs.contains(&cur)?,
                _ => return Err(nope(format!("comparison operator {opc}"))),
            };
            if !ok {
                return Ok(false.into_bound_py_any(self.py)?);
            }
            cur = rhs;
        }
        Ok(true.into_bound_py_any(self.py)?)
    }

    /// Inline `x if y else z`.  The implicit else creates a plain runtime
    /// `Undefined` (not the environment's one), exactly like the code
    /// generator's `cond_expr_undefined`.
    fn condexpr(&self, lineno: Option<u32>, na: &[Av]) -> ERes<Bound<'py, PyAny>> {
        if na.len() < 3 {
            return Err(nope("bad conditional expression"));
        }
        let t = self.node(&na[0])?;
        if t.is_truthy()? {
            return self.node(&na[1]);
        }
        if !matches!(&na[2], Av::NoneV) {
            return self.node(&na[2]);
        }
        let mut pos = match lineno {
            Some(l) => format!("line {}", l),
            None => "unknown".to_string(),
        };
        if let Some(nm) = &self.name {
            pos = format!("{} in {}", pos, py_repr_str(nm));
        }
        let hint = format!(
            "the inline if-expression on {} evaluated to false and no else section was defined.",
            pos
        );
        Ok(self.rt.getattr("Undefined")?.call1((hint,))?)
    }

    fn two(&self, args: &[Av]) -> ERes<(Bound<'py, PyAny>, Bound<'py, PyAny>)> {
        if args.len() < 2 {
            return Err(nope("binary node expected two children"));
        }
        Ok((self.node(&args[0])?, self.node(&args[1])?))
    }

    fn output_items(&self, items: &[Av]) -> ERes<Bound<'py, PyAny>> {
        let mut chunks: Vec<Bound<PyAny>> = Vec::new();
        for it in items {
            match it {
                Av::Node("TemplateData", _, td) => chunks.push(self.lit(&td[0])?),
                _ => {
                    let v = self.node(it)?;
                    chunks.push(self.out_str(&v)?);
                }
            }
        }
        self.out_join(chunks)
    }

    fn node(&self, av: &Av) -> ERes<Bound<'py, PyAny>> {
        let (name, lineno, args): (&str, Option<u32>, &[Av]) = match av {
            Av::Node(n, l, a) => (*n, *l, a),
            _ => return Err(nope("node expected")),
        };
        if args.is_empty() && !matches!(name, "Const") {
            return Err(nope(format!("{name} without children")));
        }
        match name {
            "Const" | "TemplateData" => self.lit(&args[0]),
            "Name" => {
                let nm = match arg_str(&args[0]) {
                    Some(s) => s,
                    None => return Err(nope("name node")),
                };
                if args.len() < 2 || arg_str(&args[1]) != Some("load") {
                    return Err(nope("non-loading name reference"));
                }
                self.resolve(nm)
            }
            "Getattr" => {
                let obj = self.node(&args[0])?;
                let attr = match arg_str(&args[1]) {
                    Some(s) => s,
                    None => return Err(nope("attribute name")),
                };
                Ok(self.env.call_method1("getattr", (&obj, attr))?)
            }
            "Getitem" => {
                let obj = self.node(&args[0])?;
                self.subscript(&obj, &args[1])
            }
            "Tuple" | "List" => match &args[0] {
                Av::List(items) => {
                    let mut vals: Vec<Bound<PyAny>> = Vec::new();
                    for it in items {
                        vals.push(self.node(it)?);
                    }
                    if name == "Tuple" {
                        Ok(PyTuple::new(self.py, vals)?.into_any())
                    } else {
                        Ok(PyList::new(self.py, vals)?.into_any())
                    }
                }
                _ => Err(nope("tuple/list items")),
            },
            "Dict" => match &args[0] {
                Av::List(items) => {
                    let d = PyDict::new(self.py);
                    for it in items {
                        let (pn, pa) = match parts(it) {
                            Some(x) => x,
                            None => return Err(nope("dict entry")),
                        };
                        if pn != "Pair" || pa.len() < 2 {
                            return Err(nope("bad pair node"));
                        }
                        let k = self.node(&pa[0])?;
                        let v = self.node(&pa[1])?;
                        d.set_item(&k, &v)?;
                    }
                    Ok(d.into_any())
                }
                _ => Err(nope("dict items")),
            },
            "CondExpr" => self.condexpr(lineno, args),
            "Or" | "And" => {
                let l = self.node(&args[0])?;
                let truth = l.is_truthy()?;
                if truth == (name == "Or") {
                    Ok(l)
                } else {
                    self.node(&args[1])
                }
            }
            "Not" => {
                let v = self.node(&args[0])?;
                Ok((!v.is_truthy()?).into_bound_py_any(self.py)?)
            }
            "Compare" => match &args[1] {
                Av::List(ops) => self.compare(&args[0], ops),
                _ => Err(nope("comparison operands")),
            },
            "Concat" => match &args[0] {
                Av::List(ps) => {
                    let mut vals: Vec<Bound<PyAny>> = Vec::new();
                    for p in ps {
                        vals.push(self.node(p)?);
                    }
                    self.concat_values(vals)
                }
                _ => Err(nope("concat parts")),
            },
            "Call" => self.call_node(args),
            "Filter" => self.filter_test(args, true),
            "Test" => self.filter_test(args, false),
            "Add" => { let (a, b) = self.two(args)?; Ok(a.add(&b)?) }
            "Sub" => { let (a, b) = self.two(args)?; Ok(a.sub(&b)?) }
            "Mul" => { let (a, b) = self.two(args)?; Ok(a.mul(&b)?) }
            "Div" => { let (a, b) = self.two(args)?; Ok(a.div(&b)?) }
            "FloorDiv" => { let (a, b) = self.two(args)?; Ok(a.floor_div(&b)?) }
            "Mod" => { let (a, b) = self.two(args)?; Ok(a.rem(&b)?) }
            "Pow" => {
                let (a, b) = self.two(args)?;
                Ok(a.pow(&b, self.py.None())?)
            }
            "Pos" => Ok(self.node(&args[0])?.pos()?),
            "Neg" => Ok(self.node(&args[0])?.neg()?),
            "Output" => match &args[0] {
                Av::List(items) => self.output_items(items),
                _ => Err(nope("output items")),
            },
            "Template" => match &args[0] {
                Av::List(items) => {
                    let mut chunks: Vec<Bound<PyAny>> = Vec::new();
                    for it in items {
                        match it {
                            Av::Node("Output", _, oa) => match &oa[0] {
                                Av::List(inner) => chunks.push(self.output_items(inner)?),
                                _ => return Err(nope("bad output node")),
                            },
                            Av::Node(other, _, _) => {
                                return Err(nope(format!("{other} outside the subset")))
                            }
                            _ => return Err(nope("template body item")),
                        }
                    }
                    self.out_join(chunks)
                }
                _ => Err(nope("template body")),
            },
            other => Err(nope(other.to_string())),
        }
    }
}

/// Evaluate a parsed template against a live python `Context` (`ctx`) using
/// the `jutsu.runtime` module (`rt`) for the handful of runtime helpers that
/// must come from python.  `name` is the template name used in the message of
/// the implicit-else conditional expression, mirroring `CodeGenerator.name`.
pub fn evaluate<'py>(
    py: Python<'py>,
    av: &Av,
    ctx: Bound<'py, PyAny>,
    rt: Bound<'py, PyAny>,
    name: Option<String>,
) -> EvalOutcome<'py> {
    let ev = match Ev::new(py, ctx, rt, name) {
        Ok(e) => e,
        Err(EvalErr::Exc(e)) => return EvalOutcome::Exc(e),
        Err(EvalErr::Unsupported(u)) => return EvalOutcome::Unsupported(u),
    };
    match ev.node(av) {
        Ok(v) => EvalOutcome::Rendered(v),
        Err(EvalErr::Unsupported(u)) => EvalOutcome::Unsupported(u),
        Err(EvalErr::Exc(e)) => EvalOutcome::Exc(e),
    }
}


