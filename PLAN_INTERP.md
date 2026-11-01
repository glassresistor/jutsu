# Plan: porting jinja2's runtime/interpreter to Rust (Phase C)

Follows the same methodology as the lexer and parser ports: keep the original
implementation reachable, validate differentially, only flip defaults once
parity is proven.

## Why a tree-walking rust interpreter (strategy decision, TODO Phase C.1)

The compiler port has two candidate strategies:

1. Emit python bytecode strings from rust (mirrors `codegen.py`).  Rejected:
   requires reproducing CPython codegen quirks exactly and still executes in
   CPython; no wasm story; filters/tests remain python callables anyway.
2. Tree-walking evaluator in rust (`eval.rs`) consuming the rust parser's
   `Av` AST directly -- chosen.  Key evidence from the parser benchmarks:
   rebuilding python node objects costs more than the whole rust parse
   (rust core parse is ~5x faster than the python parser, but parse+rebuild
   only wins ~1.4x).  Evaluating on the rust AST removes the rebuild entirely
   and lets the interpreter share the value model (`Av`/opaque pyobjects)
   with the parser -- no codegen surface at all, which is essential for wasm
   (Phase E).

## Validation strategy (identical shape to the parser port)

1. **Differential rendering**: every template rendered through the python
   engine and the rust evaluator, compared byte-for-byte including exception
   messages; repo fixtures + structured fuzz corpus (`tests/gen_templates.py`).
2. **Full suite**: once parity holds, flip `Template.render` to prefer the
   rust evaluator (flag `JUTSU_RUST_EVAL`, default on when no extensions are
   active), run the original suite unchanged.

## Slice plan (each slice lands green and committed)

* [x] C-v0: evaluate Template/Output/TemplateData/Const/Name against a plain
      python mapping; undefined renders empty like jinja's default Undefined;
      everything else reports "unsupported" so callers fall back.  Validated:
      fixed cases + unsupported list + structured cross-validation against
      `Environment.from_string(...).render()` on the generated corpus.
      Entry point `_jutsu.eval_template(tokens, lexerr, ctx)` parses and
      evaluates without ever serializing the AST to python.
* [x] C-v1: expression evaluation: arithmetic, chained comparisons, concat
      (`~`), `in`/`not in`, and-or-not-if-expr, tuple/list/dict literals,
      subscripts/slices, attribute access, calls with kwargs, filter/test
      expressions via the python-side filters/tests.  Runtime exceptions are
      propagated as the original python exception objects; parse errors get
      the same `translated` treatment `Environment.compile` applies.
      Deferred to later slices: dynamic call arguments (`*a`/`**k`),
      autoescape + Markup (including the non-markup-aware constant folding of
      `Concat.as_const`), environment finalize functions, volatile eval
      contexts -- all report unsupported so callers fall back.
* [X] C-v2: statements implemented -- print, set / set-block (implicit filter
      target), if/elif/else, for incl. lazy test-filters, else branches,
      extended loops (live `LoopContext`) and recursive loops (`loop()` renders
      the next level in rust), with; loop features cycle/first/last/revidx/
      depth/length/previtem/nextitem/changed via the real python LoopContext;
      namespace refs.  Scope model: slot frames per hard scope with
      first-occurrence init kinds (PARAM / ALIAS copy / RESOLVE / UNDEFINED),
      root-level writeback into `context.vars` + exported_vars, stores inside
      loops/blocks stay local; tuple targets unpack with python semantics and
      messages.  Whole-tree pre-evaluation scan reproduces the code generator's
      compile-time failures (unsupported nodes anywhere, missing filter/test
      names, `loop` assignment in any for-subtree).  Deferred to C-v3/C-v4:
      autoescape scope modifiers (`{% autoescape %}` -> Scope), macros,
      filter/call blocks, block/include/import/from_import.
      Validated: 40 fixed cases + 12k structured statement templates (3 seeds)
      with exception type/message parity; perf: rust parse+eval 129 us vs
      python compile+render 770 us cold, cached python render 6.2 us.
* [X] C-v3a: filter blocks implemented (buffered body in its own scope frame,
      implicit filter target receives the joined buffer, chain applied via
      `_filter_test_common`, result finalized with `str`); namespace refs were
      already covered.  Occurrence-collection order corrected to follow
      `analyze_node`: set-blocks analyze the target first, filter blocks the
      filter expression first, for loops analyze target+body before the test
      expression registers loads.
* [x] C-v3b: macros and call blocks implemented -- `runtime.Macro` wrapping a
      rust callable (`MacroFunc`), defining frame chain kept by `Rc` so
      closure cells stay alive and writable after the defining statement
      ended, define-time analysis mirroring `macro_body` + both
      `analyze_node` passes, implicit/explicit caller-kwargs-varargs,
      defaults evaluated in the macro frame, call blocks forwarding
      `caller=`.  Python codegen artifacts that only fail because the
      generated source is compiled (duplicate keyword arguments, duplicate
      macro parameters, explicit `caller=` in a call-block target) are
      reported unsupported rather than reproduced; the generator's
      dict-literal workaround path (python-keyword keys) renders normally
      and matches.  Validated: 33 differential cases plus 8 codegen-syntax
      fallback cases and 4 semantics locks, and 33k structured macro
      templates over eleven seeds, zero mismatches.
* [x] C-v4a: plain block inheritance + super()/self implemented and validated
      differentially (extends chains, codegen runtime guards mirrored
      dynamically, required blocks); scoped blocks, non-string extends targets
      and un-loadable parents fell back.
