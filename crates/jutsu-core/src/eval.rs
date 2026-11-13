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
use std::rc::Rc;
use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::parser::{Av, PToken, RtParser};
use crate::pyrepr::py_repr_str;

pyo3::import_exception!(jutsu.exceptions, TemplateRuntimeError);
pyo3::import_exception!(jutsu.exceptions, TemplateNotFound);

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
    s: Rc<Session>,
    /// Interpreter-local scope frames, innermost last.  Frames are shared by
    /// `Rc` so closures captured at a macro definition keep seeing the live
    /// slot objects even after this interpreter instance has finished.
    stack: RefCell<Vec<Rc<RefCell<Scope>>>>,
    /// Root executions currently in progress (innermost last), mirroring the
    /// generated `parent_template` local of every root render function.
    ext_stack: RefCell<Vec<Rc<RootExec>>>,
    /// The name of the template whose generated code is executing right here:
    /// `CodeGenerator.name`, used for the loader parent argument and for the
    /// positions embedded into runtime messages.  It follows the module a
    /// closure was compiled into (macro / caller / block functions capture
    /// theirs) and switches to an extends parent while that parent renders --
    /// python compiles one generator per template module, so includes reached
    /// from a parent body resolve relative names against the parent
    /// (slice C-v5).
    tname: RefCell<Option<String>>,
    /// Code-generator flags tracked dynamically over the executed traversal of
    /// a root region (`extends_so_far`, `has_known_extends`, `frame.toplevel`
    /// and `frame.rootlevel`); see slice C-v4 in PLAN_INTERP.md.
    flags: RefCell<BlkFlags>,
    /// Stores never reach back into `context.vars`: true for interpreters that
    /// run a compiled block function body (stores land in `_block_vars`), where
    /// only the local slot chain is written.
    local_only: bool,
    /// The python `Context` this interpreter resolves against and derives
    /// scoped contexts from.  It is the render context for the root region,
    /// but a compiled block function runs with *whatever object its caller
    /// passed* -- a derived one when reached through a scoped invocation or
    /// a `super()` reference taken on such an object (slice C-v4b).  Macro
    /// closures resolve against the context current at their definition,
    /// mirroring the generated code's closure over the nearest enclosing
    /// function's `context` variable.
    ctxobj: Py<PyAny>,
    /// Compile-time evaluation-context state of the region this interpreter
    /// currently runs (slice C-v6): mirrors the code generator's shared
    /// `EvalContext` object while it visits -- `ae` is its autoescape value,
    /// `vol` the volatile flag set by `{% autoescape %}` tags whose option
    /// expression cannot be folded at compile time.  It decides which
    /// constant folding python performed ahead of time (and therefore which
    /// wrappers and joins the generated code uses), independently of the
    /// *live* eval context attribute that runtime checks consult.
    aestate: RefCell<AeState>,
}

/// The inheritance bookkeeping of the code generator, mirrored at runtime.
/// Every executed `{% extends %}` marks `seen_any`; one located directly in a
/// root body additionally marks `known_top` (the generator's
/// `has_known_extends`, which drops all later top-level output and block
/// invocations).  `chain_top`/`chain_root` mirror the frame flags: soft frames
/// from `if` preserve them, every other construct resets.
#[derive(Clone, Copy)]
pub struct BlkFlags {
    seen_any: bool,
    known_top: bool,
    chain_top: bool,
    chain_root: bool,
}

impl Default for BlkFlags {
    fn default() -> Self {
        BlkFlags { seen_any: false, known_top: false, chain_top: false, chain_root: false }
    }
}

/// The code generator's compile-time eval-context state over a region,
/// mirrored at runtime (slice C-v6).  `ae` is the autoescape attribute value
/// as seen during compilation; `vol` the volatile flag an `{% autoescape %}`
/// tag sets when its option expression cannot be folded.  Generated wrappers
/// consult *this* state for everything but the explicit runtime checks
/// (`context.eval_ctx.autoescape`) that only exist inside volatile regions,
/// where the compile-time attribute keeps holding the enclosing value.
#[derive(Clone, Copy, PartialEq)]
pub struct AeState {
    pub vol: bool,
    pub ae: bool,
}

/// One runtime scope frame: slots by name; `None` is the `missing` sentinel.
/// Frames live in `Rc`s so that macro closures (slice C-v3b) can capture a
/// defining frame chain whose slot objects stay alive -- and writable --
/// after the render step that created them has ended.
pub struct Scope {
    slots: HashMap<String, Option<Py<PyAny>>>,
    /// True for the frames whose compiled counterpart is a generated function's
    /// top frame (`Frame(eval_ctx)` with no symbol parent): template roots and
    /// block functions.  A scoped invocation dumps stores along the symbol
    /// chain only -- never across such a boundary into whatever happened to be
    /// live below (slice C-v4b).
    boundary: bool,
}

/// Shared evaluator session.  Everything except the per-interpreter scope
/// stack lives here in owned form so it can be captured by python-side
/// closures (the `runtime.Macro` wrapper holds a rust callable object that
/// outlives individual FFI calls).  Invariant: no borrow of a `RefCell`
/// field may be held across a python call; helpers below collect values
/// first and construct undefined objects only after dropping borrows.
pub struct Session {
    env: Py<PyAny>,
    evalctx: Py<PyAny>,
    rt: Py<PyAny>,
    filters: Py<PyDict>,
    tests: Py<PyDict>,
    slice_cls: Py<PyAny>,
    /// The `runtime.missing` sentinel.
    missing: Py<PyAny>,
    /// `environment.undefined`, the undefined factory used at load sites.
    undef: Py<PyAny>,
    /// `runtime.LoopContext` class.
    loopctx: Py<PyAny>,
    /// `runtime.Namespace` class.
    ns: Py<PyAny>,
    autoescape: bool,
    name: Option<String>,
    /// Loop contexts created for recursive loops, awaiting recursive calls.
    rec: RefCell<Vec<RecEntry>>,
    /// Next block function id (slice C-v4).  A block body needs to hand its own
    /// python callable object to `Context.super`, whose index lookup compares
    /// identities; the side table lets a function find *itself* without a
    /// recursive rust handle.
    next_blk: RefCell<usize>,
    blocks_reg: RefCell<HashMap<usize, Py<PyAny>>>,
    /// Extra template sources for `{% extends %}`, already wrapped into token
    /// form; a name mapped to `None` marks a source that cannot even be lexed.
    srcs: std::collections::HashMap<String, Option<Vec<PToken>>>,
    /// Parsed and prepared units by template name (slice C-v4).
    units: RefCell<HashMap<String, Rc<UnitInfo>>>,
    /// Warm-path cache of the per-render compile-time analyses (scan verdict,
    /// layout check, root frame slot kinds, uses_self) keyed by tree pointer.
    /// The parsed AST is immutable and shared through `ParsedAst` handles, so
    /// a fixed tree pointer identifies one template source; the cached values
    /// depend only on the tree plus the environment's static autoescape flag,
    /// which is re-checked on every hit.  A miss recomputes everything exactly
    /// as before and stores the result.
    unit_prep: RefCell<HashMap<*const Av, UnitPrep>>,
    /// Stable runtime functions cached at session build so every use reuses
    /// the same object instead of a per-node pyo3 attribute lookup
    /// (performance pass part 2).  Module-level objects never change during a
    /// render, so this is byte-identical to fetching them on demand.
    esc: Py<PyAny>,
    markup_cls: Py<PyAny>,
    str_join: Py<PyAny>,
    markup_join: Py<PyAny>,
    /// Static cache for stable runtime constants keyed by rt module pointer
    rt_constants_cache: Mutex<HashMap<usize, Rc<RuntimeConstants>>>,
}

/// Static runtime constants that are cached once per rt module to avoid repeated pyo3 attribute lookups
struct RuntimeConstants {
    missing: Py<PyAny>,
    loopctx: Py<PyAny>,
    ns: Py<PyAny>,
    esc: Py<PyAny>,
    markup_cls: Py<PyAny>,
    str_join: Py<PyAny>,
    markup_join: Py<PyAny>,
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

/// Runtime state of one template root execution, mirroring the generated
/// `parent_template` local inside each template's root render function.
pub struct RootExec {
    /// Whether this template contains an `{% extends %}` anywhere at all (the
    /// code generator's `have_extends`, which puts a runtime check in front of
    /// every top-level output).
    any_extends: bool,
    /// Whether the parent has been loaded already (`parent_template is not None`).
    extended: RefCell<bool>,
    /// The prepared parent unit, to be rendered after this body finishes.
    parent: RefCell<Option<Rc<UnitInfo>>>,
}

/// The once-per-tree results of the pre-execution analyses that used to run
/// on every render: the scan/layout verdicts, the root frame's slot kinds and
/// whether an undeclared `self` load needs a param slot.  All fields depend
/// only on the immutable tree plus the environment's static autoescape flag.
#[derive(Clone)]
struct UnitPrep {
    autoescape: bool,
    /// Scan passed (otherwise the stored reason is re-reported).
    scan_ok: bool,
    scan_reason: Option<String>,
    layout_ok: bool,
    layout_reason: Option<String>,
    root_kinds: Vec<(String, IK)>,
    uses_self: bool,
}

/// A parsed and prepared template unit (the interpreter's equivalent of a
/// compiled template module: its tree plus the block functions it registers).
pub struct UnitInfo {
    /// The parsed `Template` node.
    tree: Av,
    /// The name this template was loaded under, i.e. the `CodeGenerator.name`
    /// of its module (slice C-v5).
    name: Option<String>,
    /// Block names in document order (`find_all(nodes.Block)`).  Nested blocks
    /// belong to their own template's registry exactly like codegen does.
    names: Vec<String>,
    /// One callable per collected block, same order as `names`.
    funcs: Vec<Py<PyAny>>,
    /// Whether this template mentions `{% extends %}` anywhere.
    any_extends: bool,
}


/// Node names the evaluator understands (expressions plus C-v2 statements).
pub const SUPPORTED_NODES: &[&str] = &[
    "Const", "TemplateData", "Name", "Getattr", "Getitem", "Tuple", "List", "Dict",
    "Pair", "CondExpr", "Or", "And", "Not", "Compare", "Operand", "Concat", "Call",
    "Filter", "Test", "Add", "Sub", "Mul", "Div", "FloorDiv", "Mod", "Pow", "Pos",
    "Neg", "Slice", "Keyword", "Output", "If", "Assign", "AssignBlock", "FilterBlock",
    "For", "With", "NSRef", "Template", "Macro", "CallBlock", "Extends", "Block",
    "Include", "Import", "FromImport", "Scope", "ScopedEvalContextModifier",
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

/// Compile-time-equivalent analysis of a macro / call block body, done once
/// at define-execution time (the compiled engine does it during codegen).
#[derive(Clone)]
struct MacroPlan {
    kinds: HashMap<String, IK>,
    params: Vec<(String, Option<Av>)>,
    accesses_caller: bool,
    caller_implicit: bool,
    accesses_kwargs: bool,
    accesses_varargs: bool,
}

/// Rust callable backing a `runtime.Macro` object (slice C-v3b).  Holds the
/// defining frame chain by `Rc` so closure cells stay alive and writable
/// after the render step that defined them ended, plus everything needed to
/// enter the macro's own scope frame on every invocation.
#[pyclass(unsendable)]
pub struct MacroFunc {
    session: Rc<Session>,
    /// The template name current at the definition site (`CodeGenerator.name`
    /// of the module this macro was compiled into; slice C-v5).
    tname: Option<String>,
    /// The python context current at the definition site.  Generated code
    /// closes over the nearest enclosing function's `context` variable, so a
    /// macro defined inside a scoped block invocation resolves through that
    /// derived object for its whole life, not whatever caller passes later.
    ctxobj: Py<PyAny>,
    chain: Vec<Rc<RefCell<Scope>>>,
    kinds: HashMap<String, IK>,
    params: Vec<(String, Option<Av>)>,
    caller_slot: bool,
    kwargs_slot: bool,
    varargs_slot: bool,
    body: Vec<Av>,
    /// Compile-time eval-context state at the definition site (slice C-v6):
    /// python compiles a macro function inline where its node is visited, so
    /// the wrappers inside it follow that region's flags, not the caller's.
    aestate: AeState,
}

#[pymethods]
impl MacroFunc {
    /// Called from `runtime.Macro._invoke` with the argument list built by
    /// python's own `Macro.__call__`: positional fills for every declared
    /// parameter (`missing` where not provided), then the appended implicit
    /// specials in codegen order -- caller, kwargs dict, varargs tuple.  The
    /// layout is guaranteed to match the flags captured at define time.
    #[pyo3(signature = (* args))]
    fn __call__(&self, args: &Bound<'_, PyTuple>) -> PyResult<Py<PyAny>> {
        let py = args.py();
        let mut values: Vec<Py<PyAny>> = Vec::with_capacity(args.len());
        for a in args.iter() {
            values.push(a.unbind());
        }
        into_pyres(self.invoke(py, &values))
    }
}

impl MacroFunc {
    fn invoke(&self, py: Python<'_>, values: &[Py<PyAny>]) -> Result<Py<PyAny>, EvalErr> {
        let ev = Ev::with_chain(
            py,
            self.session.clone(),
            self.chain.clone(),
            self.ctxobj.clone(),
            self.tname.clone(),
            self.aestate,
        );
        ev.enter_scope(&self.kinds)?;
        let miss = ev.missing();
        for (i, (pname, def)) in self.params.iter().enumerate() {
            // a `missing` slot value means "parameter was not provided"
            let provided: Option<Bound<'_, PyAny>> = match values.get(i) {
                Some(v) => {
                    let b = v.bind(py).clone();
                    if b.is(&miss) {
                        None
                    } else {
                        Some(b)
                    }
                }
                None => None,
            };
            let to_store: Bound<'_, PyAny> = match provided {
                Some(v) => v,
                None => match def {
                    Some(dav) => ev.node(dav)?,
                    None => {
                        // python's `!r` of an identifier is always single-quoted
                        let hint = format!("parameter '{pname}' was not provided");
                        let kw = PyDict::new(py);
                        kw.set_item("name", pname.as_str())?;
                        ev.undef().call((&hint,), Some(&kw))?
                    }
                },
            };
            ev.set_top_slot(pname, &to_store)?;
        }
        let mut pos = self.params.len();
        for (slot_name, wanted) in [
            ("caller", self.caller_slot),
            ("kwargs", self.kwargs_slot),
            ("varargs", self.varargs_slot),
        ] {
            if wanted {
                match values.get(pos) {
                    Some(v) => ev.set_top_slot_raw(slot_name, v.clone())?,
                    None => return Err(nope(format!("missing {slot_name} argument"))),
                }
                pos += 1;
            }
        }
        let mut buf: Vec<Bound<'_, PyAny>> = Vec::new();
        ev.exec_items(&self.body, &mut buf)?;
        let lst = PyList::new(py, &buf)?;
        let joined = ev.env().call_method1("concat", (&lst,))?;
        ev.pop_scope();
        Ok(joined.unbind())
    }
}

impl<'py> Ev<'py> {
    fn set_top_slot_raw(&self, name: &str, v: Py<PyAny>) -> ERes<()> {
        let top = self.stack.borrow().last().cloned();
        match top {
            Some(frame) => {
                frame
                    .borrow_mut()
                    .slots
                    .insert(name.to_string(), Some(v));
                Ok(())
            }
            None => Err(nope("no scope frame to bind")),
        }
    }
}

/// `keyword.kwlist` of the target python runtime; mirrors the
/// `is_python_keyword` check that decides the code generator's dict-literal
/// workaround for keyword arguments.
const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break",
    "class", "continue", "def", "del", "elif", "else", "except", "finally",
    "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal",
    "not", "or", "pass", "raise", "return", "try", "while", "with", "yield",
];

