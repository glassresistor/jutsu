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

use std::cell::RefCell;
use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::parser::Av;
use crate::pyrepr::py_repr_str;

pyo3::import_exception!(jutsu.exceptions, TemplateRuntimeError);

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
    /// The `runtime.missing` sentinel.
    missing: Bound<'py, PyAny>,
    /// `environment.undefined`, the undefined factory used at load sites.
    undef: Bound<'py, PyAny>,
    /// `runtime.LoopContext` class.
    loopctx: Bound<'py, PyAny>,
    /// `runtime.Namespace` class.
    ns: Bound<'py, PyAny>,
    /// Live scope frames, innermost last.
    stack: RefCell<Vec<Scope<'py>>>,
    /// Loop contexts created for recursive loops, awaiting recursive calls.
    rec: RefCell<Vec<RecEntry>>
}

/// One runtime scope frame: slots by name; `None` is the `missing` sentinel.
struct Scope<'py> {
    slots: HashMap<String, Option<Bound<'py, PyAny>>>,
}

/// How a slot is initialized when its scope is entered (mirrors the code
/// generator's load instructions ALIAS / RESOLVE / UNDEFINED / PARAMETER).
#[derive(Clone, Copy, PartialEq)]
pub enum IK {
    Param,
    Alias,
    Resolve,
    Undef,
}

/// A loop context object created for a recursive `for` statement.
struct RecEntry {
    ptr: usize,
    args: Vec<Av>,
}

/// Node names the evaluator understands (expressions plus C-v2 statements).
pub const SUPPORTED_NODES: &[&str] = &[
    "Const", "TemplateData", "Name", "Getattr", "Getitem", "Tuple", "List", "Dict",
    "Pair", "CondExpr", "Or", "And", "Not", "Compare", "Operand", "Concat", "Call",
    "Filter", "Test", "Add", "Sub", "Mul", "Div", "FloorDiv", "Mod", "Pow", "Pos",
    "Neg", "Slice", "Keyword", "Output", "If", "Assign", "AssignBlock", "FilterBlock",
    "For", "With", "NSRef", "Template",
];

/// Occurrence kinds collected by the scope symbol walk.
const OC_LOAD: u8 = 0;
const OC_STORE: u8 = 1;
const OC_PARAM: u8 = 2;

/// One occurrence collected in a scope region.  `Branch` marks an `if`
/// statement: its test occurrences were already pushed before this entry,
/// the branch groups carry the occurrences that belong to the per-branch
/// symbol copies (`Symbols.copy`) which get merged with `branch_update`.
#[derive(Clone)]
enum Occ {
    One(String, u8),
    Branch {
        body: Vec<Occ>,
        elifs: Vec<Occ>,
        els: Vec<Occ>,
    },
}