* [x] C-v4b: scoped blocks (`{% block x scoped %}`) -- derived eval contexts.
      Call-site locals dumped by merging the live scope frames top-down with
      boundary-stop at generated-function symbol chains (template roots, block
      functions), passed through python's own `Context.derived` which copies
      the blocks lists; the derived object flows onward through block function
      bodies, super()/self bindings, nested scoped invocations and macro-closure
      captures (`ctxobj`). Extended-loop forcing by any scoped block below a
      for node mirrored; test-filtered extended/recursive loops rejected at scan
      time (unsupported from inside a closure body could no longer become an
      engine switch). Still open: parent-side error parity instead of fallback,
      and the `loop` / error-propagation items. Error propagation parity
      (`TemplateRuntimeError`, tracebacks via debug.py -- likely keep
      python-side wrapping).
* [x] C-v5: includes and imports implemented.  Lookup function chosen exactly
      like codegen (literal string -> `get_template`, literal sequence ->
      `select_template`, anything else -> `get_or_select_template`), always with
      the current module's name as the loader parent argument; ignore-missing
      catches only `TemplateNotFound` and only around the lookup, so render-time
      errors propagate.  With-context renders through python's own
      `template.new_context(context.get_all(), True, dump_locals())` + root
      generator parts joining into the caller buffer -- reusing exactly the
      boundary-aware local dump of C-v4b; without context takes
      `_get_default_module()._body_stream` (no arguments, so python's module
      cache behaves as in compiled engines).  `{% import %}` builds a fresh
      `make_module(...)` or the cached default module *with* this context passed,
      then applies `visit_Import` store rules including dropping public aliases
      from exported_vars; named imports go through python's own attribute lookup
      with codegen's undefined message and per-name batch writeback.  A current-
      module name (`tname`) is threaded through the interpreter, macro closures,
      block functions and parent renderings so relative `join_path` resolution
      matches the compiled modules exactly.  Validation: 34 fixed differential
      cases plus a join-path suite (three templates discriminating which module
      a relative include resolves against) in `tests/test_rust_eval.py`, and a
      structured include/import fuzz corpus (`scripts/include_fuzz.py`, seeds
      1-3 x400): ok=643 exc=557 fallbacks=0 mismatches=0.  Macro / call-block
      wrappers are left out of that generator on purpose: python's own machinery
      swallows some render-time errors raised inside *invoked* macro bodies into
      unconsumed generator objects whose repr embeds an object address, so those
      shapes cannot be compared at all (fixed cases cover includes inside macro
      and caller bodies where nothing raises).
* [x] C-v6: autoescape/Markup/volatile integration.  The code generator's
      compile-time `EvalContext` is mirrored as a per-region state (`aestate`)
      beside the live eval context object; `{% autoescape %}` tags with a
      foldable option fix wrapper decisions for their region, non-foldable ones
      make it volatile and let generated runtime checks consult the *live*
      attribute.  Output children constant-fold through a rust `as_const`
      graph mirroring every python fold rule (`Concat.as_const` joins parts
      with plain `str()` -- `None` becomes 'None', filters get called),
      escaped per compile flag; volatile regions still fold (the default
      environment has a `finalize.const`) and only un-foldable children take
      the live check.  Static template data is never escaped: non-volatile
      frames return its value text, volatile ones wrap it in Markup before
      the outer escape for no-op.  Block functions always compile at
      module-start defaults because python emits them after the root
      traversal reverted every scoped tag; this slice also fixed a latent
      bug where nested silent block invocations after a known top-level
      extend were dropped -- python only consults the codegen flags while
      the frame chain is still toplevel, so scope-tagged blocks keep their
      own calls (a child template even renders an override *twice*, once
      through the inheritance chain and once at its position).  Call-block,
      filter-block and recursion results go into the caller buffer as raw
      objects (`start_write`/`end_write`, `return_buffer_contents`), Markup
      wrapping follows the compile flag except in volatile regions where a
      live check applies; set-block storage always uses the live check.
      Validated by two new parity suites (18 autoescape shapes incl. nested
      scoped tags, undefined options and filter/set blocks under live wraps,
      plus inheritance probes for the doubling quirk -- `scripts/probe_ae.py`,
      `scripts/probe_ae2.py`) folded into `tests/test_rust_eval.py`; include
      corpus seed 1 ok=211 exc=189 fb=0 mismatches=0, statement seeds 1-2,
      macro seed 1 x3k and block fuzz rerun green at zero mismatches, full
      python suite passes at 994 tests with the rust core active, `cargo
      test` unchanged at 17.  Remaining deferred: parent-side error parity
      instead of fallback, async environments falling out wholesale, and the
      performance pass.
* [ ] performance pass: measure against python-compiled baseline; only keep
      wins; watch pyo3 round-trips for attribute access and iteration.

## Value model notes

* `Av::Val(Arc<Py<PyAny>>)` already carries arbitrary-precision literals --
  the evaluator must never re-parse them.
* Undefined family (Chainable/Fast) stays python-side: rust returns a sentinel
  and lets python construct the right subclass; rendering of default undefined
  is empty string, `{{ x.y }}` on missing attr likewise yields undefined-like
  empties per jinja's semantics -- mirror exactly what `runtime.Undefined`
  does for operators (`__eq__`, `__len__`, boolean...).  This is the trickiest
  parity area; validate with dedicated fuzz.
* Markup/autoescape: check markupsafe object types through pyo3; join must be
  markup-aware when autoescape or `is_marked` (Concat node semantics).

## Files

    crates/jutsu-core/src/eval.rs       the evaluator
    src/jutsu/rust_eval.py              driver + fallback logic
    tests/test_rust_eval.py             differential render tests