fn is_python_keyword(name: Option<&str>) -> bool {
    match name {
        Some(n) => PY_KEYWORDS.iter().any(|k| *k == n),
        None => false,
    }
}

/// The key of a `Keyword` node (field layout: [key, value]).
fn kw_key(av: &Av) -> Option<&str> {
    match av {
        Av::Node("Keyword", _, a) if !a.is_empty() => arg_str(&a[0]),
        _ => None,
    }
}

/// Name of a signature parameter node (`parse_signature` only allows simple
/// name targets).  Field layout: [name, ctx].
fn param_name(av: &Av) -> Option<&str> {
    match av {
        Av::Node("Name", _, a) if !a.is_empty() => arg_str(&a[0]),
        _ => None,
    }
}

fn into_pyres<T>(r: Result<T, EvalErr>) -> PyResult<T> {
    match r {
        Ok(v) => Ok(v),
        Err(EvalErr::Exc(e)) => Err(e),
        Err(EvalErr::Unsupported(u)) => Err(pyo3::exceptions::PyRuntimeError::new_err(
            format!("unsupported inside a macro body: {u}"),
        )),
    }
}

impl Session {
    fn build(
        py: Python<'_>,
        ctx: &Bound<'_, PyAny>,
        rt: &Bound<'_, PyAny>,
        name: Option<String>,
        srcs: std::collections::HashMap<String, Option<Vec<PToken>>>,
    ) -> Result<Rc<Session>, EvalErr> {
        let env = ctx.getattr("environment")?;
        let evalctx = ctx.getattr("eval_ctx")?;
        let volatile: bool = evalctx.getattr("volatile")?.extract()?;
        if volatile {
            return Err(nope("volatile eval context"));
        }
        // A callable autoescape policy resolves per template name in
        // `EvalContext.__init__`; with inheritance each module compiles under
        // its own name while the runtime context carries another one, so the
        // two decisions can disagree.  Only plain boolean policies are served.
        if !env.getattr("autoescape")?.is_instance_of::<pyo3::types::PyBool>() {
            return Err(nope("non-boolean autoescape policy"));
        }
        // The evaluator calls `environment.getattr/getitem/call` directly,
        // bypassing the sandbox's intercepting machinery (which also gates
        // constant folding through `intercepted_binops`).
        if env.getattr("sandboxed")?.is_truthy()? {
            return Err(nope("sandboxed environment"));
        }
        let autoescape: bool = evalctx.getattr("autoescape")?.extract()?;
        // an async-enabled environment awaits coroutines and consumes async
        // iterables even in the synchronous `render` path; driving that from
        // rust means porting the whole async runtime, so fall back.
        if env.getattr("is_async")?.extract::<bool>()? {
            return Err(nope("async environment"));
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
        // Stable runtime functions cached once per session (performance pass
        // part 2): reusing them avoids a pyo3 attribute lookup on every use.
        let esc = rt.getattr("escape")?;
        let markup_cls = rt.getattr("Markup")?;
        let str_join = rt.getattr("str_join")?;
        let markup_join = rt.getattr("markup_join")?;
        Ok(Rc::new(Session {
            env: env.unbind(),
            evalctx: evalctx.unbind(),
            rt: rt.clone().unbind(),
            filters: filters.unbind(),
            tests: tests.unbind(),
            slice_cls: slice_cls.unbind(),
            missing: missing.unbind(),
            undef: undef.unbind(),
            loopctx: loopctx.unbind(),
            ns: ns.unbind(),
            esc: esc.unbind(),
            markup_cls: markup_cls.unbind(),
            str_join: str_join.unbind(),
            markup_join: markup_join.unbind(),
            autoescape,
            name,
            rec: RefCell::new(Vec::new()),
            next_blk: RefCell::new(0),
            blocks_reg: RefCell::new(HashMap::new()),
            srcs,
            units: RefCell::new(HashMap::new()),
            unit_prep: RefCell::new(HashMap::new()),
            rt_constants_cache: Mutex::new(HashMap::new()),
        }))
    }
}

impl<'py> Ev<'py> {
    fn new(
        py: Python<'py>,
        ctx: Bound<'py, PyAny>,
        rt: Bound<'py, PyAny>,
        name: Option<String>,
        srcs: std::collections::HashMap<String, Option<Vec<PToken>>>,
    ) -> Result<Ev<'py>, EvalErr> {
        let s = Session::build(py, &ctx, &rt, name.clone(), srcs)?;
        let st = AeState { vol: false, ae: s.autoescape };
        Ok(Ev {
            py,
            s,
            stack: RefCell::new(Vec::new()),
            ext_stack: RefCell::new(Vec::new()),
            tname: RefCell::new(name),
            flags: RefCell::new(BlkFlags::default()),
            local_only: false,
            ctxobj: ctx.unbind(),
            aestate: RefCell::new(st),
        })
    }