/// The per-region symbol state mirrored from `idtracking.Symbols`.
#[derive(Clone)]
struct Sym {
    refs: HashMap<String, IK>,
    stores: std::collections::HashSet<String>,
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
        if !env.getattr("finalize")?.is_none() {
            return Err(nope("environment finalize function"));
        }
        let missing = rt.getattr("missing")?;
        let loopctx = rt.getattr("LoopContext")?;
        let ns = rt.getattr("Namespace")?;
        let undef = env.getattr("undefined")?;
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
            missing,
            undef,
            loopctx,
            ns,
            stack: RefCell::new(Vec::new()),
            rec: RefCell::new(Vec::new()),
        })
    }


    /// Raw context lookup: `Context.resolve_or_missing`, `None` for missing.
    fn raw_resolve(&self, name: &str) -> Result<Option<Bound<'py, PyAny>>, PyErr> {
        let rv = self.ctx.call_method1("resolve_or_missing", (name,))?;
        if rv.is(&self.missing) {
            Ok(None)
        } else {
            Ok(Some(rv))
        }
    }

    /// Wrap a slot value for a load site: missing becomes a fresh undefined
    /// created by the environment's factory, exactly like the generated
    /// `(undefined(name='x') if l_0_x is missing else l_0_x)`.
    fn wrap(&self, name: &str, v: Option<&Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
        match v {
            Some(x) => Ok(x.clone()),
            None => {
                let kw = PyDict::new(self.py);
                kw.set_item("name", name)?;
                Ok(self.undef.call((), Some(&kw))?)
            }
        }
    }

    /// Scope-aware name lookup for load sites: walk the scope chain from the
    /// innermost frame; slots win, otherwise read from the python context.
    fn lookup(&self, name: &str) -> ERes<Bound<'py, PyAny>> {
        {
            let frames = self.stack.borrow();
            for frame in frames.iter().rev() {
                if let Some(v) = frame.slots.get(name) {
                    return self.wrap(name, v.as_ref());
                }
            }
        }
        let raw = self.raw_resolve(name)?;
        self.wrap(name, raw.as_ref())
    }

    /// Names owned by any live scope (the compiled engine's `find_ref` set).
    fn owned_union(&self) -> Vec<String> {
        let frames = self.stack.borrow();
        let mut out: Vec<String> = Vec::new();
        for frame in frames.iter() {
            for k in frame.slots.keys() {
                if !out.contains(k) {
                    out.push(k.clone());
                }
            }
        }
        out
    }

    /// Value of the nearest owning slot for `name`, walking innermost first.
    fn nearest_slot(&self, name: &str) -> Option<Option<Bound<'py, PyAny>>> {
        let frames = self.stack.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.slots.get(name) {
                return Some(v.clone());
            }
        }
        None
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
        if matches!(&na[0], Av::NoneV) {
            return Err(nope("implicit filter block target"));
        }
        let value = self.node(&na[0])?;
        self.filter_common(na, is_filter, value)
    }

    /// The shared body of a filter/test application.  `value` is the target
    /// the pipeline attaches to it; `None` means the implicit target hole of
    /// a filter/call block (the caller supplies the buffered content).
    fn filter_common(&self, na: &[Av], is_filter: bool, value: Bound<'py, PyAny>) -> ERes<Bound<'py, PyAny>> {
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
        // Recursive loop interception: calling a loop context created by a
        // recursive `for` statement renders one recursion level in rust,
        // mirroring the compiled engine's LoopContext.__call__ -> _recurse.
        if let (Av::List(al), Av::List(kl)) = (&na[1], &na[2]) {
            if kl.is_empty() && al.len() == 1 {
                if let Some(entry) = self.rec_find(&func) {
                    let arg = self.node(&al[0])?;
                    let depth: i64 = func.getattr("depth")?.extract()?;
                    return self.run_recursive(&entry, arg, depth);
                }
            }
        }
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
                self.lookup(nm)
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
other => Err(nope(other.to_string())),
        }
    }
    // ---------------------------------------------------------------- C-v2 --
    //
    // Statement execution with compiled-engine scoping semantics.  Every hard
    // scope (root, for-body, for-else, with, set-block body) owns a frame of
    // slots.  Occurrences are collected in source order (the symbol visitor
    // visits an assignment's expression before its target), and the first
    // occurrence of a name decides how the slot initializes when the scope is
    // entered: parameters are not loaded, stores alias-copy from the nearest
    // owning ancestor slot or become `missing`, and loads only own a slot
    // when no scope in the chain owns the name -- then they resolve from the
    // python context at each entry.

    /// FrameSymbolVisitor-style symbol walk over one region.
    fn walk_fsf(&self, av: &Av, occs: &mut Vec<Occ>) {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.walk_fsf(a, occs);
                }
            }
            Av::Node("Name", _, a) => {
                if a.len() >= 2 {
                    let n = match arg_str(&a[0]) {
                        Some(n) => n,
                        None => return,
                    };
                    let kind = match arg_str(&a[1]) {
                        Some("load") => OC_LOAD,
                        Some("store") => OC_STORE,
                        Some("param") => OC_PARAM,
                        _ => return,
                    };
                    occs.push(Occ::One(n.to_string(), kind));
                }
            }
            Av::Node("NSRef", _, a) => {
                if let Some(n) = arg_str(&a[0]) {
                    occs.push(Occ::One(n.to_string(), OC_LOAD));
                }
            }
            // `if` statements split the region into per-branch symbol copies;
            // the test applies to the current state before the copies exist.
            Av::Node("If", _, a) => {
                if a.len() > 0 {
                    self.walk_fsf(&a[0], occs);
                }
                let mut body: Vec<Occ> = Vec::new();
                let mut elifs: Vec<Occ> = Vec::new();
                let mut els: Vec<Occ> = Vec::new();
                if a.len() > 1 {
                    if let Av::List(b) = &a[1] {
                        for it in b {
                            self.walk_fsf(it, &mut body);
                        }
                    }
                }
                if a.len() > 2 {
                    if let Av::List(es) = &a[2] {
                        for e in es {
                            // each elif is itself an If node: its test joins
                            // the evolving chain state, then it nests
                            self.walk_fsf(e, &mut elifs);
                        }
                    }
                }
                if a.len() > 3 {
                    if let Av::List(os) = &a[3] {
                        for it in os {
                            self.walk_fsf(it, &mut els);
                        }
                    }
                }
                occs.push(Occ::Branch { body, elifs, els });
            }
            // visiting stops at nested hard scopes
            Av::Node("Scope" | "Block" | "OverlayScope", _, _) => {}
            // only the outer expression of a nested construct belongs here
            Av::Node("For", _, a) => {
                if a.len() > 1 {
                    self.walk_fsf(&a[1], occs);
                }
            }
            Av::Node("CallBlock", _, a) => {
                if !a.is_empty() {
                    self.walk_fsf(&a[0], occs);
                }
            }
            Av::Node("FilterBlock", _, a) => {
                if a.len() > 1 && !matches!(&a[1], Av::NoneV) {
                    self.walk_fsf(&a[1], occs);
                }
            }
            Av::Node("Assign", _, a) => {
                // the symbol visitor visits expression, then target
                if a.len() > 1 {
                    self.walk_fsf(&a[1], occs);
                }
                if !a.is_empty() {
                    self.walk_fsf(&a[0], occs);
                }
            }
            Av::Node("AssignBlock", _, a) => {
                // body belongs to the inner block frame, target here
                if !a.is_empty() {
                    self.walk_fsf(&a[0], occs);
                }
            }
            Av::Node("With", _, a) => {
                // nested `with`: only its values belong to this scope
                if a.len() > 1 {
                    if let Av::List(vs) = &a[1] {
                        for v in vs {
                            self.walk_fsf(v, occs);
                        }
                    }
                }
            }
            Av::Node("Macro", _, a) => {
                if let Some(n) = a.first().and_then(|x| arg_str(x)) {
                    occs.push(Occ::One(n.to_string(), OC_STORE));
                }
            }
            Av::Node("Import", _, a) => {
                if !a.is_empty() {
                    self.walk_fsf(&a[0], occs);
                }
                if a.len() > 1 {
                    if let Some(n) = arg_str(&a[1]) {
                        occs.push(Occ::One(n.to_string(), OC_STORE));
                    }
                }
            }
            Av::Node("FromImport", _, a) => {
                if !a.is_empty() {
                    self.walk_fsf(&a[0], occs);
                }
                if a.len() > 1 {
                    if let Av::List(names) = &a[1] {
                        for nm in names {
                            match nm {
                                Av::Str(s) => occs.push(Occ::One(s.clone(), OC_STORE)),
                                Av::Tuple(t) => {
                                    if t.len() > 1 {
                                        if let Some(x) = arg_str(&t[1]) {
                                            occs.push(Occ::One(x.to_string(), OC_STORE));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.walk_fsf(x, occs);
                }
            }
            _ => {}
        }
    }

    /// Turn collected occurrences into slot kinds, mirroring `idtracking`
    /// exactly: unconditional visits apply to the region state; each `if`
    /// statement evaluates its branch groups against independent copies of the
    /// state and merges them with `branch_update` -- names stored inside
    /// branches (and not already stored unconditionally) become RESOLVE
    /// loads (or ALIAS when an ancestor owns them), never UNDEFINED slots.
    fn build_kinds(&self, occs: &[Occ], ancestors: &[String]) -> HashMap<String, IK> {
        let mut sym = Sym { refs: HashMap::new(), stores: std::collections::HashSet::new() };
        self.apply_occs(occs, &mut sym, ancestors);
        sym.refs
    }

    fn apply_occs(&self, occs: &[Occ], sym: &mut Sym, ancestors: &[String]) {
        for oc in occs {
            match oc {
                Occ::One(name, kind) => {
                    self.apply_one(name, *kind, sym, ancestors);
                }
                Occ::Branch { body, elifs, els } => {
                    // inner_visit(): each branch gets a copy of the state
                    let mut bs = sym.clone();
                    self.apply_occs(body, &mut bs, ancestors);
                    // the elif chain evolves on its own copy (each nested If
                    // merges into it as it is visited)
                    let mut es = sym.clone();
                    self.apply_occs(elifs, &mut es, ancestors);
                    let mut os = sym.clone();
                    self.apply_occs(els, &mut os, ancestors);
                    Self::branch_update(sym, [bs, es, os], ancestors);
                }
            }
        }
    }

    fn apply_one(&self, name: &str, kind: u8, sym: &mut Sym, ancestors: &[String]) {
        match kind {
            OC_PARAM => {
                // declare_parameter always re-defines the reference
                sym.stores.insert(name.to_string());
                sym.refs.insert(name.to_string(), IK::Param);
            }
            OC_STORE => {
                sym.stores.insert(name.to_string());
                if !sym.refs.contains_key(name) {
                    let k = if ancestors.iter().any(|a| a == name) {
                        IK::Alias
                    } else {
                        IK::Undef
                    };
                    sym.refs.insert(name.to_string(), k);
                }
            }
            _ => {
                // load: only defines when no reference exists anywhere in the
                // symbol chain (own refs plus the ancestor scope names)
                if !sym.refs.contains_key(name) && !ancestors.iter().any(|a| a == name) {
                    sym.refs.insert(name.to_string(), IK::Resolve);
                }
            }
        }
    }

    fn branch_update<const N: usize>(sym: &mut Sym, branches: [Sym; N], ancestors: &[String]) {
        // stores = union(branch.stores) - self.stores
        let mut stores: std::collections::HashSet<String> = std::collections::HashSet::new();
        for b in &branches {
            for s in &b.stores {
                if !sym.stores.contains(s) {
                    stores.insert(s.clone());
                }
            }
        }
        // merge refs/loads/stores in branch order (later definitions win)
        for b in &branches {
            for (name, k) in &b.refs {
                sym.refs.insert(name.clone(), *k);
            }
            sym.stores.extend(b.stores.iter().cloned());
        }
        // stored-inside-a-branch names resolve rather than stay undefined
        for name in stores {
            let k = if ancestors.iter().any(|a| a == &name) {
                IK::Alias
            } else {
                IK::Resolve
            };
            sym.refs.insert(name, k);
        }
    }

    fn init_scope(&self, kinds: &HashMap<String, IK>) -> ERes<Scope<'py>> {
        let mut slots: HashMap<String, Option<Bound<'py, PyAny>>> = HashMap::new();
        let names: Vec<String> = kinds.keys().cloned().collect();
        for name in names {
            let k = kinds[&name];
            let v = match k {
                IK::Param | IK::Undef => None,
                IK::Resolve => self.raw_resolve(&name)?,
                IK::Alias => match self.nearest_slot(&name) {
                    Some(v) => v,
                    None => None,
                },
            };
            slots.insert(name, v);
        }
        Ok(Scope { slots })
    }

    fn enter_scope(&self, kinds: &HashMap<String, IK>) -> ERes<()> {
        let sc = self.init_scope(kinds)?;
        self.stack.borrow_mut().push(sc);
        Ok(())
    }

    fn pop_scope(&self) {
        self.stack.borrow_mut().pop();
    }

    /// True if a Name node loading `target` appears anywhere below (the
    /// compiled engine's `find_undeclared`, without stop rules).
    fn deep_name_load(&self, av: &Av, target: &str) -> bool {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                inner.iter().any(|a| self.deep_name_load(a, target))
            }
            Av::Node("Name", _, a) => {
                a.len() >= 2
                    && arg_str(&a[0]).map(|s| s == target).unwrap_or(false)
                    && arg_str(&a[1]) == Some("load")
            }
            Av::Node(_, _, a) => a.iter().any(|x| self.deep_name_load(x, target)),
            _ => false,
        }
    }

    fn deep_name_load_in(&self, items: &[Av], target: &str) -> bool {
        items.iter().any(|a| self.deep_name_load(a, target))
    }

    fn exec_template(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        let items = match args.first() {
            Some(Av::List(l)) => l,
            _ => return Err(nope("bad template body")),
        };
        let mut occs: Vec<Occ> = Vec::new();
        for it in items {
            self.walk_fsf(it, &mut occs);
        }
        let kinds = self.build_kinds(&occs, &[]);
        self.enter_scope(&kinds)?;
        self.exec_items(items, out)?;
        self.pop_scope();
        Ok(())
    }

    fn exec_items(&self, items: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        for it in items {
            self.exec_one(it, out)?;
        }
        Ok(())
    }

    fn exec_one(&self, av: &Av, out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        let (name, args): (&str, &[Av]) = match av {
            Av::Node(n, _, a) => (*n, a),
            _ => return Err(nope("statement node expected")),
        };
        match name {
            "Output" => {
                let items = match args.first() {
                    Some(Av::List(l)) => l,
                    _ => return Err(nope("bad output node")),
                };
                for it in items {
                    match it {
                        Av::Node("TemplateData", _, td) => out.push(self.lit(&td[0])?),
                        _ => {
                            let v = self.node(it)?;
                            out.push(self.out_str(&v)?);
                        }
                    }
                }
                Ok(())
            }
            "If" => {
                if args.len() < 4 {
                    return Err(nope("bad if node"));
                }
                let mut chosen: Option<&Vec<Av>> = None;
                let t = self.node(&args[0])?;
                if t.is_truthy()? {
                    if let Av::List(b) = &args[1] {
                        chosen = Some(b);
                    }
                } else {
                    if let Av::List(elifs) = &args[2] {
                        for e in elifs {
                            let (en, ea) = match parts(e) {
                                Some(x) => x,
                                None => return Err(nope("bad elif")),
                            };
                            if en != "If" || ea.len() < 4 {
                                return Err(nope("bad elif node"));
                            }
                            let t2 = self.node(&ea[0])?;
                            if t2.is_truthy()? {
                                if let Av::List(b) = &ea[1] {
                                    chosen = Some(b);
                                }
                                break;
                            }
                        }
                    }
                    if chosen.is_none() {
                        if let Av::List(els) = &args[3] {
                            if !els.is_empty() {
                                chosen = Some(els);
                            }
                        }
                    }
                }
                match chosen {
                    Some(b) => self.exec_items(b, out),
                    None => Ok(()),
                }
            }
            "Assign" => {
                if args.len() < 2 {
                    return Err(nope("bad assign node"));
                }
                let val = self.node(&args[1])?;
                self.store_assign(&args[0], val)
            }
            "AssignBlock" => self.exec_assign_block(args),
            "FilterBlock" => self.exec_filter_block(args, out),
            "For" => self.exec_for(args, out),
            "With" => self.exec_with(args, out),
            _ => Err(nope(format!("{name} statement outside the subset"))),
        }
    }

    /// Collect stored names of an assignment target recursively.
    fn target_names(&self, tgt: &Av, out: &mut Vec<String>) {
        match tgt {
            Av::Node("Name", _, a) => {
                if let Some(n) = arg_str(&a[0]) {
                    out.push(n.to_string());
                }
            }
            Av::Node("Tuple" | "List", _, a) => {
                if let Some(Av::List(items)) = a.first() {
                    for i in items {
                        self.target_names(i, out);
                    }
                }
            }
            _ => {}
        }
    }

    /// Store an assignment value into a target and perform the root-level
    /// writeback into `context.vars` plus exported_vars bookkeeping.
    fn store_assign(&self, tgt: &Av, val: Bound<'py, PyAny>) -> ERes<()> {
        let mut names: Vec<String> = Vec::new();
        self.target_names(tgt, &mut names);
        self.store_target(tgt, &val, false)?;
        if !names.is_empty() && self.stack.borrow().len() == 1 {
            let vars = self.ctx.getattr("vars")?;
            for name in &names {
                if let Some(Some(vv)) = self.nearest_slot(name) {
                    vars.set_item(name.as_str(), vv)?;
                }
            }
            let public: Vec<String> = names
                .iter()
                .filter(|n| !n.starts_with('_'))
                .cloned()
                .collect();
            if !public.is_empty() {
                let ex = self.ctx.getattr("exported_vars")?;
                let strs: Vec<Bound<'py, PyAny>> = public
                    .iter()
                    .map(|s| PyString::new(self.py, s).into_any())
                    .collect();
                let lst = PyList::new(self.py, &strs)?;
                ex.call_method1("update", (&lst,))?;
            }
        }
        Ok(())
    }

    /// Load a namespace base object raw: the compiled engine checks
    /// `isinstance(ref, Namespace)` on the slot value itself.
    fn load_raw_ns_base(&self, name: &str) -> ERes<Bound<'py, PyAny>> {
        {
            let frames = self.stack.borrow();
            for frame in frames.iter().rev() {
                if let Some(v) = frame.slots.get(name) {
                    let kw = PyDict::new(self.py);
                    kw.set_item("name", name)?;
                    return Ok(match v {
                        Some(x) => x.clone(),
                        None => self.undef.call((), Some(&kw))?,
                    });
                }
            }
        }
        let raw = self.raw_resolve(name)?;
        let kw = PyDict::new(self.py);
        kw.set_item("name", name)?;
        Ok(match raw {
            Some(x) => x,
            None => self.undef.call((), Some(&kw))?,
        })
    }

    fn store_target(&self, tgt: &Av, val: &Bound<'py, PyAny>, iter_style: bool) -> ERes<()> {
        match tgt {
            Av::Node("Name", _, a) => {
                let name = match arg_str(&a[0]) {
                    Some(s) => s,
                    None => return Err(nope("name target")),
                };
                {
                    let mut frames = self.stack.borrow_mut();
                    for frame in frames.iter_mut().rev() {
                        if frame.slots.contains_key(name) {
                            frame.slots.insert(name.to_string(), Some(val.clone()));
                            return Ok(());
                        }
                    }
                }
                Err(nope(format!("store to unowned name {name:?}")))
            }
            Av::Node("NSRef", _, a) => {
                let base = match arg_str(&a[0]) {
                    Some(s) => s,
                    None => return Err(nope("nsref base")),
                };
                let attr = match a.get(1).and_then(arg_str) {
                    Some(s) => s,
                    None => return Err(nope("nsref attr")),
                };
                let obj = self.load_raw_ns_base(base)?;
                if !obj.is_instance(&self.ns)? {
                    return Err(EvalErr::Exc(PyErr::new::<TemplateRuntimeError, _>(
                        "cannot assign attribute on non-namespace object",
                    )));
                }
                obj.set_item(attr, val)?;
                Ok(())
            }
            Av::Node("Tuple" | "List", _, a) => {
                let items = match a.first() {
                    Some(Av::List(l)) => l,
                    _ => return Err(nope("bad tuple/list target")),
                };
                if items.is_empty() {
                    return Ok(());
                }
                let seq = self.unpack_seq(val, iter_style)?;
                let nseq = seq.len();
                if nseq != items.len() {
                    return Err(EvalErr::Exc(self.unpack_error(items.len(), nseq)));
                }
                {
                    for (t, v) in items.iter().zip(seq.iter()) {
                        self.store_target(t, &v, iter_style)?;
                    }
                }
                Ok(())
            }
            _ => Err(nope("bad assignment target")),
        }
    }

    /// Materialize a value for sequence unpacking.  `iter_style` selects the
    /// message of python's `for` statement (`'int' object is not iterable`)
    /// versus plain assignment (`cannot unpack non-iterable int object`).
    fn unpack_seq(&self, val: &Bound<'py, PyAny>, iter_style: bool) -> ERes<Bound<'py, PyList>> {
        let builtins = self.py.import("builtins")?;
        let lst_fn = builtins.getattr("list")?;
        match lst_fn.call1((val,)) {
            Ok(l) => match l.downcast_into::<PyList>() {
                Ok(p) => Ok(p),
                Err(e) => Err(EvalErr::Exc(e.into())),
            },
            Err(_) => {
                let tn: String = val.get_type().name()?.extract()?;
                if iter_style {
                    return Err(EvalErr::Exc(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                        format!("'{tn}' object is not iterable"),
                    )));
                }
                Err(EvalErr::Exc(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                    format!("cannot unpack non-iterable {tn} object"),
                )))
            }
        }
    }

    fn unpack_error(&self, need: usize, got: usize) -> PyErr {
        if got < need {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "not enough values to unpack (expected {need}, got {got})"
            ))
        } else {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                "too many values to unpack (expected {need})"
            ))
        }
    }

    /// `{% filter x %}body{% endfilter %}`: body renders into a buffer inside
    /// its own scope frame, the implicit filter target receives the joined
    /// buffer and the chain is applied, then the result goes through the
    /// normal output finalize (`str`), mirroring start_write/end_write.  A
    /// filter block without a filter expression cannot be built by the parser.
    fn exec_filter_block(
        &self,
        args: &[Av],
        out: &mut Vec<Bound<'py, PyAny>>,
    ) -> ERes<()> {
        if args.len() < 2 {
            return Err(nope("bad filter block"));
        }
        let body = match args.first() {
            Some(Av::List(b)) => b,
            _ => return Err(nope("bad filter block body")),
        };
        if matches!(&args[1], Av::NoneV) {
            return Err(nope("filter block without filter"));
        }
        // analyze_node order first (the filter expression), body afterwards;
        // later occurrences never override an existing slot decision.
        let mut occs: Vec<Occ> = Vec::new();
        self.walk_fsf(&args[1], &mut occs);
        for it in body {
            self.walk_fsf(it, &mut occs);
        }
        let ancestors = self.owned_union();
        let kinds = self.build_kinds(&occs, &ancestors);
        self.enter_scope(&kinds)?;
        let mut buf: Vec<Bound<'py, PyAny>> = Vec::new();
        let res = self.exec_items(body, &mut buf);
        if let Err(e) = res {
            return Err(e);
        }
        let lst = PyList::new(self.py, &buf)?;
        let value = self.env.call_method1("concat", (&lst,))?;
        let applied = self.eval_filter_hole(&args[1], &value)?;
        self.pop_scope();
        out.push(self.out_str(&applied)?);
        Ok(())
    }

    fn eval_filter_hole(&self, av: &Av, hole: &Bound<'py, PyAny>) -> ERes<Bound<'py, PyAny>> {
        match av {
            Av::Node("Filter", _, a) => {
                if matches!(&a[0], Av::NoneV) {
                    self.filter_common(a, true, hole.clone())
                } else {
                    let inner = self.eval_filter_hole(&a[0], hole)?;
                    self.filter_common(a, true, inner)
                }
            }
            _ => self.node(av),
        }
    }

    fn exec_assign_block(&self, args: &[Av]) -> ERes<()> {
        if args.len() < 3 {
            return Err(nope("bad assign block"));
        }
        let body = match &args[2] {
            Av::List(b) => b,
            _ => return Err(nope("bad assign block body")),
        };
        // analyze_node order: target first (RootVisitor.visit_AssignBlock),
        // then the body as it is emitted; the filter expression registers its
        // loads last.
        let mut occs: Vec<Occ> = Vec::new();
        self.walk_fsf(&args[0], &mut occs);
        for it in body {
            self.walk_fsf(it, &mut occs);
        }
        if !matches!(&args[1], Av::NoneV) {
            self.walk_fsf(&args[1], &mut occs);
        }
        let ancestors = self.owned_union();
        let kinds = self.build_kinds(&occs, &ancestors);
        self.enter_scope(&kinds)?;
        let mut buf: Vec<Bound<'py, PyAny>> = Vec::new();
        let res = self.exec_items(body, &mut buf);
        if res.is_err() {
            return Err(res.unwrap_err());
        }
        let lst = PyList::new(self.py, &buf)?;
        let mut value = self.env.call_method1("concat", (&lst,))?;
        if !matches!(&args[1], Av::NoneV) {
            value = self.eval_filter_hole(&args[1], &value)?;
        }
        self.pop_scope();
        self.store_assign(&args[0], value)
    }

    /// Shared loop scope analysis for a `for` node: analyze_node visits the
    /// target (as parameters) and the body; the test expression belongs to a
    /// separate frame (`analyze_node(for_branch="test")`) built by
    /// `test_kinds` -- merging them would let a body store shadow the very
    /// name the test resolves from the context.
    fn loop_kinds(&self, args: &[Av]) -> ERes<(HashMap<String, IK>, bool)> {
        let mut occs: Vec<Occ> = Vec::new();
        self.walk_fsf(&args[0], &mut occs);
        let body = match &args[2] {
            Av::List(b) => b,
            _ => return Err(nope("bad for body")),
        };
        for it in body {
            self.walk_fsf(it, &mut occs);
        }
        let ancestors = self.owned_union();
        let mut kinds = self.build_kinds(&occs, &ancestors);
        let extended = args.len() > 5 && self.deep_name_load_in(body, "loop");
        if extended {
            kinds.insert("loop".to_string(), IK::Param);
        }
        Ok((kinds, extended))
    }

    /// Symbols of the test-filter frame: target parameters plus whatever the
    /// test expression loads; must be computed before the loop frame is on
    /// the stack so that loads resolve from context and never see a body
    /// store of the same name.
    fn test_kinds(&self, args: &[Av]) -> ERes<HashMap<String, IK>> {
        let mut occs: Vec<Occ> = Vec::new();
        self.walk_fsf(&args[0], &mut occs);
        self.walk_fsf(&args[4], &mut occs);
        let ancestors = self.owned_union();
        Ok(self.build_kinds(&occs, &ancestors))
    }

    fn exec_for(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        if args.len() < 6 {
            return Err(nope("bad for node"));
        }
        let recursive = match &args[5] {
            Av::Bool(b) => *b,
            _ => return Err(nope("bad recursive flag")),
        };
        if recursive && !matches!(&args[4], Av::NoneV) {
            // The test filter wraps the reciter at every recursion level;
            // deferred until that generator path is implemented.
            return Err(nope("recursive loop with test filter"));
        }
        if recursive {
            let v = self.node(&args[1])?;
            let res = self.run_recursive(args, v, 0)?;
            let s = self.out_str(&res)?;
            out.push(s);
            return Ok(());
        }
        let (kinds, extended) = self.loop_kinds(args)?;
        let has_test = !matches!(&args[4], Av::NoneV);
        if extended && has_test {
            // LoopContext would need to wrap the lazily filtered generator;
            // eager filtering could reorder test vs body errors. Defer.
            return Err(nope("extended loop with test filter"));
        }
        let iter_val = self.node(&args[1])?;
        let else_list = match &args[3] {
            Av::List(l) => l,
            _ => return Err(nope("bad for else")),
        };
        let mut ran = false;
        if extended {
            let none = self.py.None();
            let lc = self.loopctx.call1((&iter_val, &self.undef, none, 0i64))?;
            let mut iterated = lc.try_iter()?;
            while let Some(pair_r) = iterated.next() {
                let (val, loopobj): (Bound<'py, PyAny>, Bound<'py, PyAny>) = pair_r?.extract()?;
                self.enter_scope(&kinds)?;
                self.store_target(&args[0], &val, false)?;
                {
                    if let Some(frame) = self.stack.borrow_mut().last_mut() {
                        frame.slots.insert("loop".to_string(), Some(loopobj));
                    }
                }
                self.exec_items(&body_of(args), out)?;
                self.pop_scope();
                ran = true;
            }
        } else if has_test {
            // Lazy filtering: the test runs in its own frame (the generated
            // filter function), so its loads resolve from context and never
            // observe a store of the same name made by the loop body.
            let kt = self.test_kinds(args)?;
            let mut it = iter_val.try_iter()?;
            while let Some(item_r) = it.next() {
                let item: Bound<'py, PyAny> = item_r?;
                self.enter_scope(&kt)?;
                let pass = match self.store_target(&args[0], &item, false) {
                    Ok(()) => {
                        let t = self.node(&args[4])?;
                        t.is_truthy()?
                    }
                    Err(e) => {
                        return Err(e);
                    }
                };
                self.pop_scope();
                if pass {
                    self.enter_scope(&kinds)?;
                    self.store_target(&args[0], &item, false)?;
                    let res = self.exec_items(&body_of(args), out);
                    self.pop_scope();
                    if res.is_err() {
                        return Err(res.unwrap_err());
                    }
                    ran = true;
                }
            }
        } else {
            let mut it = iter_val.try_iter()?;
            while let Some(item_r) = it.next() {
                let item: Bound<'py, PyAny> = item_r?;
                self.enter_scope(&kinds)?;
                self.store_target(&args[0], &item, false)?;
                self.exec_items(&body_of(args), out)?;
                self.pop_scope();
                ran = true;
            }
        }
        if !else_list.is_empty() && !ran {
            let mut occs: Vec<Occ> = Vec::new();
            for it in else_list.iter() {
                self.walk_fsf(it, &mut occs);
            }
            let ancestors = self.owned_union();
            let kinds = self.build_kinds(&occs, &ancestors);
            self.enter_scope(&kinds)?;
            self.exec_items(else_list, out)?;
            self.pop_scope();
        }
        Ok(())
    }

    fn run_recursive(
        &self,
        args: &[Av],
        iterable: Bound<'py, PyAny>,
        depth0: i64,
    ) -> ERes<Bound<'py, PyAny>> {
        let (kinds, _extended) = self.loop_kinds(args)?;
        let none = self.py.None();
        let lc = self.loopctx.call1((&iterable, &self.undef, none, depth0))?;
        let lptr = lc.as_ptr() as usize;
        self.rec.borrow_mut().push(RecEntry {
            ptr: lptr,
            args: args.to_vec(),
        });
        let body = body_of(args);
        let mut buf: Vec<Bound<'py, PyAny>> = Vec::new();
        let mut ran = false;
        let mut iterated = lc.try_iter()?;
        while let Some(pair_r) = iterated.next() {
            let (val, loopobj): (Bound<'py, PyAny>, Bound<'py, PyAny>) = pair_r?.extract()?;
            self.enter_scope(&kinds)?;
            self.store_target(&args[0], &val, false)?;
            {
                if let Some(frame) = self.stack.borrow_mut().last_mut() {
                    frame.slots.insert("loop".to_string(), Some(loopobj));
                }
            }
            let res = self.exec_items(body, &mut buf);
            self.pop_scope();
            if res.is_err() {
                self.unregister_lc(lptr);
                return Err(res.unwrap_err());
            }
            ran = true;
        }
        let else_list = match &args[3] {
            Av::List(l) => l,
            _ => return Err(nope("bad for else")),
        };
        if !else_list.is_empty() && !ran {
            let mut occs: Vec<Occ> = Vec::new();
            for it in else_list.iter() {
                self.walk_fsf(it, &mut occs);
            }
            let ancestors = self.owned_union();
            let kinds = self.build_kinds(&occs, &ancestors);
            self.enter_scope(&kinds)?;
            let res = self.exec_items(else_list, &mut buf);
            self.pop_scope();
            if res.is_err() {
                self.unregister_lc(lptr);
                return Err(res.unwrap_err());
            }
        }
        self.unregister_lc(lptr);
        let lst = PyList::new(self.py, &buf)?;
        Ok(self.env.call_method1("concat", (&lst,))?)
    }

    /// Pre-evaluation scan mirroring the python code generator's compile-time
    /// checks over the *whole* tree: nodes outside the evaluated subset (even
    /// in never-executed positions), missing filter/test names (the compiler
    /// fails on those unless they sit in a soft if/condexpr frame, and a
    /// fallback reproduces either behaviour exactly), and assignment to the
    /// special `loop` variable inside any `for` subtree.
    fn scan(&self, av: &Av) -> Result<(), EvalErr> {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.scan(a)?;
                }
            }
            Av::Node(name, _, args) => {
                if !SUPPORTED_NODES.contains(name) {
                    return Err(nope(*name));
                }
                if *name == "Filter" || *name == "Test" {
                    if args.len() >= 6 {
                        if let Some(nm) = arg_str(&args[1]) {
                            let is_filter = *name == "Filter";
                            let map = if is_filter { &self.filters } else { &self.tests };
                            if map.get_item(nm)?.is_none() {
                                let kind = if is_filter { "filter" } else { "test" };
                                return Err(nope(format!("missing {} {:?}", kind, nm)));
                            }
                        }
                    }
                }
                if *name == "For" {
                    if let Some(l) = self.find_loop_store(av) {
                        return Err(EvalErr::Exc(
                            self.assertion_error(
                                "Can't assign to special loop variable in for-loop target",
                                Some(l),
                            )?,
                        ));
                    }
                }
                for a in args {
                    self.scan(a)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn find_loop_store(&self, av: &Av) -> Option<u32> {
        match av {
            Av::Node("Name", lineno, a) => {
                if a.len() >= 2
                    && arg_str(&a[0]) == Some("loop")
                    && arg_str(&a[1]) == Some("store")
                {
                    *lineno
                } else {
                    None
                }
            }
            Av::List(inner) | Av::Tuple(inner) => {
                inner.iter().find_map(|x| self.find_loop_store(x))
            }
            Av::Node(_, _, a) => a.iter().find_map(|x| self.find_loop_store(x)),
            _ => None,
        }
    }

    fn assertion_error(&self, msg: &str, lineno: Option<u32>) -> Result<PyErr, EvalErr> {
        let exceptions = self.py.import("jutsu.exceptions")?;
        let cls = exceptions.getattr("TemplateAssertionError")?;
        let inst = cls.call(
            (
                msg,
                lineno.map(|l| l as i64),
                self.name.clone(),
                None::<String>,
            ),
            None,
        )?;
        inst.setattr("translated", true)?;
        Ok(PyErr::from_value(inst))
    }

    fn unregister_lc(&self, ptr: usize) {
        let mut r = self.rec.borrow_mut();
        if let Some(i) = r.iter().position(|e| e.ptr == ptr) {
            r.swap_remove(i);
        }
    }

    fn rec_find(&self, func: &Bound<'py, PyAny>) -> Option<Vec<Av>> {
        let r = self.rec.borrow();
        let fptr = func.as_ptr() as usize;
        for e in r.iter().rev() {
            if e.ptr == fptr {
                return Some(e.args.clone());
            }
        }
        None
    }

    fn exec_with(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        if args.len() < 3 {
            return Err(nope("bad with node"));
        }
        let (targets, values) = match (&args[0], &args[1]) {
            (Av::List(t), Av::List(v)) => (t, v),
            _ => return Err(nope("bad with targets")),
        };
        let body = match &args[2] {
            Av::List(b) => b,
            _ => return Err(nope("bad with body")),
        };
        let mut occs: Vec<Occ> = Vec::new();
        for t in targets.iter() {
            self.walk_fsf(t, &mut occs);
        }
        for it in body.iter() {
            self.walk_fsf(it, &mut occs);
        }
        let ancestors = self.owned_union();
        let kinds = self.build_kinds(&occs, &ancestors);
        // values evaluate against the outer scopes: the code generator visits
        // each expression with the parent frame, not the with frame.
        let mut pairs: Vec<(usize, Bound<'py, PyAny>)> = Vec::new();
        for (i, v) in values.iter().enumerate() {
            if i >= targets.len() {
                break;
            }
            pairs.push((i, self.node(v)?));
        }
        self.enter_scope(&kinds)?;
        let mut res: ERes<()> = Ok(());
        for (i, val) in &pairs {
            if let Err(e) = self.store_target(&targets[*i], val, false) {
                res = Err(e);
                break;
            }
        }
        if res.is_ok() {
            res = self.exec_items(body, out);
        }
        self.pop_scope();
        res
    }
}

fn body_of(args: &[Av]) -> &[Av] {
    match &args[2] {
        Av::List(b) => b,
        _ => &[],
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
    match ev.scan(av) {
        Ok(()) => {}
        Err(EvalErr::Unsupported(u)) => return EvalOutcome::Unsupported(u),
        Err(EvalErr::Exc(e)) => return EvalOutcome::Exc(e),
    }
    let mut out: Vec<Bound<'py, PyAny>> = Vec::new();
    match av {
        Av::Node("Template", _, a) => {
            let res = ev.exec_template(a, &mut out);
            match res {
                Ok(()) => match ev.out_join(out) {
                    Ok(v) => EvalOutcome::Rendered(v),
                    Err(EvalErr::Exc(e)) => EvalOutcome::Exc(e),
                    Err(EvalErr::Unsupported(u)) => EvalOutcome::Unsupported(u),
                },
                Err(EvalErr::Exc(e)) => EvalOutcome::Exc(e),
                Err(EvalErr::Unsupported(u)) => EvalOutcome::Unsupported(u),
            }
        }
        _ => EvalOutcome::Unsupported("not a template".to_string()),
    }
}
