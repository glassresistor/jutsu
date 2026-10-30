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
* [ ] C-v2: statements: print/set(+block)/if/for (incl. recursive + LoopContext
      features cycle/first/last/revidx/depth/length/parent), with/autoescape
      scope modifiers, block/include/import/from_import semantics via the
      python template machinery (loaders stay python).
* [ ] C-v3: macro, filter blocks, call blocks/caller, namespace objects.
* [ ] C-v4: block inheritance + super(); `loop` in nested templates; error
      propagation parity (`TemplateRuntimeError`, tracebacks via debug.py --
      likely keep python-side wrapping).
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