    /// Interpreter over a captured session and defining frame chain: used by
    /// macro / caller closures (slice C-v3b) whose bodies evaluate against
    /// where they were defined, never the caller's live stack.  Block functions
    /// (slice C-v4) use the same mechanism with an empty chain: compiled block
    /// functions are module-level and inherit nothing from their caller.
    #[allow(dead_code)]
    pub fn with_chain(
        py: Python<'py>,
        s: Rc<Session>,
        chain: Vec<Rc<RefCell<Scope>>>,
        ctxobj: Py<PyAny>,
        tname: Option<String>,
        aestate: AeState,
    ) -> Ev<'py> {
        Ev {
            py,
            s,
            stack: RefCell::new(chain),
            ext_stack: RefCell::new(Vec::new()),
            tname: RefCell::new(tname),
            flags: RefCell::new(BlkFlags::default()),
            local_only: false,
            ctxobj,
            aestate: RefCell::new(aestate),
        }
    }

    /// Mapping lookup that treats a missing key as python's `None` instead of
    /// raising (generic `get_item` on a non-dict mapping raises KeyError).
    fn dict_get(&self, d: &Bound<'py, PyAny>, key: &str) -> Result<Bound<'py, PyAny>, PyErr> {
        match d.get_item(key) {
            Ok(v) => Ok(v),
            Err(e) if e.is_instance_of::<pyo3::exceptions::PyKeyError>(self.py) => Ok(self.none()),
            Err(e) => Err(e),
        }
    }

    /// Child frames from loops, with / filter / set blocks lose both frame flags.
    fn enter_body_flags(&self) {
        let mut nf = *self.flags.borrow();
        nf.chain_top = false;
        nf.chain_root = false;
        *self.flags.borrow_mut() = nf;
    }

    /// Soft frames from an `if` keep toplevel but lose rootlevel.
    fn enter_if_flags(&self) {
        let mut nf = *self.flags.borrow();
        nf.chain_root = false;
        *self.flags.borrow_mut() = nf;
    }

    /// Interpreter for a compiled block function body: no defining chain (the
    /// generated functions are module level), and stores stay local (`_block_vars`).
    fn block_ev(
        py: Python<'py>,
        s: Rc<Session>,
        ctxobj: Py<PyAny>,
        tname: Option<String>,
        aestate: AeState,
    ) -> Ev<'py> {
        Ev {
            py,
            s,
            stack: RefCell::new(Vec::new()),
            ext_stack: RefCell::new(Vec::new()),
            tname: RefCell::new(tname),
            flags: RefCell::new(BlkFlags::default()),
            local_only: true,
            ctxobj,
            aestate: RefCell::new(aestate),
        }
    }

    fn env(&self) -> Bound<'py, PyAny> {
        self.s.env.bind(self.py).clone()
    }
    fn evalctx(&self) -> Bound<'py, PyAny> {
        self.s.evalctx.bind(self.py).clone()
    }
    fn ctx(&self) -> Bound<'py, PyAny> {
        self.ctxobj.bind(self.py).clone()
    }
    fn rt(&self) -> Bound<'py, PyAny> {
        self.s.rt.bind(self.py).clone()
    }

    /// The template name the session was started with (the top-level one).
    fn sname(&self) -> Option<String> {
        self.s.name.clone()
    }

    /// The current module's template name (slice C-v5): what python's
    /// per-module `CodeGenerator` would answer through `self.name`.
    fn cur_name(&self) -> Option<String> {
        self.tname.borrow().clone()
    }
    fn filters(&self) -> Bound<'py, PyDict> {
        self.s.filters.bind(self.py).clone()
    }
    fn tests(&self) -> Bound<'py, PyDict> {
        self.s.tests.bind(self.py).clone()
    }
    fn slice_cls(&self) -> Bound<'py, PyAny> {
        self.s.slice_cls.bind(self.py).clone()
    }
    fn missing(&self) -> Bound<'py, PyAny> {
        self.s.missing.bind(self.py).clone()
    }
    fn undef(&self) -> Bound<'py, PyAny> {
        self.s.undef.bind(self.py).clone()
    }
    fn loopctx(&self) -> Bound<'py, PyAny> {
        self.s.loopctx.bind(self.py).clone()
    }
    fn ns(&self) -> Bound<'py, PyAny> {
        self.s.ns.bind(self.py).clone()
    }
    fn esc(&self) -> Bound<'py, PyAny> {
        self.s.esc.bind(self.py).clone()
    }
    fn markup_cls(&self) -> Bound<'py, PyAny> {
        self.s.markup_cls.bind(self.py).clone()
    }
    fn str_join(&self) -> Bound<'py, PyAny> {
        self.s.str_join.bind(self.py).clone()
    }
    fn markup_join(&self) -> Bound<'py, PyAny> {
        self.s.markup_join.bind(self.py).clone()
    }

    /// Raw context lookup: `Context.resolve_or_missing`, `None` for missing.
    fn raw_resolve(&self, name: &str) -> Result<Option<Py<PyAny>>, PyErr> {
        let miss = self.missing();
        let rv = self.ctx().call_method1("resolve_or_missing", (name,))?;
        if rv.is(&miss) {
            Ok(None)
        } else {
            Ok(Some(rv.unbind()))
        }
    }

    /// Wrap a slot value for a load site: missing becomes a fresh undefined
    /// created by the environment's factory, exactly like the generated
    /// `(undefined(name='x') if l_0_x is missing else l_0_x)`.
    fn wrap(&self, name: &str, v: Option<Py<PyAny>>) -> ERes<Bound<'py, PyAny>> {
        match v {
            Some(x) => Ok(x.into_bound(self.py)),
            None => {
                let kw = PyDict::new(self.py);
                kw.set_item("name", name)?;
                Ok(self.undef().call((), Some(&kw))?)
            }
        }
    }

    /// Scope-aware name lookup for load sites: walk the scope chain from the
    /// innermost frame; slots win, otherwise read from the python context.
    fn lookup(&self, name: &str) -> ERes<Bound<'py, PyAny>> {
        #[cfg(feature = "prof")]
        return crate::prof::time("lookup", || self.lookup_inner(name));
        #[cfg(not(feature = "prof"))]
        self.lookup_inner(name)
    }

    fn lookup_inner(&self, name: &str) -> ERes<Bound<'py, PyAny>> {
        let found: Option<Option<Py<PyAny>>> = {
            let frames = self.stack.borrow();
            let mut out: Option<Option<Py<PyAny>>> = None;
            for frame in frames.iter().rev() {
                if let Some(v) = frame.borrow().slots.get(name).cloned() {
                    out = Some(v);
                    break;
                }
            }
            out
        };
        match found {
            Some(v) => self.wrap(name, v),
            None => {
                let raw = self.raw_resolve(name)?;
                self.wrap(name, raw)
            }
        }
    }

    /// Names owned by any live scope (the compiled engine's `find_ref` set).
    fn owned_union(&self) -> Vec<String> {
        let frames = self.stack.borrow();
        let mut out: Vec<String> = Vec::new();
        for frame in frames.iter() {
            for k in frame.borrow().slots.keys() {
                if !out.contains(k) {
                    out.push(k.clone());
                }
            }
        }
        out
    }

    /// Runtime equivalent of codegen's `dump_local_context`: merge the slots
    /// owned by the live scope frames from the top down (inner layers shadow,
    /// like `find_ref`), stopping after the first root-like boundary frame --
    /// inclusive, since a generated function's own frame is the end of its
    /// symbol chain.  Slots still holding `missing` stay out of the mapping;
    /// an owned-but-missing name still shadows outer layers exactly like the
    /// compiled engine's single reference per name (the dict built later by
    /// python's `Context.derived` drops missing values itself).
    fn dump_locals(&self) -> ERes<Bound<'py, PyDict>> {
        let mut pairs: Vec<(String, Py<PyAny>)> = Vec::new();
        {
            let frames = self.stack.borrow();
            let mut seen: Vec<String> = Vec::new();
            for frame in frames.iter().rev() {
                let sc = frame.borrow();
                for name in sc.slots.keys() {
                    if seen.iter().any(|s| s == name) {
                        continue;
                    }
                    seen.push(name.clone());
                    if let Some(Some(v)) = sc.slots.get(name) {
                        pairs.push((name.clone(), v.clone()));
                    }
                }
                if sc.boundary {
                    break;
                }
            }
        }
        let dict = PyDict::new(self.py);
        for (name, v) in pairs {
            dict.set_item(name.as_str(), v.bind(self.py))?;
        }
        Ok(dict)
    }

    /// Value of the nearest owning slot for `name`, walking innermost first.
    fn nearest_slot(&self, name: &str) -> Option<Option<Py<PyAny>>> {
        let frames = self.stack.borrow();
        for frame in frames.iter().rev() {
            if let Some(v) = frame.borrow().slots.get(name).cloned() {
                return Some(v);
            }
        }
        None
    }

    fn none(&self) -> Bound<'py, PyAny> {
        self.py.None().bind(self.py).clone()
    }

    fn lit(&self, av: &Av) -> ERes<Bound<'py, PyAny>> {
        #[cfg(feature = "prof")]
        return crate::prof::time("lit", || self.lit_inner(av));
        #[cfg(not(feature = "prof"))]
        self.lit_inner(av)
    }

    fn lit_inner(&self, av: &Av) -> ERes<Bound<'py, PyAny>> {
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

    /// Truthy value of the *live* eval-context autoescape attribute (the
    /// shared mutable `EvalContext` object; slice C-v6).  Runtime checks in
    /// generated code (`context.eval_ctx.autoescape`) consult this, while the
    /// compile-time state lives in `Ev::aestate`.
    fn live_ae(&self) -> Result<bool, PyErr> {
        Ok(self.evalctx().getattr("autoescape")?.is_truthy()?)
    }

    /// Wrap one runtime-evaluated output expression child: outside volatile
    /// regions the wrapper is fixed by the compile-time flags (`str(...)` or
    /// `escape(...)`); inside them the generated code checks the live
    /// attribute per part.
    fn out_wrap(&self, v: &Bound<'py, PyAny>) -> ERes<Bound<'py, PyAny>> {
        #[cfg(feature = "prof")]
        return crate::prof::time("out-wrap", || self.out_wrap_inner(v));
        #[cfg(not(feature = "prof"))]
        self.out_wrap_inner(v)
    }

    fn out_wrap_inner(&self, v: &Bound<'py, PyAny>) -> ERes<Bound<'py, PyAny>> {
        let st = *self.aestate.borrow();
        if !st.vol {
            return Ok(if st.ae { self.esc().call1((v,))? } else { v.str()?.into_any() });
        }
        if self.live_ae()? {
            Ok(self.esc().call1((v,))?)
        } else {
            Ok(v.str()?.into_any())
        }
    }

    /// Join of a runtime-evaluated `Concat` node.  Only non-volatile frames
    /// with the compile-time autoescape flag use `markup_join`; volatile
    /// regions emit `(markup_join if context.eval_ctx.volatile else
    /// str_join)` and python's eval contexts are never volatile at runtime,
    /// so they always take the plain join -- even when the live attribute is
    /// on (slice C-v6).
    fn ae_join(&self, parts: Vec<Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
        let tup = PyTuple::new(self.py, parts)?;
        let st = *self.aestate.borrow();
        if !st.vol && st.ae {
            let mj = self.markup_join();
            Ok(mj.call1((tup,))?)
        } else {
            let sj = self.str_join();
            Ok(sj.call1((tup,))?)
        }
    }

    /// Fold a subtree to a constant value exactly like the code generator's
    /// compile-time `as_const` machinery: "Impossible" becomes "cannot fold",
    /// and an error raised while folding is not an error here but simply
    /// non-foldability -- generated code re-evaluates such subtrees at render
    /// time anyway, so both engines agree as long as folded values are pure.
    /// `vol`/`ae` carry what python's shared eval-context object would hold
    /// while visiting this position: volatile regions fold nothing (the guards
    /// live inside the individual `as_const` implementations), and autoescape
    /// decides whether template data comes back wrapped in Markup.  There is
    /// deliberately no safe-repr gate anywhere here: at every optimizer site a
    /// rejected inner replacement still lets the parent's own recursive fold
    /// reach the same value through the direct `as_const` calls, and the one
    /// caller without any optimizer (the autoescape tag's option check) gets
    /// precisely that ungated call.
    fn try_fold(
        &self,
        r: std::result::Result<Bound<'py, PyAny>, PyErr>,
    ) -> ERes<Option<Bound<'py, PyAny>>> {
        match r { Ok(v) => Ok(Some(v)), Err(_) => Ok(None) }
    }

    fn fold(
        &self,
        av: &Av,
        vol: bool,
        ae: bool,
    ) -> ERes<Option<Bound<'py, PyAny>>> {
        let (name, args): (&str, &[Av]) = match av {
            Av::Node(n, _, a) => (*n, a),
            _ => return Ok(None),
        };
        match name {
            "Const" => match args.first() { Some(x) => Ok(Some(self.lit(x)?)), None => Ok(None) },
            "TemplateData" => {
                if vol {
                    return Ok(None);
                }
                let raw = match args.first() { Some(x) => self.lit(x)?, None => return Ok(None) };
                if !ae {
                    return Ok(Some(raw));
                }
                self.try_fold(self.markup_cls().call1((&raw,)))
            }
            "Tuple" | "List" => {
                let items = match args.first() { Some(Av::List(l)) => l, _ => return Ok(None) };
                let mut vals: Vec<Bound<'py, PyAny>> = Vec::new();
                for it in items {
                    match self.fold(it, vol, ae)? {
                        Some(v) => vals.push(v),
                        None => return Ok(None),
                    }
                }
                if name == "Tuple" {
                    let t = PyTuple::new(self.py, &vals)?;
                    return Ok(Some(t.into_any()));
                }
                let l = PyList::new(self.py, &vals)?;
                return Ok(Some(l.into_any()));
            }
            "Dict" => {
                let items = match args.first() { Some(Av::List(l)) => l, _ => return Ok(None) };
                let d = PyDict::new(self.py);
                for it in items {
                    let (pn, pa) = match parts(it) {
                        Some(x) => x,
                        None => return Ok(None),
                    };
                    if pn != "Pair" || pa.len() < 2 {
                        return Ok(None);
                    }
                    let k = match self.fold(&pa[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                    let v = match self.fold(&pa[1], vol, ae)? { Some(v) => v, None => return Ok(None) };
                    d.set_item(&k, &v)?;
                }
                return Ok(Some(d.into_any()));
            }
            "CondExpr" => {
                if args.len() < 3 {
                    return Ok(None);
                }
                let t = match self.fold(&args[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                if t.is_truthy()? {
                    return self.fold(&args[1], vol, ae);
                }
                // folding to an implicit undefined is best done at runtime
                if matches!(&args[2], Av::NoneV) {
                    return Ok(None);
                }
                self.fold(&args[2], vol, ae)
            }
            "And" | "Or" => {
                if args.len() < 2 {
                    return Ok(None);
                }
                let l = match self.fold(&args[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                let truthy = l.is_truthy()?;
                if truthy == (name == "Or") {
                    return Ok(Some(l));
                }
                self.fold(&args[1], vol, ae)
            }
            "Not" | "Neg" | "Pos" => {
                if args.is_empty() {
                    return Ok(None);
                }
                let v = match self.fold(&args[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                match name {
                    "Not" => Ok(Some((!v.is_truthy()?).into_bound_py_any(self.py)?)),
                    "Neg" => self.try_fold(v.neg()),
                    _ => self.try_fold(v.pos()),
                }
            }
            "Add" | "Sub" | "Mul" | "Div" | "FloorDiv" | "Pow" | "Mod" => {
                if args.len() < 2 {
                    return Ok(None);
                }
                let a = match self.fold(&args[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                let b = match self.fold(&args[1], vol, ae)? { Some(v) => v, None => return Ok(None) };
                let r: std::result::Result<Bound<'py, PyAny>, PyErr> = match name {
                    "Add" => a.add(&b),
                    "Sub" => a.sub(&b),
                    "Mul" => a.mul(&b),
                    "Div" => a.div(&b),
                    "FloorDiv" => a.floor_div(&b),
                    "Mod" => a.rem(&b),
                    _ => a.pow(&b, self.py.None()),
                };
                self.try_fold(r)
            }
            "Compare" => {
                if args.len() < 2 {
                    return Ok(None);
                }
                match &args[1] {
                    Av::List(ops) => self.fold_cmp(&args[0], ops, vol, ae),
                    _ => Ok(None),
                }
            }
            "Concat" => {
                // python's `Concat.as_const` joins the parts with plain str()
                // -- the documented non-markup-aware fold (`None` becomes 'None').
                let ps = match args.first() { Some(Av::List(l)) => l, _ => return Ok(None) };
                let mut buf = String::new();
                for pt in ps {
                    let v = match self.fold(pt, vol, ae)? { Some(v) => v, None => return Ok(None) };
                    buf.push_str(&v.str()?.extract::<String>()?);
                }
                Ok(Some(PyString::new(self.py, &buf).into_any()))
            }
            "Getattr" | "Getitem" => {
                if args.len() < 2 {
                    return Ok(None);
                }
                // only load-context references fold.
                if args.len() >= 3 && arg_str(&args[2]) != Some("load") {
                    return Ok(None);
                }
                let obj = match self.fold(&args[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
                if name == "Getattr" {
                    let attr = match arg_str(&args[1]) { Some(s) => s, None => return Ok(None) };
                    self.try_fold(self.env().call_method1("getattr", (&obj, attr)))
                } else {
                    let key = match self.fold(&args[1], vol, ae)? { Some(v) => v, None => return Ok(None) };
                    self.try_fold(self.env().call_method1("getitem", (&obj, &key)))
                }
            }
            "Filter" | "Test" => self.fold_filter_test(args, name == "Filter", vol, ae),
            "Slice" => {
                let mut vals: Vec<Bound<'py, PyAny>> = Vec::new();
                for p in args.iter().take(3) {
                    if matches!(p, Av::NoneV) {
                        vals.push(self.none());
                        continue;
                    }
                    match self.fold(p, vol, ae)? {
                        Some(v) => vals.push(v),
                        None => return Ok(None),
                    }
                }
                while vals.len() < 3 {
                    vals.push(self.none());
                }
                let sl = self.slice_cls().call(
                    (vals[0].clone(), vals[1].clone(), vals[2].clone()),
                    None,
                )?;
                Ok(Some(sl))
            }
            _ => Ok(None),
        }
    }

    fn cmp_op(&self, opc: &str, a: &Bound<'py, PyAny>, b: &Bound<'py, PyAny>) -> Option<bool> {
        let r: std::result::Result<bool, PyErr> = match opc {
            "eq" => a.eq(b),
            "ne" => a.ne(b),
            "lt" => a.lt(b),
            "lteq" => a.le(b),
            "gt" => a.gt(b),
            "gteq" => a.ge(b),
            "in" => b.contains(a),
            _ => b.contains(a).map(|v| !v),
        };
        r.ok()
    }

    /// Chained comparison fold with python's exact order: evaluate the next
    /// operand, apply its operator, stop at the first false.
    fn fold_cmp(
        &self,
        expr: &Av,
        ops: &[Av],
        vol: bool,
        ae: bool,
    ) -> ERes<Option<Bound<'py, PyAny>>> {
        let mut cur = match self.fold(expr, vol, ae)? { Some(v) => v, None => return Ok(None) };
        for op in ops.iter() {
            let (on, oa) = match parts(op) {
                Some(x) => x,
                None => return Ok(None),
            };
            if on != "Operand" || oa.len() < 2 {
                return Ok(None);
            }
            let rhs = match self.fold(&oa[1], vol, ae)? { Some(v) => v, None => return Ok(None) };
            let opname = match arg_str(&oa[0]) { Some(s) => s, None => return Ok(None) };
            let ok = match self.cmp_op(opname, &cur, &rhs) {
                Some(v) => v,
                None => return Ok(None),
            };
            if !ok {
                return Ok(Some(false.into_bound_py_any(self.py)?));
            }
            cur = rhs;
        }
        Ok(Some(true.into_bound_py_any(self.py)?))
    }

    /// The full `_FilterTestCommon.as_const` mirror: volatile regions fold
    /// nothing, context-dependent functions never fold, everything else folds
    /// by *calling the function at compile time* exactly like python's code
    /// generator does through its optimizer.
    fn fold_filter_test(
        &self,
        na: &[Av],
        is_filter: bool,
        vol: bool,
        ae: bool,
    ) -> ERes<Option<Bound<'py, PyAny>>> {
        // a filter-block target raises Impossible before even looking at the
        // context (python's `Filter.as_const` override).
        if matches!(&na[0], Av::NoneV) || na.len() < 6 {
            return Ok(None);
        }
        if vol {
            return Ok(None);
        }
        let fname = match arg_str(&na[1]) {
            Some(s) => s,
            None => return Ok(None),
        };
        let map = if is_filter { self.filters() } else { self.tests() };
        let func = match map.get_item(fname)? {
            Some(f) => f,
            None => return Ok(None),
        };
        // python's `_PassArg.from_obj`: context-dependent never folds.
        let pa_name: Option<String> = match func.getattr("jinja_pass_arg") {
            Ok(p) => p.getattr("name")?.extract()?,
            Err(_) => None,
        };
        if pa_name.as_deref() == Some("context") {
            return Ok(None);
        }
        let mut vals: Vec<Bound<'py, PyAny>> = Vec::new();
        match &na[2] {
            Av::List(l) => {
                for a in l {
                    match self.fold(a, vol, ae)? {
                        Some(v) => vals.push(v),
                        None => return Ok(None),
                    }
                }
            }
            _ => return Ok(None),
        }
        let kw = PyDict::new(self.py);
        match &na[3] {
            Av::List(l) => {
                for p in l {
                    let (pn, pa) = match parts(p) {
                        Some(x) => x,
                        None => return Ok(None),
                    };
                    if pa.len() < 2 {
                        return Ok(None);
                    }
                    let k = if pn == "Keyword" {
                        match arg_str(&pa[0]) {
                            Some(s) => self.lit(&Av::Str(s.to_string()))?,
                            None => return Ok(None),
                        }
                    } else if pn == "Pair" {
                        match self.fold(&pa[0], vol, ae)? { Some(v) => v, None => return Ok(None) }
                    } else {
                        return Ok(None);
                    };
                    let v = match self.fold(&pa[1], vol, ae)? { Some(v) => v, None => return Ok(None) };
                    kw.set_item(&k, &v)?;
                }
            }
            _ => return Ok(None),
        }
        // dynamic arguments fold through python's own extend/update rules.
        if !matches!(&na[4], Av::NoneV) {
            match self.fold(&na[4], vol, ae)? {
                Some(d) => {
                    #[allow(deprecated)] // `iter()` matches the rest of this file.
                    let iter = match d.iter() { Ok(i) => i, Err(_) => return Ok(None) };
                    for e in iter {
                        match e {
                            Ok(v) => vals.push(v),
                            Err(_) => return Ok(None),
                        }
                    }
                }
                None => return Ok(None),
            }
        }
        if !matches!(&na[5], Av::NoneV) {
            match self.fold(&na[5], vol, ae)? {
                Some(d) => {
                    if kw.call_method1("update", (&d,)).is_err() {
                        return Ok(None);
                    }
                }
                None => return Ok(None),
            }
        }
        let target = match self.fold(&na[0], vol, ae)? { Some(v) => v, None => return Ok(None) };
        // python inserts the target first, then the pass argument in front.
        let mut full: Vec<Bound<'py, PyAny>> = Vec::new();
        match pa_name.as_deref() {
            Some("eval_context") => full.push(self.evalctx()),
            Some("environment") => full.push(self.env()),
            _ => {}
        }
        full.push(target);
        full.extend(vals);
        let atup = PyTuple::new(self.py, &full)?;
        self.try_fold(func.call(atup, Some(&kw)))
    }

    /// Join of already finalized output pieces: the compiled engine yields
    /// strings and joins them with `Environment.concat` (plain `"".join`), so
    /// static template text is never escaped here.
    fn out_join(&self, chunks: Vec<Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
        #[cfg(feature = "prof")]
        return crate::prof::time("out-join", || self.out_join_inner(chunks));
        #[cfg(not(feature = "prof"))]
        self.out_join_inner(chunks)
    }

    fn out_join_inner(&self, chunks: Vec<Bound<'py, PyAny>>) -> ERes<Bound<'py, PyAny>> {
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
            let sl = self.slice_cls().call(
                (vals[0].clone(), vals[1].clone(), vals[2].clone()),
                None,
            )?;
            return Ok(obj.get_item(sl)?);
        }
        let key = self.node(arg)?;
        Ok(self.env().call_method1("getitem", (obj, key))?)
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
        let map = if is_filter { self.filters() } else { self.tests() };
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
        let ctxv = self.ctx();
        let evalv = self.evalctx();
        let flag = is_filter.into_bound_py_any(self.py)?;
        Ok(self.env().call_method(
            "_filter_test_common",
            (name, &value, &args_list, &kwargs_arg, &ctxv, &evalv, &flag),
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
        Ok(self.ctx().call_method("call", tup, kw.as_ref())?)
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
        if let Some(nm) = self.cur_name() {
            pos = format!("{} in {}", pos, py_repr_str(nm.as_str()));
        }
        let hint = format!(
            "the inline if-expression on {} evaluated to false and no else section was defined.",
            pos
        );
        Ok(self.rt().getattr("Undefined")?.call1((hint,))?)
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
                Ok(self.env().call_method1("getattr", (&obj, attr))?)
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
                    // python's optimizer runs on every non-volatile concat
                    // visit regardless of the autoescape flag; folded joins use
                    // plain str() semantics (`None` becomes 'None'), not markup.
                    let st = *self.aestate.borrow();
                    if !st.vol {
                        if let Some(v) = self.fold(av, false, st.ae)? {
                            return Ok(v);
                        }
                    }
                    let mut vals: Vec<Bound<PyAny>> = Vec::new();
                    for p in ps {
                        vals.push(self.node(p)?);
                    }
                    self.ae_join(vals)
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

    fn init_scope(&self, kinds: &HashMap<String, IK>) -> ERes<Scope> {
        let mut slots: HashMap<String, Option<Py<PyAny>>> = HashMap::new();
        let names: Vec<String> = kinds.keys().cloned().collect();
        for name in names {
            let k = kinds[&name];
            let v: Option<Py<PyAny>> = match k {
                IK::Param | IK::Undef => None,
                IK::Resolve => self.raw_resolve(&name)?,
                IK::Alias => match self.nearest_slot(&name) {
                    Some(v) => v,
                    None => None,
                },
            };
            slots.insert(name, v);
        }
        Ok(Scope { slots, boundary: false })
    }

    fn enter_scope(&self, kinds: &HashMap<String, IK>) -> ERes<()> {
        #[cfg(feature = "prof")]
        return crate::prof::time("scope-enter", || self.enter_scope_inner(kinds));
        #[cfg(not(feature = "prof"))]
        self.enter_scope_inner(kinds)
    }

    fn enter_scope_inner(&self, kinds: &HashMap<String, IK>) -> ERes<()> {
        let sc = self.init_scope(kinds)?;
        self.stack.borrow_mut().push(Rc::new(RefCell::new(sc)));
        Ok(())
    }

    /// Enter a scope whose compiled counterpart is a generated function's top
    /// frame (template root or block function): the symbol chain ends there.
    fn enter_boundary_scope(&self, kinds: &HashMap<String, IK>) -> ERes<()> {
        let mut sc = self.init_scope(kinds)?;
        sc.boundary = true;
        self.stack.borrow_mut().push(Rc::new(RefCell::new(sc)));
        Ok(())
    }

    fn pop_scope(&self) {
        #[cfg(feature = "prof")]
        crate::prof::time("scope-pop", || self.pop_scope_inner());
        #[cfg(not(feature = "prof"))]
        self.pop_scope_inner()
    }

    fn pop_scope_inner(&self) {
        self.stack.borrow_mut().pop();
    }

    /// Warm-path cache of the per-render pre-execution analyses for one tree.
    /// A miss runs the exact scan / layout check / root-frame symbol analysis
    /// that used to run on every render and stores the outcome; a hit (same
    /// tree pointer AND same environment autoescape flag) reuses it.  The
    /// parsed AST is immutable, so a fixed pointer identifies one source.
    fn unit_prep(&self, av: &Av) -> ERes<UnitPrep> {
        let key = av as *const Av;
        let ae = self.s.autoescape;
        if let Some(p) = self.s.unit_prep.borrow().get(&key) {
            if p.autoescape == ae {
                return Ok(p.clone());
            }
        }
        // Cold path: the unchanged per-render analyses, in their original
        // order (scan before layout), with the same error precedence.
        let scan_res = self.scan(av);
        let (scan_ok, scan_reason) = match scan_res {
            Ok(()) => (true, None),
            Err(EvalErr::Unsupported(u)) => (false, Some(u)),
            Err(EvalErr::Exc(e)) => return Err(EvalErr::Exc(e)),
        };
        let layout_res = if scan_ok { self.check_layout(av, true) } else { Ok(()) };
        let (layout_ok, layout_reason) = match layout_res {
            Ok(()) => (true, None),
            Err(EvalErr::Unsupported(u)) => (false, Some(u)),
            Err(EvalErr::Exc(e)) => return Err(EvalErr::Exc(e)),
        };
        let (root_kinds, uses_self) = if scan_ok && layout_ok {
            let items = match av {
                Av::Node("Template", _, a) => match a.first() {
                    Some(Av::List(l)) => l,
                    _ => return Err(nope("bad template body")),
                },
                _ => return Err(nope("template node expected")),
            };
            let mut occs: Vec<Occ> = Vec::new();
            for it in items {
                self.walk_fsf(it, &mut occs);
            }
            let mut kinds = self.build_kinds(&occs, &[]);
            // the root frame gets `self` when the name is accessed undeclared
            // outside blocks (codegen's find_undeclared over node.body).
            let mut track = [true];
            let mut found = [false];
            for it in items {
                self.spec1_walk(it, &mut track, &mut found);
            }
            if found[0] {
                kinds.insert("self".to_string(), IK::Param);
            }
            (kinds.into_iter().collect(), found[0])
        } else {
            (Vec::new(), false)
        };
        let prep = UnitPrep {
            autoescape: ae,
            scan_ok,
            scan_reason,
            layout_ok,
            layout_reason,
            root_kinds,
            uses_self,
        };
        // bounded: the AST cache holds at most 1024 trees, so beyond that the
        // map only churns -- drop entries until it fits again.
        let mut cache = self.s.unit_prep.borrow_mut();
        if cache.len() >= 2048 {
            cache.clear();
        }
        cache.insert(key, prep.clone());
        Ok(prep)
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



    fn exec_items(&self, items: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        for it in items {
            self.exec_one(it, out)?;
        }
        Ok(())
    }

    fn exec_one(&self, av: &Av, out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        let (name, lineno, args): (&str, Option<u32>, &[Av]) = match av {
            Av::Node(n, l, a) => (*n, *l, a),
            _ => return Err(nope("statement node expected")),
        };
        #[cfg(feature = "prof")]
        crate::prof::add("nodes-visited", 1);
        match name {
            // Top-level output of a template that mentions `{% extends %}` is
            // guarded at runtime by the `parent_template` check, or dropped
            // entirely once a root level extend was visited before it.
            "Output" => {
                if !self.ext_stack.borrow().is_empty() {
                    let known = self.flags.borrow().known_top;
                    let extended: bool = {
                        let st = self.ext_stack.borrow();
                        match st.last() {
                            Some(re) => *re.extended.borrow() && re.any_extends,
                            None => false,
                        }
                    };
                    if known || extended {
                        return Ok(());
                    }
                }
                let items = match args.first() {
                    Some(Av::List(l)) => l,
                    _ => return Err(nope("bad output node")),
                };
                // Static template data renders raw in every state: python folds
                // it into module literals (the compile-time escape is a no-op on
                // its Markup wrap) and even the volatile runtime path wraps it
                // with `(Markup if autoescape else identity)` inside the outer
                // `(escape if autoescape else str)` so nothing can escape it.
                // Other children ARE constant-folded at compile time whenever
                // `as_const` succeeds (finalize.const is populated even without
                // a user finalizer!) -- escaped according to the *compile-time*
                // attribute, not the live one; only non-foldable ones take the
                // runtime wrappers with their per-part live checks.  In
                // non-volatile regions both routes coincide (escape/str around
                // an equal value), so folding there is optional; volatile
                // regions must reproduce the compile-time fold, because their
                // runtime attribute may disagree with it.
                let st = *self.aestate.borrow();
                for it in items {
                    match it {
                        Av::Node("TemplateData", _, td) => out.push(self.lit(&td[0])?),
                        _ => {
                            if st.vol {
                                if let Some(c) = self.fold(it, true, st.ae)? {
                                    let esc: Bound<'py, PyAny> = if st.ae {
                                        self.esc().call1((&c,))?
                                    } else { c };
                                    out.push(esc.str()?.into_any());
                                    continue;
                                }
                            }
                            let v = self.node(it)?;
                            out.push(self.out_wrap(&v)?);
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
                    Some(b) => {
                        self.enter_if_flags();
                        self.exec_items(b, out)
                    }
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
            "Macro" => self.exec_macro(args),
            "CallBlock" => self.exec_call_block(args, out),
            "Extends" => self.exec_extends(args),
            "Block" => self.exec_block(args, out),
            // slice C-v5: includes and imports
            "Include" => self.exec_include(args, out),
            "Import" => self.exec_import(args),
            "FromImport" => self.exec_from_import(args, lineno),
            // slice C-v6: the `{% autoescape %}` tag and its scope wrapper
            "Scope" => {
                if args.len() < 1 {
                    return Err(nope("bad scope node"));
                }
                let body = match &args[0] {
                    Av::List(b) => b,
                    _ => return Err(nope("bad scope body")),
                };
                self.enter_body_flags();
                let mut occs: Vec<Occ> = Vec::new();
                for it in body {
                    self.walk_fsf(it, &mut occs);
                }
                let ancestors = self.owned_union();
                let kinds = self.build_kinds(&occs, &ancestors);
                self.enter_scope(&kinds)?;
                let res = self.exec_items(body, out);
                self.pop_scope();
                res
            }
            "ScopedEvalContextModifier" => {
                if args.len() < 2 {
                    return Err(nope("bad autoescape tag"));
                }
                let opts = match &args[0] {
                    Av::List(o) => o,
                    _ => return Err(nope("bad autoescape options")),
                };
                let body = match &args[1] {
                    Av::List(b) => b,
                    _ => return Err(nope("bad autoescape body")),
                };
                // generated code: `t_x = context.eval_ctx.save()`, one
                // assignment per option keyword, the body, then revert.
                let evalv = self.evalctx();
                let saved = evalv.call_method0("save")?;
                let oldst = *self.aestate.borrow();
                let mut newst = oldst;
                for kw in opts {
                    let (kn, ka) = match parts(kw) {
                        Some(x) => x,
                        None => return Err(nope("bad autoescape keyword")),
                    };
                    if kn != "Keyword" || ka.len() < 2 {
                        return Err(nope("bad autoescape keyword node"));
                    }
                    let key = match arg_str(&ka[0]) {
                        Some(s) => s,
                        None => return Err(nope("autoescape option name")),
                    };
                    // the runtime assignment is emitted for every keyword.
                    let v = self.node(&ka[1])?;
                    evalv.setattr(key, &v)?;
                    // compile-time folding decides the region mode exactly
                    // like python's visit_EvalContextModifier: success sets
                    // the attribute value (truthiness decides later checks),
                    // failure marks volatile.
                    // python mutates its single shared compile-time context
                    // inside the loop: later options fold against what earlier
                    // ones left behind (volatile once any of them failed).
                    match self.fold(&ka[1], newst.vol, newst.ae)? {
                        Some(c) => newst.ae = c.is_truthy()?,
                        None => newst.vol = true,
                    }
                }
                *self.aestate.borrow_mut() = newst;
                let res = self.exec_items(body, out);
                // restore both the live eval context and the mirrored compile
                // state exactly like the emitted revert lines.
                evalv.call_method1("revert", (&saved,))?;
                *self.aestate.borrow_mut() = oldst;
                res
            }
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
        if !names.is_empty() && self.stack.borrow().len() == 1 && !self.local_only {
            let vars = self.ctx().getattr("vars")?;
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
                let ex = self.ctx().getattr("exported_vars")?;
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
        let found: Option<Option<Py<PyAny>>> = {
            let frames = self.stack.borrow();
            let mut out: Option<Option<Py<PyAny>>> = None;
            for frame in frames.iter().rev() {
                if let Some(v) = frame.borrow().slots.get(name).cloned() {
                    out = Some(v);
                    break;
                }
            }
            out
        };
        let v: Option<Py<PyAny>> = match found {
            Some(inner) => inner,
            None => self.raw_resolve(name)?,
        };
        Ok(match v {
            Some(x) => x.into_bound(self.py),
            None => {
                let kw = PyDict::new(self.py);
                kw.set_item("name", name)?;
                self.undef().call((), Some(&kw))?
            }
        })
    }

    fn store_target(&self, tgt: &Av, val: &Bound<'py, PyAny>, iter_style: bool) -> ERes<()> {
        #[cfg(feature = "prof")]
        return crate::prof::time("store-target", || self.store_target_inner(tgt, val, iter_style));
        #[cfg(not(feature = "prof"))]
        self.store_target_inner(tgt, val, iter_style)
    }

    fn store_target_inner(&self, tgt: &Av, val: &Bound<'py, PyAny>, iter_style: bool) -> ERes<()> {
        match tgt {
            Av::Node("Name", _, a) => {
                let name = match arg_str(&a[0]) {
                    Some(s) => s,
                    None => return Err(nope("name target")),
                };
                {
                    let frames = self.stack.borrow();
                    for frame in frames.iter().rev() {
                        let mut sc = frame.borrow_mut();
                        if sc.slots.contains_key(name) {
                            sc.slots.insert(name.to_string(), Some(val.clone().unbind()));
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
                if !obj.is_instance(&self.ns())? {
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
        self.enter_body_flags();
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
        // the implicit filter target is `Markup(concat(buf))` when compiled
        // under a non-volatile autoescape frame; in volatile regions the wrap
        // sits behind a runtime check of the live attribute.
        let mut value = self.env().call_method1("concat", (&lst,))?;
        let st = *self.aestate.borrow();
        let wrap = if !st.vol { st.ae } else { self.live_ae()? };
        if wrap {
            value = self.markup_cls().call1((&value,))?;
        }
        let applied = self.eval_filter_hole(&args[1], &value)?;
        self.pop_scope();
        // start_write/end_write append the filtered result as a raw object --
        // no str()/escape wrapper; soft_str semantics apply at the joins.
        out.push(applied);
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
        self.enter_body_flags();
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
        let mut value = self.env().call_method1("concat", (&lst,))?;
        if !matches!(&args[1], Av::NoneV) {
            value = self.eval_filter_hole(&args[1], &value)?;
        }
        // the stored value is `(Markup if context.eval_ctx.autoescape else
        // identity)(...)` -- a runtime check of the live attribute, emitted
        // unconditionally regardless of the compile-time state.
        if self.live_ae()? {
            value = self.markup_cls().call1((&value,))?;
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
        // `extended_loop` in visit_For: recursive mode, a body load of the
        // special loop variable -- or ANY scoped block anywhere below the for
        // node (codegen's find_all over the whole subtree), because its call
        // site dumps stores through this loop frame.
        let mut extended = args.len() > 5 && self.deep_name_load_in(body, "loop");
        if !extended {
            let mut scan: Option<bool> = None;
            any_scoped(args, &mut scan);
            extended = scan.unwrap_or(false);
        }
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
        self.enter_body_flags();
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
            // the root function yields the loop result as a plain object
            // (`yield loop(xs, loop)`); no output wrapper applies.
            let v = self.node(&args[1])?;
            let res = self.run_recursive(args, v, 0)?;
            out.push(res);
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
            let lc = self.loopctx().call1((&iter_val, &self.undef(), none, 0i64))?;
            let mut iterated = lc.try_iter()?;
            while let Some(pair_r) = iterated.next() {
                let (val, loopobj): (Bound<'py, PyAny>, Bound<'py, PyAny>) = pair_r?.extract()?;
                self.enter_scope(&kinds)?;
                self.store_target(&args[0], &val, false)?;
                {
                    if let Some(frame) = self.stack.borrow().last().cloned() {
                        frame.borrow_mut().slots.insert(
                            "loop".to_string(),
                            Some(loopobj.clone().unbind()),
                        );
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
            // Fast path for simple loop targets
            let simple_target = matches!(&args[0], Av::Node("Name", _, _));
            let reusable = simple_target
                && kinds.iter().all(|(_, k)| matches!(k, IK::Param | IK::Undef)); // no Resolve/Alias
            if reusable {
                // Get the target name for reuse
                let target_name = match &args[0] {
                    Av::Node("Name", _, a) if !a.is_empty() => arg_str(&a[0]).ok_or(nope("invalid loop target"))?,
                    _ => return Err(nope("invalid loop target")),
                };
                
                let frame = Rc::new(RefCell::new(self.init_scope(&kinds)?)); // ONCE
                self.stack.borrow_mut().push(frame.clone());
                let mut it = iter_val.try_iter()?;
                while let Some(item_r) = it.next() {
                    let item = item_r?;
                    if let Some(top) = self.stack.borrow().last().cloned() {   // top == loop frame (store precedes body)
                        top.borrow_mut().slots.insert(target_name.to_string(), Some(item.clone().unbind()));
                    }
                    self.exec_items(&body_of(args), out)?;
                    ran = true;
                }
                self.pop_scope();
            } else {
                // existing enter/store/exec/pop loop, unchanged
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
        let lc = self.loopctx().call1((&iterable, &self.undef(), none, depth0))?;
        let lptr = lc.as_ptr() as usize;
        self.s.rec.borrow_mut().push(RecEntry {
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
                if let Some(frame) = self.stack.borrow().last().cloned() {
                    frame.borrow_mut().slots.insert(
                        "loop".to_string(),
                        Some(loopobj.clone().unbind()),
                    );
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
        let joined = self.env().call_method1("concat", (&lst,))?;
        // `return_buffer_contents`: non-volatile frames wrap according to the
        // compile-time flag; volatile regions emit a runtime check of the
        // *live* autoescape attribute.
        let st = *self.aestate.borrow();
        let wrap = if !st.vol { st.ae } else { self.live_ae()? };
        if wrap {
            return Ok(self.markup_cls().call1((&joined,))?);
        }
        Ok(joined)
    }

    /// Reject for loops whose test filter rides together with extended or
    /// recursive mode before anything runs.  The python code generator has a
    /// dedicated reciter-wrapping / LoopContext-over-the-filter-generator
    /// path for those; reproducing it is deferred (see TODO.md), and deciding
    /// at scan time keeps the fallback out of block function and macro bodies
    /// where an unsupported report could no longer turn into a clean engine
    /// switch.  A scoped block below the for node forces extended mode even
    /// without any `loop` load, exactly like visit_For's `extended_loop`.
    fn scan_for(&self, args: &[Av]) -> Result<(), EvalErr> {
        if args.len() < 6 {
            return Ok(());
        }
        let has_test = !matches!(&args[4], Av::NoneV);
        if !has_test {
            return Ok(());
        }
        let body: &[Av] = match &args[2] {
            Av::List(b) => b,
            _ => &[],
        };
        // extended mode is forced by recursion, a load of `loop` in the body
        // or any scoped block below (codegen's extended_loop condition).
        let mut scoped: Option<bool> = None;
        any_scoped(args, &mut scoped);
        let extended = matches!(&args[5], Av::Bool(true))
            || self.deep_name_load_in(body, "loop")
            || scoped == Some(true);
        if extended {
            return Err(nope("for with test filter in extended/recursive mode"));
        }
        Ok(())
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
            Av::Node(name, lineno, args) => {
                if !SUPPORTED_NODES.contains(name) {
                    return Err(nope(*name));
                }
                if *name == "Filter" || *name == "Test" {
                    if args.len() >= 6 {
                        if let Some(nm) = arg_str(&args[1]) {
                            let is_filter = *name == "Filter";
                            let map = if is_filter { self.filters() } else { self.tests() };
                            if map.get_item(nm)?.is_none() {
                                let kind = if is_filter { "filter" } else { "test" };
                                return Err(nope(format!("missing {} {:?}", kind, nm)));
                            }
                            // Compile-time folding of a filter/test whose every
                            // part is constant means python's code generator
                            // CALLS the function once while generating and then
                            // never at render; a rust-served evaluation of the
                            // same tree would run that user code once per
                            // render instead -- side-effect counts diverge even
                            // though rendered text matches.  Decline rather
                            // than guess (PLAN_C_DONE.md R1): the python path
                            // stays byte-identical, including the call counts
                            // the forked suite asserts on.
                            if self.folds_at_compile(is_filter, args)? {
                                let kind = if is_filter { "filter" } else { "test" };
                                return Err(nope(format!(
                                    "compile-time folded {} {:?}",
                                    kind,
                    nm
                                )));
                            }
                        }
                    }
                }
                // Dynamic call / filter / test arguments (`*a` / `**k`) are
                // rejected at scan time, not only at evaluation: a template
                // whose verdict was cached as "supported" must never decline
                // part-way through a render (the never-guess invariant), so
                // every exec-time unsupported path has to be caught by the
                // scan too.  These remain correct via the python fallback.
                if *name == "Call" && args.len() >= 5 {
                    if !matches!(&args[3], Av::NoneV) || !matches!(&args[4], Av::NoneV) {
                        return Err(nope("dynamic call arguments"));
                    }
                }
                if (*name == "Filter" || *name == "Test") && args.len() >= 6 {
                    if !matches!(&args[4], Av::NoneV) || !matches!(&args[5], Av::NoneV) {
                        return Err(nope("dynamic filter/test arguments"));
                    }
                }
                // python's codegen writes keyword arguments into generated
                // source, so a duplicated key makes CPython fail with
                // `SyntaxError: keyword argument repeated` whose message
                // embeds generated line numbers -- not reproducible here, so
                // fall back.  When any key is a python keyword the generator
                // takes the dict-literal path instead, where a duplicate just
                // loses to the later entry: exactly what this interpreter
                // does, so no fallback is needed in that case.
                // a call block always forwards `caller=`, so an explicit
                // `caller=` in the target's own keywords duplicates it in the
                // generated source -- same SyntaxError as above.
                if *name == "CallBlock" {
                    if let Some(Av::Node("Call", _, ca)) = args.get(0) {
                        if let Some(Av::List(kl)) = ca.get(2) {
                            if !kl.iter().any(|k| is_python_keyword(kw_key(k)))
                                && kl.iter().any(|k| kw_key(k) == Some("caller"))
                            {
                                return Err(nope("call block caller argument"));
                            }
                        }
                    }
                }
                if *name == "Call" || *name == "Filter" || *name == "Test" {
                    // both `Call` and `_FilterTestCommon` nodes keep the
                    // kwargs list in field slot 2/3 respectively
                    let kwidx = match *name {
                        "Call" => 2,
                        _ => 3,
                    };
                    if let Some(Av::List(kl)) = args.get(kwidx) {
                        if !kl.iter().any(|k| is_python_keyword(kw_key(k))) {
                            let mut seen: Vec<&str> = Vec::new();
                            for k in kl {
                                if let Some(key) = kw_key(k) {
                                    if seen.iter().any(|s| *s == key) {
                                        return Err(nope("duplicate keyword argument"));
                                    }
                                    seen.push(key);
                                }
                            }
                        }
                    }
                }
                if *name == "For" {
                    self.scan_for(args)?;
                    if let Some(l) = self.find_loop_store(av) {
                        return Err(EvalErr::Exc(
                            self.assertion_error(
                                "Can't assign to special loop variable in for-loop target",
                                Some(l),
                            )?,
                        ));
                    }
                }
                // codegen's macro_body fails when the body references caller
                // and an explicit `caller` parameter lacks a default.
                if (*name == "Macro" || *name == "CallBlock") && args.len() >= 4 {
                    if let (Av::List(p), Av::List(d), Av::List(b)) =
                        (&args[1], &args[2], &args[3])
                    {
                        // `macro_body` only remembers the LAST parameter
                        // named caller (later occurrences overwrite), and only
                        // fails when that one has no default -- defaults are
                        // attached to the trailing parameters.
                        let last_caller = p.iter().rposition(|pm| {
                            param_name(pm) == Some("caller")
                        });
                        if let Some(idx) = last_caller {
                            let has_default = idx + d.len() >= p.len();
                            if !has_default {
                                let (loads_caller, _, _) = self.find_specials(b);
                                if loads_caller {
                                    return Err(EvalErr::Exc(self.assertion_error(
                                        "When defining macros or call blocks the special \"caller\" argument must be omitted or be given a default.",
                                        *lineno,
                                    )?));
                                }
                            }
                        }
                        // python's own compiler rejects duplicate parameter
                        // names in the generated function definition with a
                        // SyntaxError whose message embeds generated line
                        // numbers -- not reproducible here, so fall back.
                        let mut seen: Vec<&str> = Vec::new();
                        for pm in p.iter() {
                            if let Some(n) = param_name(pm) {
                                if seen.iter().any(|s| *s == n) {
                                    return Err(nope("duplicate macro parameter"));
                                }
                                seen.push(n);
                            }
                        }
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

    /// Would python's optimizer fold this `_FilterTestCommon` node at compile
    /// time -- i.e. does code generation *call* the filter or test function
    /// once while generating, so that a rust-served evaluation of the same
    /// expression would run that user code an extra time per render?  Decided
    /// without executing: the subject and every argument must be pure per
    /// :meth:`pure_fold`.  Filter-block targets (subject ``None``) and
    /// context-marked callables never fold, exactly like
    /// `_FilterTestCommon.as_const`; eval-context/environment markers DO fold,
    /// by calling with the extra argument inserted.  A volatile region would
    /// also skip folding at codegen time -- treating those as folded anyway
    /// only declines a template that could have been served, never serves a
    /// divergent one, so the simplification is safe (recorded in
    /// ACCEPTED_DIVERGENCES.md).
    fn folds_at_compile(&self, is_filter: bool, na: &[Av]) -> Result<bool, EvalErr> {
        if na.len() < 6 || matches!(&na[0], Av::NoneV) {
            return Ok(false);
        }
        let fname = match arg_str(&na[1]) {
            Some(s) => s,
            None => return Ok(false),
        };
        let map = if is_filter { self.filters() } else { self.tests() };
        let func = match map.get_item(fname)? {
            Some(f) => f,
            None => return Ok(false),
        };
        // Only the context-marked callables are excluded from folding; the
        // marker attribute is the same one `fold_filter_test` consults.
        if let Ok(p) = func.getattr("jinja_pass_arg") {
            if let Ok(nm) = p.getattr("name")?.extract::<String>() {
                if nm == "context" {
                    return Ok(false);
                }
            }
        }
        // Dynamic argument slots are rejected by the scan separately; here
        // they just mean "not provably pure".
        if !matches!(&na[4], Av::NoneV) || !matches!(&na[5], Av::NoneV) {
            return Ok(false);
        }
        if self.pure_fold(&na[0])?.is_none() {
            return Ok(false);
        }
        match &na[2] {
            Av::List(l) => {
                for a in l {
                    if self.pure_fold(a)?.is_none() {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
        match &na[3] {
            Av::List(l) => {
                for p in l {
                    let (pn, pa) = match parts(p) {
                        Some(x) => x,
                        None => return Ok(false),
                    };
                    if pn != "Pair" || pa.len() < 2 {
                        return Ok(false);
                    }
                    if self.pure_fold(&pa[1])?.is_none() {
                        return Ok(false);
                    }
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Restricted `as_const` mirror that never touches user callables -- used
    /// by the scan to classify foldability without executing.  Only literals,
    /// static containers, arithmetic / comparison / concatenation over pure
    /// parts and short-circuit boolean / conditional logic may fold; a name
    /// load, attribute lookup, subscript, call, filter or test makes the whole
    /// subtree impure, and an operator raising while folding counts as
    /// non-foldable exactly like `Impossible` does for python's optimizer.
    fn pure_fold(&self, av: &Av) -> ERes<Option<Bound<'py, PyAny>>> {
        let (name, args): (&str, &[Av]) = match av {
            Av::Node(n, _, a) => (*n, a),
            _ => return Ok(None),
        };
        match name {
            "Const" | "TemplateData" => match args.first() {
                Some(x) => Ok(Some(self.lit(x)?)),
                None => Ok(None),
            },
            "Tuple" | "List" => {
                let items = match args.first() {
                    Some(Av::List(l)) => l,
                    _ => return Ok(None),
                };
                let mut vals: Vec<Bound<'py, PyAny>> = Vec::new();
                for it in items {
                    match self.pure_fold(it)? {
                        Some(v) => vals.push(v),
                        None => return Ok(None),
                    }
                }
                if name == "Tuple" {
                    Ok(Some(PyTuple::new(self.py, &vals)?.into_any()))
                } else {
                    Ok(Some(PyList::new(self.py, &vals)?.into_any()))
                }
            }
            "Dict" => {
                let items = match args.first() {
                    Some(Av::List(l)) => l,
                    _ => return Ok(None),
                };
                let d = PyDict::new(self.py);
                for it in items {
                    let (pn, pa) = match parts(it) {
                        Some(x) => x,
                        None => return Ok(None),
                    };
                    if pn != "Pair" || pa.len() < 2 {
                        return Ok(None);
                    }
                    let k = match self.pure_fold(&pa[0])? { Some(v) => v, None => return Ok(None) };
                    let v = match self.pure_fold(&pa[1])? { Some(v) => v, None => return Ok(None) };
                    d.set_item(&k, &v)?;
                }
                Ok(Some(d.into_any()))
            }
            "And" | "Or" if args.len() >= 2 => {
                // python's as_const short-circuits exactly like the runtime
                // operators: `left and right` / `left or right` in value terms.
                let l = match self.pure_fold(&args[0])? { Some(v) => v, None => return Ok(None) };
                let truthy = l.is_truthy()?;
                if truthy == (name == "Or") {
                    Ok(Some(l))
                } else {
                    match self.pure_fold(&args[1])? {
                        Some(v) => Ok(Some(v)),
                        None => Ok(None),
                    }
                }
            }
            "Not" | "Neg" | "Pos" if args.len() >= 1 => {
                let v = match self.pure_fold(&args[0])? { Some(v) => v, None => return Ok(None) };
                match name {
                    "Not" => Ok(Some((!v.is_truthy()?).into_bound_py_any(self.py)?)),
                    "Neg" => self.try_fold(v.neg()),
                    _ => self.try_fold(v.pos()),
                }
            }
            "Add" | "Sub" | "Mul" | "Div" | "FloorDiv" | "Pow" | "Mod" if args.len() >= 2 => {
                let a = match self.pure_fold(&args[0])? { Some(v) => v, None => return Ok(None) };
                let b = match self.pure_fold(&args[1])? { Some(v) => v, None => return Ok(None) };
                let r: std::result::Result<Bound<'py, PyAny>, PyErr> = match name {
                    "Add" => a.add(&b),
                    "Sub" => a.sub(&b),
                    "Mul" => a.mul(&b),
                    "Div" => a.div(&b),
                    "FloorDiv" => a.floor_div(&b),
                    "Mod" => a.rem(&b),
                    _ => a.pow(&b, self.py.None()),
                };
                self.try_fold(r)
            }
            "Compare" if args.len() >= 2 => {
                let mut cur = match self.pure_fold(&args[0])? { Some(v) => v, None => return Ok(None) };
                let ops = match &args[1] {
                    Av::List(l) => l,
                    _ => return Ok(None),
                };
                for op in ops.iter() {
                    let (on, oa) = match parts(op) {
                        Some(x) => x,
                        None => return Ok(None),
                    };
                    if on != "Operand" || oa.len() < 2 {
                        return Ok(None);
                    }
                    let rhs = match self.pure_fold(&oa[1])? { Some(v) => v, None => return Ok(None) };
                    let ok = match arg_str(&oa[0]) {
                        Some(opc) if opc == "eq" => cur.eq(&rhs)?,
                        Some(opc) if opc == "ne" => cur.ne(&rhs)?,
                        Some(opc) if opc == "lt" => cur.lt(&rhs)?,
                        Some(opc) if opc == "lteq" => cur.le(&rhs)?,
                        Some(opc) if opc == "gt" => cur.gt(&rhs)?,
                        Some(opc) if opc == "gteq" => cur.ge(&rhs)?,
                        Some(opc) if opc == "in" => rhs.contains(&cur)?,
                        Some(opc) if opc == "notin" => !rhs.contains(&cur)?,
                        _ => return Ok(None),
                    };
                    if !ok {
                        return Ok(Some(false.into_bound_py_any(self.py)?));
                    }
                    cur = rhs;
                }
                Ok(Some(true.into_bound_py_any(self.py)?))
            }
            "Concat" => {
                let ps = match args.first() {
                    Some(Av::List(l)) => l,
                    _ => return Ok(None),
                };
                // python's `Concat.as_const` joins with plain str() semantics.
                let mut buf = String::new();
                for pt in ps {
                    match self.pure_fold(pt)? {
                        Some(v) => buf.push_str(&v.str()?.extract::<String>()?),
                        None => return Ok(None),
                    }
                }
                Ok(Some(PyString::new(self.py, &buf).into_any()))
            }
            "CondExpr" if args.len() >= 3 => {
                let t = match self.pure_fold(&args[0])? { Some(v) => v, None => return Ok(None) };
                if t.is_truthy()? {
                    match self.pure_fold(&args[1])? { Some(v) => Ok(Some(v)), None => Ok(None) }
                } else if matches!(&args[2], Av::NoneV) {
                    // implicit undefined never folds (as_const raises Impossible)
                    Ok(None)
                } else {
                    match self.pure_fold(&args[2])? { Some(v) => Ok(Some(v)), None => Ok(None) }
                }
            }
            _ => Ok(None),
        }
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
                self.cur_name(),
                None::<String>,
            ),
            None,
        )?;
        inst.setattr("translated", true)?;
        Ok(PyErr::from_value(inst))
    }

    fn unregister_lc(&self, ptr: usize) {
        let mut r = self.s.rec.borrow_mut();
        if let Some(i) = r.iter().position(|e| e.ptr == ptr) {
            r.swap_remove(i);
        }
    }

    fn rec_find(&self, func: &Bound<'py, PyAny>) -> Option<Vec<Av>> {
        let r = self.s.rec.borrow();
        let fptr = func.as_ptr() as usize;
        for e in r.iter().rev() {
            if e.ptr == fptr {
                return Some(e.args.clone());
            }
        }
        None
    }

    fn exec_with(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        self.enter_body_flags();
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

    // ------------------------------------------------------ slice C-v3b --
    //
    // Macros and call blocks.  A macro definition creates a real
    // `runtime.Macro` python object whose func is a rust callable holding the
    // defining frame chain: closure cells stay alive and writable through the
    // `Rc`s exactly like the compiled engine's closures over root locals.
    // Invocations enter a fresh scope frame initialized from the define-time
    // symbol analysis, bind parameters (missing sentinel -> default expression
    // evaluated inside the macro frame, or environment.undefined with the
    // codegen's hint), render the buffered body and return it joined through
    // `Environment.concat`.  Argument matching itself runs in python's
    // `Macro.__call__`, byte-identical semantics by construction.

    /// UndeclaredNameVisitor-style detection of the special names inside a
    /// macro / call block body: a load marks the name as accessed; a store or
    /// parameter occurrence drops it from tracking afterwards (python's
    /// discard behaviour), so only loads before the first binding count.
    fn find_specials(&self, items: &[Av]) -> (bool, bool, bool) {
        let mut track = [true, true, true];
        let mut found = [false, false, false];
        for it in items {
            self.spec_walk(it, &mut track, &mut found);
        }
        (found[0], found[1], found[2])
    }

    fn spec_walk(&self, av: &Av, track: &mut [bool; 3], found: &mut [bool; 3]) {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.spec_walk(a, track, found);
                }
            }
            Av::Node("Name", _, a) => {
                if a.len() >= 2 {
                    if let Some(n) = arg_str(&a[0]) {
                        let idx = match n {
                            "caller" => Some(0usize),
                            "kwargs" => Some(1),
                            "varargs" => Some(2),
                            _ => None,
                        };
                        if let Some(i) = idx {
                            if track[i] {
                                if arg_str(&a[1]) == Some("load") {
                                    found[i] = true;
                                } else {
                                    track[i] = false;
                                }
                            }
                        }
                    }
                }
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.spec_walk(x, track, found);
                }
            }
            _ => {}
        }
    }

    /// Define-time analysis mirroring `macro_body` + the two `analyze_node`
    /// passes: occurrence order parameters, defaults, body; implicit specials
    /// then force PARAM kinds (the `declare_parameter` override).
    fn analyze_macro(
        &self,
        params: &[Av],
        defaults: &[Av],
        body: &[Av],
    ) -> ERes<MacroPlan> {
        let mut occs: Vec<Occ> = Vec::new();
        for p in params {
            self.walk_fsf(p, &mut occs);
        }
        for d in defaults {
            self.walk_fsf(d, &mut occs);
        }
        for it in body {
            self.walk_fsf(it, &mut occs);
        }
        let ancestors = self.owned_union();
        let mut kinds = self.build_kinds(&occs, &ancestors);

        let (loads_caller, loads_kwargs, loads_varargs) = self.find_specials(body);
        let mut explicit_caller = false;
        let mut has_kwargs_param = false;
        let mut has_varargs_param = false;
        for p in params {
            match param_name(p) {
                Some("caller") => explicit_caller = true,
                Some("kwargs") => has_kwargs_param = true,
                Some("varargs") => has_varargs_param = true,
                _ => {}
            }
        }
        let caller_implicit = loads_caller && !explicit_caller;
        let accesses_kwargs = loads_kwargs && !has_kwargs_param;
        let accesses_varargs = loads_varargs && !has_varargs_param;
        if caller_implicit {
            kinds.insert("caller".to_string(), IK::Param);
        }
        if accesses_kwargs {
            kinds.insert("kwargs".to_string(), IK::Param);
        }
        if accesses_varargs {
            kinds.insert("varargs".to_string(), IK::Param);
        }

        // defaults belong to the trailing parameters (the parser forbids a
        // non-default argument after a default one)
        let l = params.len();
        let d = defaults.len();
        let mut plist: Vec<(String, Option<Av>)> = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let name = match param_name(p) {
                Some(n) => n,
                None => return Err(nope("macro parameter target")),
            };
            let def = if i + d >= l {
                Some(defaults[i + d - l].clone())
            } else {
                None
            };
            plist.push((name.to_string(), def));
        }
        Ok(MacroPlan {
            kinds,
            params: plist,
            accesses_caller: loads_caller,
            caller_implicit,
            accesses_kwargs,
            accesses_varargs,
        })
    }

    /// Build the `runtime.Macro` python object for a fresh closure over the
    /// current defining frame chain.
    fn make_runtime_macro(
        &self,
        name: Option<&str>,
        plan: &MacroPlan,
        body: Vec<Av>,
    ) -> ERes<Bound<'py, PyAny>> {
        let chain = self.stack.borrow().clone();
        let ctxobj = self.ctx().unbind();
        let tname = self.cur_name();
        let func = Py::new(
            self.py,
            MacroFunc {
                session: self.s.clone(),
                tname,
                ctxobj,
                chain,
                kinds: plan.kinds.clone(),
                params: plan.params.clone(),
                caller_slot: plan.caller_implicit,
                kwargs_slot: plan.accesses_kwargs,
                varargs_slot: plan.accesses_varargs,
                body,
                aestate: *self.aestate.borrow(),
            },
        )?;
        let macrocls = self.rt().getattr("Macro")?;
        let envb = self.env();
        let argnames: Vec<Bound<'py, PyAny>> = plan
            .params
            .iter()
            .map(|(n, _)| PyString::new(self.py, n).into_any())
            .collect();
        let args_tup = PyTuple::new(self.py, argnames)?;
        let name_arg: Option<String> = name.map(|s| s.to_string());
        Ok(macrocls.call(
            (
                envb,
                func,
                name_arg,
                args_tup,
                plan.accesses_kwargs,
                plan.accesses_varargs,
                plan.accesses_caller,
                false,
            ),
            None,
        )?)
    }

    /// Store a named value into the nearest owning slot plus the root-level
    /// writeback and exported_vars bookkeeping (`visit_Macro` semantics).
    fn store_named(&self, name: &str, val: &Bound<'py, PyAny>) -> ERes<()> {
        let mut stored = false;
        {
            let frames = self.stack.borrow();
            for frame in frames.iter().rev() {
                let mut sc = frame.borrow_mut();
                if sc.slots.contains_key(name) {
                    sc.slots.insert(name.to_string(), Some(val.clone().unbind()));
                    stored = true;
                    break;
                }
            }
        }
        if !stored {
            return Err(nope(format!("store to unowned name {name:?}")));
        }
        if self.stack.borrow().len() == 1 && !self.local_only {
            let vars = self.ctx().getattr("vars")?;
            vars.set_item(name, val)?;
            if !name.starts_with('_') {
                let ex = self.ctx().getattr("exported_vars")?;
                let lst = PyList::new(self.py, [PyString::new(self.py, name)])?;
                ex.call_method1("update", (&lst,))?;
            }
        }
        Ok(())
    }

    fn set_top_slot(&self, name: &str, v: &Bound<'py, PyAny>) -> ERes<()> {
        let top = self.stack.borrow().last().cloned();
        match top {
            Some(frame) => {
                frame
                    .borrow_mut()
                    .slots
                    .insert(name.to_string(), Some(v.clone().unbind()));
                Ok(())
            }
            None => Err(nope("no scope frame to bind")),
        }
    }

    fn exec_macro(&self, args: &[Av]) -> ERes<()> {
        if args.len() < 4 {
            return Err(nope("bad macro node"));
        }
        let name = match arg_str(&args[0]) {
            Some(s) => s,
            None => return Err(nope("macro name")),
        };
        let (params_v, defaults_v, body_v) = match (&args[1], &args[2], &args[3]) {
            (Av::List(p), Av::List(d), Av::List(b)) => (p, d, b),
            _ => return Err(nope("bad macro node")),
        };
        let plan = self.analyze_macro(params_v, defaults_v, body_v)?;
        let obj = self.make_runtime_macro(Some(name), &plan, body_v.clone())?;
        self.store_named(name, &obj)
    }

    fn exec_call_block(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        if args.len() < 4 {
            return Err(nope("bad call block"));
        }
        let (params_v, defaults_v, body_v) = match (&args[1], &args[2], &args[3]) {
            (Av::List(p), Av::List(d), Av::List(b)) => (p, d, b),
            _ => return Err(nope("bad call block signature")),
        };
        let plan = self.analyze_macro(params_v, defaults_v, body_v)?;
        let caller = self.make_runtime_macro(None, &plan, body_v.clone())?;

        // the target call runs in the defining frame with `caller=` injected
        // into the keyword arguments (codegen's forward_caller=True)
        let (cn, ca) = match parts(&args[0]) {
            Some(x) => x,
            None => return Err(nope("bad call node")),
        };
        if cn != "Call" || ca.len() < 5 {
            return Err(nope("call block expects a call node"));
        }
        if !matches!(&ca[3], Av::NoneV) || !matches!(&ca[4], Av::NoneV) {
            return Err(nope("dynamic call block arguments"));
        }
        let target = self.node(&ca[0])?;
        let (alist, kw) = match (&ca[1], &ca[2]) {
            (Av::List(a), Av::List(k)) => self.sig(a, k, None)?,
            _ => return Err(nope("call argument lists")),
        };
        let kwmap = match kw {
            Some(d) => d,
            None => PyDict::new(self.py),
        };
        kwmap.set_item("caller", &caller)?;
        let mut all: Vec<Bound<'py, PyAny>> = Vec::with_capacity(alist.len() + 1);
        all.push(target);
        all.extend(alist);
        let tup = PyTuple::new(self.py, all)?;
        // start_write/end_write append the raw call result into the parent
        // buffer without any str()/escape wrapper.
        let rv = self.ctx().call_method("call", tup, Some(&kwmap))?;
        out.push(rv);
        Ok(())
    }
    // ------------------------------------------------------- slice C-v4 --
    //
    // Block inheritance.  `{% extends %}` loads a prepared parent unit and
    // appends its own block functions into `context.blocks` exactly like the
    // generated code does; block invocations always take index 0 of that
    // registry (the most derived override), and `super()` is python's own
    // `Context.super` / `BlockReference`, so parent lookup, arity and error
    // messages are byte-identical by construction.  The code generator's
    // compile-time flags (`extends_so_far`, `has_known_extends`, the frame
    // toplevel/rootlevel pair) are mirrored over the *executed* traversal: a
    // branch that never runs contributes nothing observable, so counting only
    // executed extends matches codegen wherever behaviour can differ.

    /// UndeclaredNameVisitor-style detection of `self` / `super` inside one
    /// function scope (a root body excluding blocks, or a block's own body):
    /// a load marks the name as accessed, any other occurrence drops it from
    /// further consideration afterwards -- python's discard behaviour.  The
    /// walk stops at nested `{% block %}` nodes like `visit_Block` does.
    fn undecl_specials(&self, items: &[Av]) -> (bool, bool) {
        let mut track = [true, true];
        let mut found = [false, false];
        for it in items {
            self.spec2_walk(it, &mut track, &mut found);
        }
        (found[0], found[1])
    }

    fn spec2_walk(&self, av: &Av, track: &mut [bool; 2], found: &mut [bool; 2]) {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.spec2_walk(a, track, found);
                }
            }
            Av::Node("Block", _, _) => {}
            Av::Node("Name", _, a) => {
                if a.len() >= 2 {
                    let Some(n) = arg_str(&a[0]) else { return };
                    for (i, special) in ["self", "super"].iter().enumerate() {
                        if n == *special && track[i] {
                            if arg_str(&a[1]) == Some("load") {
                                found[i] = true;
                            } else {
                                track[i] = false;
                            }
                        }
                    }
                }
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.spec2_walk(x, track, found);
                }
            }
            _ => {}
        }
    }

    /// Collect every `{% block %}` node below `av` (codegen's
    /// `find_all(nodes.Block)`), including blocks nested inside other bodies.
    /// Collect block definitions in document order while tracking the code
    /// generator's compile-time eval-context state (slice C-v6): python compiles
    /// each block function inline during its traversal of the module, so a
    /// `{% autoescape %}` tag preceding it leaves its mark on that wrapper.  The
    /// scan mirrors `visit_ScopedEvalContextModifier` exactly: options fold
    /// against the evolving shared state (success sets the attribute value's
    /// truthiness, failure pins volatile), and save/revert restores around the
    /// body so nothing leaks past the region.
    fn collect_blocks(
        &self,
        av: &Av,
        st: &mut AeState,
        out: &mut Vec<(String, Vec<Av>, bool, AeState)>,
    ) -> Result<(), EvalErr> {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.collect_blocks(a, st, out)?;
                }
            }
            // the block records the state at its position before descending.
            Av::Node("Block", _, a) => {
                let nm_opt = match a.first() { Some(x) => arg_str(x), None => None };
                if let Some(nm) = nm_opt {
                    let req = matches!(a.get(3), Some(Av::Bool(true)));
                    if let Some(Av::List(body)) = a.get(1) {
                        out.push((nm.to_string(), body.clone(), req, *st));
                    }
                }
                self.collect_blocks_children(av, st, out)?;
            }
            Av::Node("ScopedEvalContextModifier", _, a) => {
                let saved = *st;
                if let Some(Av::List(opts)) = a.first() {
                    for kw in opts {
                        let (kn, ka) = match parts(kw) {
                            Some(x) => x,
                            None => continue,
                        };
                        if kn != "Keyword" || ka.len() < 2 {
                            continue;
                        }
                        match self.fold(&ka[1], st.vol, st.ae)? {
                            Some(c) => st.ae = c.is_truthy()?,
                            None => st.vol = true,
                        }
                    }
                }
                if let Some(Av::List(body)) = a.get(1) {
                    for b in body {
                        self.collect_blocks(b, st, out)?;
                    }
                }
                *st = saved;
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.collect_blocks(x, st, out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn collect_blocks_children(
        &self,
        av: &Av,
        st: &mut AeState,
        out: &mut Vec<(String, Vec<Av>, bool, AeState)>,
    ) -> Result<(), EvalErr> {
        if let Av::Node(_, _, a) = av {
            for x in a {
                self.collect_blocks(x, st, out)?;
            }
        } else if let Av::List(inner) | Av::Tuple(inner) = av {
            for x in inner {
                self.collect_blocks(x, st, out)?;
            }
        }
        Ok(())
    }

    /// Reject constructs whose compiled counterparts cannot be reproduced: an
    /// `{% extends %}` inside a closure frame (macro / call block / another
    /// block body) is a compile-time failure in python and stays unsupported.
    fn check_layout(&self, av: &Av, toplevel_ok: bool) -> Result<(), EvalErr> {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for x in inner {
                    self.check_layout(x, toplevel_ok)?;
                }
            }
            // the root body region starts with a toplevel frame.
            Av::Node("Template", _, a) => {
                for x in a {
                    self.check_layout(x, true)?;
                }
            }
            Av::Node("Extends", _, _) => {
                if !toplevel_ok {
                    return Err(nope(
                        "extends below a non-toplevel frame (python fails with \"cannot \
                         use extend from a non top-level scope\")",
                    ));
                }
            }
            // soft frames from `if` keep toplevel; every other construct loses it.
            Av::Node("If", _, a) => {
                for x in a {
                    self.check_layout(x, toplevel_ok)?;
                }
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.check_layout(x, false)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Prepare one parsed template into a unit: create its block functions in
    /// document order and reject duplicate names within the same template (the
    /// code generator's `block 'x' defined twice` compile-time failure).
    fn prepare_unit(
        &self,
        tree: &Av,
        tname: Option<String>,
    ) -> Result<Rc<UnitInfo>, EvalErr> {
        // Block functions are compiled *after* the whole root traversal has reverted every scoped eval-context change (python creates `Frame(eval_ctx)` from the restored shared object): their wrappers therefore follow the module-start defaults, never an in-region state.
        let mut found: Vec<(String, Vec<Av>, bool, AeState)> = Vec::new();
        // every module starts from the environment's static settings (python
        // creates a fresh generator with a fresh eval context per template).
        let mut istate = AeState { vol: false, ae: self.s.autoescape };
        self.collect_blocks(tree, &mut istate, &mut found)?;
        let any_extends = has_node_named(tree, "Extends");
        let mut names: Vec<String> = Vec::new();
        let mut funcs: Vec<Py<PyAny>> = Vec::new();
        for (nm, body, _req, _) in found {
            if names.iter().any(|n| n == &nm) {
                return Err(nope(format!("block {:?} defined twice", nm)));
            }
            let (us_self, us_super) = self.undecl_specials(&body);
            let id = {
                let mut n = self.s.next_blk.borrow_mut();
                *n += 1;
                *n
            };
            let func: Py<PyAny> = Py::new(
                self.py,
                BlockFunc {
                    session: self.s.clone(),
                    // module-start defaults for every block function.
                    aestate: AeState { vol: false, ae: self.s.autoescape },
                    tname: tname.clone(),
                    name: nm.to_string(),
                    body,
                    uses_super: us_super,
                    uses_self: us_self,
                    id,
                },
            )?
            .into_any();
            self.s.blocks_reg.borrow_mut().insert(id, func.clone());
            names.push(nm.to_string());
            funcs.push(func);
        }
        Ok(Rc::new(UnitInfo {
            tree: tree.clone(),
            name: tname,
            names,
            funcs,
            any_extends,
        }))
    }

    /// Register a unit's own block functions into `context.blocks`.  The top
    /// template starts from `{k: [v]}` (python's `Context.__init__`), every
    /// extend appends the parent's entries at the end of each list.
    fn register_blocks(&self, unit: &UnitInfo, append_last: bool) -> Result<(), PyErr> {
        let dicts = self.ctx().getattr("blocks")?;
        for (name, f) in unit.names.iter().zip(unit.funcs.iter()) {
            let existing = self.dict_get(&dicts, name)?;
            if append_last && !existing.is_none() {
                existing.call_method1("append", (f,))?;
            } else {
                let nl = PyList::empty(self.py);
                nl.append(f)?;
                dicts.set_item(name.as_str(), &nl)?;
            }
        }
        Ok(())
    }

    /// Load, parse and prepare a parent template reached through `{% extends %}`.
    fn get_unit(&self, name: &str) -> Result<Rc<UnitInfo>, EvalErr> {
        if let Some(u) = self.s.units.borrow().get(name).cloned() {
            return Ok(u);
        }
        let toks = match self.s.srcs.get(name) {
            Some(Some(t)) => t.clone(),
            _ => return Err(nope(format!("extends source {:?} not provided", name))),
        };
        let mut rp = RtParser::new(toks, None);
        let tree = match rp.parse() {
            Ok(t) => t,
            Err(_) => return Err(nope("parent parse error")),
        };
        self.scan(&tree)?;
        self.check_layout(&tree, true)?;
        let unit = self.prepare_unit(&tree, Some(name.to_string()))?;
        self.s.units.borrow_mut().insert(name.to_string(), unit.clone());
        Ok(unit)
    }

    /// `{% extends %}`: raise `extended multiple times` exactly where the code
    /// generator writes it -- unconditionally after a known top-level extend,
    /// otherwise behind its runtime guard -- then load and append.
    fn exec_extends(&self, args: &[Av]) -> ERes<()> {
        if self.ext_stack.borrow().is_empty() {
            return Err(nope("extends outside a root region"));
        }
        let f = *self.flags.borrow();
        if f.known_top {
            return Err(EvalErr::Exc(PyErr::new::<TemplateRuntimeError, _>(
                "extended multiple times",
            )));
        }
        if f.chain_top && f.seen_any {
            let already: bool = {
                let st = self.ext_stack.borrow();
                match st.last() { Some(re) => *re.extended.borrow(), None => false }
            };
            if already {
                return Err(EvalErr::Exc(PyErr::new::<TemplateRuntimeError, _>(
                    "extended multiple times",
                )));
            }
        }
        let val = self.node(&args[0])?;
        let tname: String = match val.extract::<String>() {
            Ok(s) => s,
            Err(_) => return Err(nope("non-string extends target")),
        };
        let unit = self.get_unit(&tname)?;
        self.register_blocks(&unit, true)?;
        {
            let mut nf = *self.flags.borrow();
            nf.seen_any = true;
            if f.chain_root {
                nf.known_top = true;
            }
            *self.flags.borrow_mut() = nf;
        }
        {
            let st = self.ext_stack.borrow();
            if let Some(re) = st.last() {
                re.extended.replace(true);
                *re.parent.borrow_mut() = Some(unit);
            }
        }
        Ok(())
    }

    /// A block invocation.  Codegen drops it entirely after a known top-level
    /// extend, guards it when the frame chain kept `toplevel` and extends were
    /// seen before, and always calls index 0 of the registry: the most derived
    /// override registered so far.
    fn exec_block(&self, args: &[Av], out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        if args.len() < 4 {
            return Err(nope("bad block node"));
        }
        let Some(name) = arg_str(&args[0]) else { return Err(nope("block name")) };
        if !self.ext_stack.borrow().is_empty() {
            // python only consults the codegen flags while the frame chain is still toplevel: inner frames (scopes, loops...) keep their own silent block invocations.
            let f = *self.flags.borrow();
            if f.chain_top && f.known_top {
                return Ok(());
            }
            if f.chain_top && f.seen_any {
                let st = self.ext_stack.borrow();
                let already = match st.last() { Some(re) => *re.extended.borrow(), None => false };
                drop(st);
                if already {
                    return Ok(());
                }
            }
        }
        // required blocks must have been overridden at least once (the check is
        // emitted before the invocation, wherever the node was visited -- and
        // it reads the *current* context object even for a scoped block: the
        // generator splices the derived expression only into the call).
        let req = matches!(&args[3], Av::Bool(true));
        let dicts = self.ctx().getattr("blocks")?;
        let lst = self.dict_get(&dicts, name)?;
        if lst.is_none() {
            if req {
                return Err(EvalErr::Exc(self.required_block_error(name)));
            }
            return Err(nope(format!("no function registered for block {:?}", name)));
        }
        let n: usize = lst.len()?;
        if req && n <= 1 {
            return Err(EvalErr::Exc(self.required_block_error(name)));
        }
        let item0: Bound<'py, PyAny> = {
            let mut seq: Vec<Bound<'py, PyAny>> = Vec::new();
            {
                let iter = lst.try_iter()?;
                for item in iter {
                    seq.push(item?);
                }
            }
            match seq.first() { Some(x) => x.clone(), None => return Err(nope("empty block list")) }
        };
        // a scoped invocation passes `context.derived(locals)` evaluated at the
        // call site: python's own Context.derived merges them over the current
        // vars, copies the blocks lists and shares the eval context.
        let arg: Bound<'py, PyAny> = if matches!(args.get(2), Some(Av::Bool(true))) {
            let locals = self.dump_locals()?;
            self.ctx().call_method1("derived", (&locals,))?
        } else {
            self.ctx()
        };
        let rv = item0.call1((&arg,))?;
        for p in rv.try_iter()? {
            out.push(p?);
        }
        Ok(())
    }

    fn required_block_error(&self, name: &str) -> PyErr {
        // python's codegen embeds the f-string repr of the block name.
        let builtins = self.py.import("builtins").unwrap();
        let rep_fn = builtins.getattr("repr").unwrap();
        let nmobj = PyString::new(self.py, name);
        let rep: String = rep_fn.call1((&nmobj,)).unwrap().extract().unwrap();
        PyErr::new::<TemplateRuntimeError, _>(format!(
            "Required block {} not found",
            rep
        ))
    }

    /// Run one template unit as a root region (its generated root function),
    /// making that module's own name the current one while it renders.
    fn exec_unit(&self, unit: &Rc<UnitInfo>, out: &mut Vec<Bound<'py, PyAny>>) -> ERes<()> {
        let saved = self.tname.borrow().clone();
        *self.tname.borrow_mut() = unit.name.clone();
        // every template module is compiled by its own code generator with a
        // fresh eval context of the environment's static settings; rendering a
        // parent (or nested) unit therefore starts from that state, not from
        // whatever region the caller interpreter happens to be in.
        let saved_ae = *self.aestate.borrow();
        *self.aestate.borrow_mut() = AeState { vol: false, ae: self.s.autoescape };
        let res = self.exec_unit_inner(unit, out);
        *self.aestate.borrow_mut() = saved_ae;
        *self.tname.borrow_mut() = saved;
        res
    }

    fn exec_unit_inner(
        &self,
        unit: &Rc<UnitInfo>,
        out: &mut Vec<Bound<'py, PyAny>>,
    ) -> ERes<()> {
        let items = match &unit.tree {
            Av::Node("Template", _, a) => match a.first() {
                Some(Av::List(l)) => l,
                _ => return Err(nope("bad template body")),
            },
            _ => return Err(nope("template node expected")),
        };
        // Root-frame symbol analysis (walk_fsf + build_kinds + the `self`
        // undeclared walk) is a pure function of the immutable tree, so it is
        // computed once per tree and cached by `unit_prep` (warm-path pass);
        // on a cache miss this reproduces exactly what ran here before.
        let prep = self.unit_prep(&unit.tree)?;
        let mut kinds: HashMap<String, IK> = HashMap::with_capacity(prep.root_kinds.len());
        for (k, v) in &prep.root_kinds {
            kinds.insert(k.clone(), *v);
        }
        let uses_self = prep.uses_self;
        let re = Rc::new(RootExec { any_extends: unit.any_extends, extended: RefCell::new(false), parent: RefCell::new(None) });
        self.enter_boundary_scope(&kinds)?;
        if uses_self {
            let trefcls = self.rt().getattr("TemplateReference")?;
            let obj = trefcls.call1((self.ctx(),))?;
            if let Some(frame) = self.stack.borrow().last().cloned() {
                frame.borrow_mut().slots.insert("self".to_string(), Some(obj.unbind()));
            }
        }
        self.ext_stack.borrow_mut().push(re.clone());
        // the generator counts every visited node regardless of runtime branch;
        // only executed branches matter for what is observable, so counting at
        // execution time reproduces it exactly (see slice notes).
        let mut res: ERes<()> = Ok(());
        for it in items {
            // every direct child is visited at the root frame itself.
            let mut nf = *self.flags.borrow();
            nf.chain_top = true;
            nf.chain_root = true;
            *self.flags.borrow_mut() = nf;
            res = self.exec_one(it, out);
            if res.is_err() {
                break;
            }
        }
        // the parent root renders after this body finished (codegen appends the
        // `yield from` at the end of the root function).
        if res.is_ok() && *re.extended.borrow() {
            let par = re.parent.borrow().clone();
            if let Some(pu) = par {
                self.flags.replace(BlkFlags::default());
                res = self.exec_unit(&pu, out);
            }
        }
        self.ext_stack.borrow_mut().pop();
        self.pop_scope();
        #[cfg(feature = "prof")]
        crate::prof::flush("unit");
        res
    }

    // --------------------------------------------------------------- C-v5 --
    //
    // Includes and imports.  Loading stays python-side exactly like codegen:
    // every extra template is fetched through the environment's own lookup
    // function with this module's name as parent, so loaders (`join_path`,
    // caches) keep working untouched; included bodies render through python's
    // compiled engine (`root_render_func` / `_body_stream`) and imported ones
    // through its real `TemplateModule` objects.  What rust reproduces is the
    // orchestration around those calls: which lookup function an `{% include %}`
    // takes, the ignore-missing guard, the with/without-context split (contexts
    // built by python's own `new_context`, `make_module` and
    // `_get_default_module`, locals dumped through the C-v4b machinery), and
    // the store plus exported_vars rules of named imports.

    /// `{% include %}`: pick the lookup function statically exactly like
    /// `CodeGenerator.visit_Include` -- a constant string goes to
    /// `get_template`, a literal sequence (python's optimizer folds those into
    /// a `Const` carrying a tuple/list value, which takes the same branch) to
    /// `select_template`, everything else to `get_or_select_template`.  The
    /// last one dispatches on the runtime value, so an unoptimized tree ends up
    /// making exactly the call python's optimized one does.
    fn exec_include(
        &self,
        args: &[Av],
        out: &mut Vec<Bound<'py, PyAny>>,
    ) -> ERes<()> {
        if args.len() < 3 {
            return Err(nope("bad include node"));
        }
        let func = include_lookup(&args[0]);
        let val = self.node(&args[0])?;
        // codegen passes this module's name as the loader parent argument.
        let parent: Option<String> = self.cur_name();
        let ignore_missing = matches!(&args[2], Av::Bool(true));
        let template: Bound<'py, PyAny> = match self.env().call_method(func, (&val, parent), None) {
            Ok(t) => t,
            Err(e) => {
                // `except TemplateNotFound: pass` -- only the lookup sits
                // inside the try, so render-time errors always propagate.
                if ignore_missing && e.is_instance_of::<TemplateNotFound>(self.py) {
                    return Ok(());
                }
                return Err(EvalErr::Exc(e));
            }
        };
        if matches!(&args[1], Av::Bool(true)) {
            // with context: `template.root_render_func(template.new_context(
            // context.get_all(), True, dump_local_context(frame)))`
            let all = self.ctx().call_method("get_all", (), None)?;
            let locals = self.dump_locals()?;
            let sharedv = true.into_bound_py_any(self.py)?;
            let newctx = template.call_method("new_context", (&all, &sharedv, &locals), None)?;
            let gen = template.call_method("root_render_func", (&newctx,), None)?;
            for p in gen.try_iter()? {
                out.push(p?);
            }
        } else {
            // without context: `yield from
            // template._get_default_module()._body_stream` -- no argument,
            // so the module renders from globals alone and python caches it on
            // the `Template` object across renders exactly as compiled engines
            // do.
            let module = template.call_method("_get_default_module", (), None)?;
            let body = module.getattr("_body_stream")?;
            for p in body.try_iter()? {
                out.push(p?);
            }
        }
        Ok(())
    }

    /// `_import_common`: always `environment.get_template` (codegen never
    /// selects here), then build the module object: a fresh one through
    /// `make_module` when the current context flows in, otherwise the cached
    /// default module -- *with* this context passed along so newly appearing
    /// globals reach it.
    fn import_common(&self, av: &Av, with_context: bool) -> ERes<Bound<'py, PyAny>> {
        let val = self.node(av)?;
        let parent: Option<String> = self.cur_name();
        let tpl: Bound<'py, PyAny> = self.env().call_method("get_template", (&val, parent), None)?;
        if with_context {
            let all = self.ctx().call_method("get_all", (), None)?;
            let locals = self.dump_locals()?;
            let sharedv = true.into_bound_py_any(self.py)?;
            Ok(tpl.call_method("make_module", (&all, &sharedv, &locals), None)?)
        } else {
            let ctxv = self.ctx();
            Ok(tpl.call_method1("_get_default_module", (&ctxv,))?)
        }
    }

    /// `{% import ... as target %}`: store rules mirror `visit_Import` -- the
    /// slot store always happens; at toplevel additionally a direct write into
    /// `context.vars`, and dropping a public name from `exported_vars` (an
    /// imported module never re-exports).
    fn exec_import(&self, args: &[Av]) -> ERes<()> {
        if args.len() < 3 {
            return Err(nope("bad import node"));
        }
        let target = match arg_str(&args[1]) {
            Some(s) => s,
            None => return Err(nope("import target")),
        };
        let module = self.import_common(&args[0], matches!(&args[2], Av::Bool(true)))?;
        self.store_owned(target, &module)?;
        if self.stack.borrow().len() == 1 && !self.local_only {
            let vars = self.ctx().getattr("vars")?;
            vars.set_item(target, &module)?;
            if !target.starts_with('_') {
                let ex = self.ctx().getattr("exported_vars")?;
                ex.call_method1("discard", (target,))?;
            }
        }
        Ok(())
    }

    /// `{% from ... import a, b as c %}`: same module path, then per name
    /// python's own `getattr(included_template, name, missing)` falling back to
    /// an undefined built by the environment with codegen's exact message --
    /// which quotes the *module* name and the original attribute name, not the
    /// alias.  Slot stores happen per name immediately; at toplevel the values
    /// then go into `context.vars` as one batch and public aliases are dropped
    /// from exported_vars (`visit_FromImport`).
    fn exec_from_import(
        &self,
        args: &[Av],
        lineno: Option<u32>,
    ) -> ERes<()> {
        if args.len() < 3 {
            return Err(nope("bad from-import node"));
        }
        let names = match &args[1] {
            Av::List(l) => l,
            _ => return Err(nope("bad from-import names")),
        };
        // codegen loads the module before touching any name.
        let module = self.import_common(&args[0], matches!(&args[2], Av::Bool(true)))?;
        let miss = self.missing();
        let builtins = self.py.import("builtins")?;
        let getattr_fn = builtins.getattr("getattr")?;
        let mut entries: Vec<(String, Bound<'py, PyAny>)> = Vec::new();
        for nm in names.iter() {
            let (orig, alias): (String, String) = match nm {
                Av::Str(s) => (s.clone(), s.clone()),
                Av::Tuple(t) if t.len() >= 2 => match (arg_str(&t[0]), arg_str(&t[1])) {
                    (Some(a), Some(b)) => (a.to_string(), b.to_string()),
                    _ => return Err(nope("bad import name")),
                },
                _ => return Err(nope("bad import name")),
            };
            let got: Bound<'py, PyAny> = getattr_fn.call((&module, orig.as_str(), &miss), None)?;
            let value: Bound<'py, PyAny> = if got.is(&miss) {
                // `included_template.__name__` -- the module object carries it,
                // never the template's own name attribute.
                let modrepr = self.py_repr(&module.getattr("__name__")?)?;
                let pos = self.position(lineno);
                let msg = format!(
                    "the template {} (imported on {}) does not export the requested name {}",
                    modrepr,
                    pos,
                    py_repr_str(orig.as_str())
                );
                let kw = PyDict::new(self.py);
                kw.set_item("name", orig.as_str())?;
                self.undef().call((msg.as_str(),), Some(&kw))?
            } else {
                got
            };
            self.store_owned(&alias, &value)?;
            entries.push((alias, value));
        }
        if !entries.is_empty() && self.stack.borrow().len() == 1 && !self.local_only {
            let vars = self.ctx().getattr("vars")?;
            let ex = self.ctx().getattr("exported_vars")?;
            for (alias, v) in entries.iter() {
                vars.set_item(alias.as_str(), v)?;
                if !alias.starts_with('_') {
                    ex.call_method1("discard", (alias.as_str(),))?;
                }
            }
        }
        Ok(())
    }

    /// Store into the nearest owning slot for a bare name -- codegen writes to
    /// `frame.symbols.ref(name)`, which resolves through the symbol chain.
    fn store_owned(&self, name: &str, val: &Bound<'py, PyAny>) -> ERes<()> {
        let stored = {
            let frames = self.stack.borrow();
            let mut done = false;
            for frame in frames.iter().rev() {
                let mut sc = frame.borrow_mut();
                if sc.slots.contains_key(name) {
                    sc.slots.insert(name.to_string(), Some(val.clone().unbind()));
                    done = true;
                    break;
                }
            }
            done
        };
        if !stored {
            return Err(nope(format!("store to unowned name {:?}", name)));
        }
        Ok(())
    }

    /// `repr()` through python so f-string `!r` semantics carry over exactly.
    fn py_repr(&self, obj: &Bound<'py, PyAny>) -> ERes<String> {
        let builtins = self.py.import("builtins")?;
        let rep_fn = builtins.getattr("repr")?;
        Ok(rep_fn.call1((obj,))?.extract::<String>()?)
    }

    /// `CodeGenerator.position`: "line N" plus the module name when known.
    fn position(&self, lineno: Option<u32>) -> String {
        let mut pos = match lineno {
            Some(l) => format!("line {}", l),
            None => "unknown line".to_string(),
        };
        if let Some(nm) = self.cur_name() {
            pos = format!("{} in {}", pos, py_repr_str(nm.as_str()));
        }
        pos
    }

    fn spec1_walk(&self, av: &Av, track: &mut [bool; 1], found: &mut [bool; 1]) {
        match av {
            Av::List(inner) | Av::Tuple(inner) => {
                for a in inner {
                    self.spec1_walk(a, track, found);
                }
            }
            Av::Node("Block", _, _) => {}
            Av::Node("Name", _, a) => {
                if a.len() >= 2 {
                    let Some(n) = arg_str(&a[0]) else { return };
                    if n == "self" && track[0] {
                        if arg_str(&a[1]) == Some("load") {
                            found[0] = true;
                        } else {
                            track[0] = false;
                        }
                    }
                }
            }
            Av::Node(_, _, a) => {
                for x in a {
                    self.spec1_walk(x, track, found);
                }
            }
            _ => {}
        }
    }


}

/// The template-lookup function codegen picks for an `{% include %}` target
/// expression (slice C-v5); see `Ev::exec_include`.
pub fn include_lookup(av: &Av) -> &'static str {
    match av {
        Av::Node("Const", _, a) => match a.first() {
            Some(Av::Str(_)) => "get_template",
            _ => "get_or_select_template",
        },
        Av::Node("Tuple" | "List", _, _) => "select_template",
        _ => "get_or_select_template",
    }
}

fn body_of(args: &[Av]) -> &[Av] {
    match &args[2] {
        Av::List(b) => b,
        _ => &[],
    }
}


/// True if a scoped `{% block %}` node appears anywhere below the nodes in
/// `args` (codegen's `any(block.scoped for block in node.find_all(nodes.Block))`
/// in visit_For).  Descends through everything, blocks included.
fn any_scoped(args: &[Av], found: &mut Option<bool>) {
    if *found == Some(true) {
        return;
    }
    for av in args {
        match av {
            Av::List(inner) | Av::Tuple(inner) => any_scoped(inner, found),
            Av::Node(name, _, a) => {
                if *name == "Block" && matches!(a.get(2), Some(Av::Bool(true))) {
                    *found = Some(true);
                    return;
                }
                any_scoped(a, found);
            }
            _ => {}
        }
    }
}

/// True if any node named `want` appears below `av` (`Node.find`).
fn has_node_named(av: &Av, want: &str) -> bool {
    match av {
        Av::List(inner) | Av::Tuple(inner) => inner.iter().any(|a| has_node_named(a, want)),
        Av::Node(name, _, a) => {
            *name == want || a.iter().any(|x| has_node_named(x, want))
        }
        _ => false,
    }
}

/// Rust callable backing one compiled block function (slice C-v4).  Compiled
/// block functions are module level and take the live context; they enter a
/// fresh frame initialized from their own symbol analysis (no ancestor scope),
/// bind `super` / `self` when accessed undeclared, render into parts and return
/// them as a list so that python's `concat` in `BlockReference.__call__` joins
/// the very same pieces.
#[pyclass(unsendable)]
pub struct BlockFunc {
    session: Rc<Session>,
    /// Compile-time eval-context state at this block's tree position, recorded
    /// by the mode scan over its module (slice C-v6): python compiles each
    /// block function inline during the generator traversal of that module.
    aestate: AeState,
    name: String,
    /// The template module this block function was compiled into (slice C-v5).
    tname: Option<String>,
    body: Vec<Av>,
    uses_super: bool,
    uses_self: bool,
    id: usize,
}

impl BlockFunc {
    /// Render this block function's body against the shared session context.
    fn render_parts<'py>(
        &self,
        py: Python<'py>,
        context: &Bound<'py, PyAny>,
    ) -> Result<Vec<Bound<'py, PyAny>>, EvalErr> {
        let ev = Ev::block_ev(
            py,
            self.session.clone(),
            context.clone().unbind(),
            self.tname.clone(),
            self.aestate,
        );
        let mut occs: Vec<Occ> = Vec::new();
        for it in &self.body {
            ev.walk_fsf(it, &mut occs);
        }
        // compiled block frames own an independent symbol table.
        let mut kinds = ev.build_kinds(&occs, &[]);
        if self.uses_super || self.uses_self {
            for n in ["super", "self"] {
                if (n == "super" && self.uses_super) || (n == "self" && self.uses_self) {
                    kinds.insert(n.to_string(), IK::Param);
                }
            }
        }
        ev.enter_boundary_scope(&kinds)?;
        if self.uses_super {
            let me = self
                .session
                .blocks_reg
                .borrow()
                .get(&self.id)
                .cloned()
                .ok_or_else(|| nope("block function not registered"))?;
            let sv: Bound<'_, PyAny> = ev.ctx().call_method1(
                "super",
                (PyString::new(py, self.name.as_str()), me.bind(py).as_any().clone()),
            )?;
            if let Some(frame) = ev.stack.borrow().last().cloned() {
                frame.borrow_mut().slots.insert("super".to_string(), Some(sv.unbind()));
            }
        }
        if self.uses_self {
            let trefcls = ev.rt().getattr("TemplateReference")?;
            let obj = trefcls.call1((ev.ctx(),))?;
            if let Some(frame) = ev.stack.borrow().last().cloned() {
                frame.borrow_mut().slots.insert("self".to_string(), Some(obj.unbind()));
            }
        }
        let mut buf: Vec<Bound<'_, PyAny>> = Vec::new();
        ev.exec_items(&self.body, &mut buf)?;
        Ok(buf)
    }
}

#[pymethods]
impl BlockFunc {
    /// Invoked with the calling context, either directly from a root region,
    /// through python's `BlockReference.__call__`, or as a scoped invocation
    /// that passes a freshly derived one; returns the finalized parts.
    #[pyo3(signature = (context))]
    fn __call__<'p>(
        &self,
        py: Python<'p>,
        context: &Bound<'p, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        match self.render_parts(py, context) {
            Ok(parts) => Ok(PyList::new(py, parts)?.into_any().unbind()),
            Err(EvalErr::Exc(e)) => Err(e),
            Err(EvalErr::Unsupported(u)) => Err(pyo3::exceptions::PyRuntimeError::new_err(
                format!("unsupported inside a block body: {u}"),
            )),
        }
    }

    fn __repr__(&self) -> String {
        format!("<BlockFunc {}>", self.name)
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
    evaluate_with_sources(py, av, ctx, rt, name, Default::default())
}

/// Evaluate a parsed template with extra parent sources for `{% extends %}`.
pub fn evaluate_with_sources<'py>(
    py: Python<'py>,
    av: &Av,
    ctx: Bound<'py, PyAny>,
    rt: Bound<'py, PyAny>,
    name: Option<String>,
    srcs: std::collections::HashMap<String, Option<Vec<PToken>>>,
) -> EvalOutcome<'py> {
    let ev = match Ev::new(py, ctx, rt, name, srcs) {
        Ok(e) => e,
        Err(EvalErr::Exc(e)) => return EvalOutcome::Exc(e),
        Err(EvalErr::Unsupported(u)) => return EvalOutcome::Unsupported(u),
    };
    // Scan and layout check are pure functions of the immutable tree plus the
    // environment's static autoescape flag, so their verdicts are computed
    // once per tree and cached (warm-path pass); on a miss this reproduces
    // exactly what ran here before, in the same order.
    let prep = match ev.unit_prep(av) {
        Ok(p) => p,
        Err(EvalErr::Unsupported(u)) => return EvalOutcome::Unsupported(u),
        Err(EvalErr::Exc(e)) => return EvalOutcome::Exc(e),
    };
    if !prep.scan_ok {
        return EvalOutcome::Unsupported(prep.scan_reason.unwrap_or_else(|| "scan".into()));
    }
    if !prep.layout_ok {
        return EvalOutcome::Unsupported(
            prep.layout_reason.unwrap_or_else(|| "layout check".into()),
        );
    }
    // prepare and register the top template's own blocks before anything runs.
    let unit = match ev.prepare_unit(av, ev.sname()) {
        Ok(u) => u,
        Err(EvalErr::Unsupported(u)) => return EvalOutcome::Unsupported(u),
        Err(EvalErr::Exc(x)) => return EvalOutcome::Exc(x),
    };
    if let Err(x) = ev.register_blocks(&unit, false) {
        return EvalOutcome::Exc(x);
    }
    let mut out: Vec<Bound<'py, PyAny>> = Vec::new();
    match ev.exec_unit(&unit, &mut out) {
        Ok(()) => match ev.out_join(out) {
            Ok(v) => EvalOutcome::Rendered(v),
            Err(EvalErr::Exc(e)) => EvalOutcome::Exc(e),
            Err(EvalErr::Unsupported(u)) => EvalOutcome::Unsupported(u),
        },
        Err(EvalErr::Exc(e)) => EvalOutcome::Exc(e),
        Err(EvalErr::Unsupported(u)) => EvalOutcome::Unsupported(u),
    }
}

/// Outcome of the C-v8a serve decision: whether a template may be served from
/// rust, and if not, why.  `Supported` means the whole-tree scan and layout
/// checks pass *and* the template reaches no other template (no extends,
/// include or import nodes), which is what makes the verdict side-effect-free:
/// with no cross-template references, evaluation cannot decline part-way
/// through a render (slice C-v5's lookups are the only mid-exec unsupported
/// sources), so a cached per-template verdict never lies.  Closing that gap --
/// serving referenced templates from rust too -- is slice C-v8b.
pub enum ServeVerdict {
    Supported,
    Refused(String),
}

/// Decide whether ``av`` may be served from rust against a live context, by
/// running only the compile-time analyses (never executing).  A parse outcome
/// that is itself an error counts as refused: the caller falls back to python
/// codegen, which surfaces the real exception.
pub fn check_supported<'py>(
    py: Python<'py>,
    av: &Av,
    ctx: Bound<'py, PyAny>,
    rt: Bound<'py, PyAny>,
    name: Option<String>,
) -> ServeVerdict {
    let ev = match Ev::new(py, ctx, rt, name, Default::default()) {
        Ok(e) => e,
        Err(EvalErr::Exc(_)) => return ServeVerdict::Refused("environment guards".into()),
        Err(EvalErr::Unsupported(u)) => return ServeVerdict::Refused(u),
    };
    if let Err(e) = ev.scan(av) {
        return match e {
            EvalErr::Unsupported(u) => ServeVerdict::Refused(u),
            EvalErr::Exc(_) => ServeVerdict::Refused("analysis error".into()),
        };
    }
    if let Err(e) = ev.check_layout(av, true) {
        return match e {
            EvalErr::Unsupported(u) => ServeVerdict::Refused(u),
            EvalErr::Exc(_) => ServeVerdict::Refused("analysis error".into()),
        };
    }
    for want in ["Extends", "Include", "Import", "FromImport"] {
        if has_node_named(av, want) {
            return ServeVerdict::Refused(format!("{want} node (cross-template)").into());
        }
    }
    ServeVerdict::Supported
}


