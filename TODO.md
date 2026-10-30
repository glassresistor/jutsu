# jutsu -- complete to-do list

Ordered by dependency.  `X` = done, `[ ]` = open.  Each item lists its exit
criterion.

## Phase A (complete) -- fork + rust lexer
* [X] fork jinja2 as `jutsu`, rename imports, original suite green
* [X] rust lexer in `crates/jutsu-core/src/lexer.rs`, wired into `Lexer.tokeniter`
* [X] differential fuzzing (~68k cases) + unit tests; maturin wheel packaging

## Phase B -- parser port (see PLAN_PARSER.md)
* [X] `parser.rs` token stream mirror (`current`, `look`, `skip`, `push`,
      `expect`, `next_if`, `skip_if`, `eos`) with exact error messages
* [X] expression grammar ported in full precedence chain, incl. tuple/list/dict,
      subscripts/slices, calls, filters, tests (`is`/`not`), postfix loops
* [X] statement grammar: all 13 keywords + call/filter blocks + assign targets
      (tuples, namespace refs) + macro signatures
* [X] `subparse` with data buffering and end-token stack bookkeeping
* [X] literal pass-through (`__val__`) so big ints survive round-trip
* [X] `ast_rebuild.py`: node registry from `fields`, rebuild + equality helpers
* [X] differential tests: repo templates, fuzz corpus (48k+ cases, 6 seeds),
      dump + lockstep tree/lineno walk parity; error-message parity incl.
      expanded ERR_CASES
* [X] flip default (`JUTSU_RUST_PARSER` on unless extensions contribute
      tags/streams/preprocessing; `=0` disables), full suite green with rust
      active (1109 parses through rust, 54 error paths, 51 python fallbacks);
      committed

## Phase C -- compiler port (largest chunk, see PLAN_INTERP.md)
* [X] strategy decided: tree-walking interpreter in rust consuming the rust
      parser's AST directly (`eval.rs`); python-bytecode emission rejected.
      Evidence: rebuild of python nodes costs more than the whole rust parse
      (rust parse ~5x python parser; parse+rebuild only ~1.4x), so evaluating
      on the rust AST is the only path with real headroom.
* [X] slice C-v0 (Template/Output/TemplateData/Const/Name) implemented in
      `crates/jutsu-core/src/eval.rs` + `src/jutsu/rust_eval.py`, differential
      against python rendering incl. structured corpus; not yet wired into
      `Template.render` (do that when the supported subset is large enough to
      matter).
* [X] slice C-v1 (expressions) implemented in `eval.rs`: arithmetic, chained
      comparisons with python short-circuit semantics, concat (`~`), tuples /
      lists / dicts, subscripts and slices, attribute access,
      calls via `Context.call`, filters/tests via
      `Environment._filter_test_common`, inline if-expressions incl. the
      implicit-else runtime `Undefined`, `and`/`or`/`not`, literals.
      Values are live python objects at every step, so operator behaviour on
      `Undefined` and raised exceptions match the compiled engine exactly.
      Validated: fixed cases plus a 6k-template structured expression corpus
      comparing rendered text *and* exception type/message (`tests/
      test_rust_eval.py`).  Deferred on purpose: dynamic call arguments
      (`*a`/`**k`) and autoescape/Markup (the code generator folds constant
      output children with the non-markup-aware `Concat.as_const`) both report
      unsupported; environment finalize functions and volatile eval contexts
      too.  Driver creates a real `Context` via `runtime.new_context`.
* [ ] `runtime.rs` mirrors of: Context/EvalContext/LoopContext/Macro/Namespace/
      Undefined (subclasses: ChainableUndefined/FastUndefined), missing sentinel,
      output buffering (`concat`, markup_join, str_join)
* [ ] autoescape semantics + `Markup` integration via markupsafe objects
* [ ] loop context features: cycle/first/last/length/revindex/depth; recursive
      loops; loop filters (`|sort` etc. stay python for now)
* [ ] block inheritance machinery (super(), blocks dict, scoped blocks,
      required blocks) ported onto the interpreter
* [ ] macro + filter-block semantics, `caller()` / CallBlock closures
* [ ] `with`/autoescape scope modifiers via ScopedEvalContextModifier
* [ ] performance pass: avoid pyo3 round-trips for hot attribute lookups where
      safe; measure against python-compiled baseline and only keep wins
* [ ] differential renderer validation: render every test template both ways,
      compare bytes + exceptions; fuzz too
* [ ] flip default once suite green; keep python codegen path behind flag

## Phase D -- extensions & ecosystem compatibility
* [ ] `ext.py` (loopcontrol, autoexcp, i18n/Translations) run on top of rust
      parser via the documented fallback rule (extension tags => python parse)
* [ ] document + test how third-party extension packages keep working; add a
      `jutsu.extensions.api` compat shim if needed
* [ ] loaders/bccache stay pure python (filesystem/zip/module cache -- no rust
      value); revisit only if profiling says otherwise
* [ ] `nativetypes.py` (NativeEnvironment) works through the interpreter path;
      add tests that it renders identically under rust core

## Phase E -- JavaScript / wasm packaging
* [ ] split pyo3 glue behind cargo feature `python`; core API becomes pure rust
      (`lexer::tokenize`, `parser::parse` -> AST enum, `render(cfg, ast, ctx)`)
* [ ] `jutsu-wasm` crate: wasm-bindgen wrapper exposing compile/render with a
      JS-side value bridge; minimal JS `Environment` equivalent (filters subset:
      upper/lower/join/length/default/escape...) -- full parity later
* [ ] node package (`@jutsu/core`) + web bundle from same crate; tests via
      wasm-pack test; document that python filters/extensions are NOT available
      in JS (parity caveat)

## Phase F -- packaging / release engineering
* [ ] CI matrix: build wheels for manylinux/macos/windows + sdist, run rust
      clippy + tests + python suite with and without core
* [ ] vendor the generated unicode tables check into CI (`gen_unicode_tables.py`)
* [ ] versioning story: keep jinja-compatible minor version (jutsu 3.1.x.y) so
      `pip install jinja2` compatibility claims can be tested
* [ ] drop-in experiment: a `jinja2/__init__.py` shim that re-exports jutsu and
      run jinja's *own* test suite against it as the ultimate parity gate

## Cross-cutting quality items
* [ ] every phase lands with commits; never leave suite red at commit time
* [ ] all ports get differential fuzzing (structured generators, not only
      random splices) to hit rare grammar corners
* [ ] keep `STATUS.md` updated per phase
