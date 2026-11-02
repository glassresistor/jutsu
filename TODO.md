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
      output buffering (`concat`, markup_join, str_join) -- demoted by the
      2026-11-01 retarget to an optional post-flip optimization backlog
      (PLAN_C_DONE.md R2): warm-rust parity no longer requires it.
* [x] autoescape semantics + `Markup` integration via markupsafe objects
      (slice C-v6): compile-time eval-context state, volatile regions and the
      constant-fold graph live in the evaluator.
* [X] async environments (`enable_async`) fall back unconditionally (found
      while validating C-v3b): even the synchronous `render` path awaits
      coroutines and consumes async iterables, so serving them needs the
      python async runtime modelled first; guard in `Session::build`,
      regression test `test_async_falls_back`.  Closed 2026-11-01 as the
      accepted design -- unconditional fallback is settled behavior, not an
      open parity obligation (PLAN_C_DONE.md R1).
* [X] slice C-v2 (statements) implemented in `eval.rs`: if/elif/else, for
      (lazy test-filtered loops, else branches, extended loops via a live
      `LoopContext`, recursive loops that render the next level in rust through
      registered loop objects), set / set-block with implicit filter target,
      with (values evaluated against outer scopes), namespace refs; compiled
      scope model reproduced: slot frames per hard scope, first-occurrence
      init kinds PARAM/ALIAS/RESOLVE/UNDEFINED, root writeback into
      `context.vars` + exported_vars, python sequence-unpack semantics and
      error messages for tuple targets.  A pre-evaluation scan mirrors the code
      generator's compile-time checks over the whole tree (unsupported nodes
      anywhere -- even in never-executed positions --, missing filter/test
      names, `loop` assignment inside any for-subtree raised as the same
      translated `TemplateAssertionError`).  Loop features cycle/first/last/
      length/revidx/previtem/nextitem/changed/depth all go through the real
      python `LoopContext`.  Validated: 40 fixed cases, loop-target and unpack
      error parity tests, plus a 12k-template structured statement corpus
      (3 seeds) with exception type/message parity -- zero mismatches.
      Deferred: macros / filter-blocks / call-blocks (`caller()` closures),
      block inheritance and includes (next slices C-v3/C-v4).
* [X] slice C-v4a (plain block inheritance): extends chains, block invocation with the codegen runtime guards, super()/self via python's own Context.super / TemplateReference, required blocks; scoped blocks, non-string extends targets and un-loadable parents fall back. Validated: 13 fixed differential cases plus `scripts/block_fuzz.py` structured chains (seeds 1-5 x200): ok=58 exc=811 fb=2131, zero mismatches; full suite green.
* [X] slice C-v4b: scoped blocks (`{% block x scoped %}`) implemented in `eval.rs`. A scoped invocation dumps its locals at the call site by merging the live scope frames top-down (inner layers shadow like `find_ref`, owned-but-missing names stay out of the dict and still shadow), stopping after the first boundary frame -- template roots and block functions are generated-function symbol chains that end there. The mapping goes through python's own `Context.derived` (merge over current vars, blocks-list copy, shared eval context). Block functions run with the caller's context object (`BlockFunc.__call__` respects its argument), so derived contexts propagate through `super()`, `TemplateReference`, nested scoped invocations and macro closures (each captures the context current at definition; new `ctxobj` on `Ev`/`MacroFunc`). visit_For's extended-loop forcing by any scoped block below the for node is mirrored (`any_scoped`), including the special loop variable entering the dumped stores. Test-filtered loops in extended/recursive mode are rejected at scan time (mid-execution unsupported reports from inside block/macro bodies could no longer become a clean engine switch). Validated: 10 fixed differential scoped cases incl. visibility contrasts, for-else exclusion, two-level derived `super()` chains, macro-closure capture and required+scoped error parity; block fuzz seeds 1-5 x600 chains green (the scoped fallback class disappeared entirely), statement corpus extended to seeds 1-10 with zero diffs, macro corpus seeds 1-7 green, full suite 991 passed.
* [X] slice C-v5 (includes and imports): lookup function selected exactly like codegen (`get_template` / `select_template` / `get_or_select_template`) with the current module name as loader parent; ignore-missing wraps only the lookup and catches only `TemplateNotFound`; with-context includes render through python's own `template.new_context(context.get_all(), True, dump_locals())` reusing C-v4b's boundary-aware local dump, without-context takes `_get_default_module()._body_stream`; `{% import %}` uses fresh `make_module(...)` or the cached default module *with* this context and applies `visit_Import` store rules incl. exported_vars discarding; named imports resolve through python's own attribute lookup with codegen's undefined message and batch writeback. A current-module name (`tname`) is threaded through interpreter, macro closures, block functions and parent renderings so relative `join_path` resolution matches compiled modules. Validated: 34 fixed differential cases + join-path suite in `tests/test_rust_eval.py`, structured corpus `scripts/include_fuzz.py` seeds 1-3 x400 (ok=643 exc=557 fb=0, zero mismatches); statement/macro/block corpora rerun green; full suite 993 passed.
* [X] python quirk left as-is: render-time errors raised inside *invoked* macro bodies can be swallowed into unconsumed generator objects (repr embeds an object address), so such shapes are simply not comparable -- the fuzz generator skips macro/call-block wrappers for exactly this reason; fixed cases cover includes in macro and caller bodies where nothing raises.  Closed 2026-11-01: un-comparable shapes are accepted, not chased (PLAN_C_DONE.md R1).
* [X] parent-side error parity for inheritance -- closed 2026-11-01 as an
      accepted divergence (PLAN_C_DONE.md R1): load/parse failures keep
      falling back; reproducing jinja's own exceptions from rust is no
      longer an obligation.
* [X] known gap fixed (found by fuzz seed 2, recorded in `scripts/mismatch.txt`):
      symbol occurrence merging across if-branches.  The occurrence walk now
      collects structured occurrences (`Occ::Branch` groups per `if`), and the
      kind builder evaluates branch groups against independent copies of the
      region state and merges them exactly like `Symbols.copy` /
      `branch_update`: names stored inside branches (and not already stored
      unconditionally) become RESOLVE loads (ALIAS when an ancestor owns them)
      instead of UNDEFINED slots; the elif chain evolves on its own copy;
      nested ifs recurse.  Validated: 9 fixed branch-merge cases incl. the
      original seed-2 template, fuzz seeds 1-3 x8k green, full suite green.
* [X] follow-up fix: the for-loop `if=` test evaluates in its own frame (the
      generated filter function) -- loads resolve from the context and never
      observe body stores of the same name; unpack-of-undefined parity
      restored.  Recursive loops combined with `if=` fall back until the
      reciter-wrapping generator exists (since slice C-v4b such combinations --
      test filters in extended or recursive mode, incl. scoped-forced extended
      -- are rejected by the pre-evaluation scan instead of mid-execution).  Statement fuzz corpus grown to ~80k
      templates (seeds 1-10, 8k/seed): zero mismatches.
* [X] slice C-v3a: filter-block semantics (buffered body in its own scope
      frame, implicit filter target = joined buffer, chain via
      `_filter_test_common`, result finalized with `str`); occurrence order
      fixed to match `analyze_node` for filter/set blocks and for loops.
      Namespace objects already covered in C-v2.
* [X] slice C-v3b: macros and call blocks implemented in `eval.rs`.  The
      evaluator's `Session` is shared through an `Rc` and each macro closure
      keeps its defining frame chain (`Vec<Rc<RefCell<Scope>>>`) so popped
      frames stay alive and writable -- a macro sees the current value of a
      captured name, including names assigned after the definition.  A
      `runtime.Macro` python object wraps a rust callable (`MacroFunc`
      pyclass), so argument matching, arity/keyword errors, the two-values
      for caller guard and the `<Macro name>` repr are python's own.
      Define-time analysis mirrors `macro_body` + both `analyze_node` passes
      (occurrence order parameters, defaults, body; implicit specials force
      PARAM kinds; only the LAST explicit `caller` parameter is remembered
      for the "must be omitted or be given a default" assertion).  Defaults
      evaluate inside the macro frame; a missing parameter without a default
      becomes `environment.undefined("parameter 'x' was not provided", ...)`.
      Call blocks inject `caller=` into the forwarded keywords exactly like
      `forward_caller=True`.
      Divergence handled by falling back: python's code generator writes
      keyword arguments into generated source, so duplicated keys in any
      Call/Filter/Test (`{{ f(a=1, a=2) }}`) or duplicated macro parameters
      make CPython raise `SyntaxError` with generated line numbers that
      cannot be reproduced; when a key is a python keyword the generator's
      dict-literal workaround applies instead and duplicates simply lose to
      the later entry (the interpreter matches that path directly).
      Validated: 33 differential macro cases plus 8 codegen-syntax fallback
      cases and 4 semantics locks (`tests/test_rust_eval.py`) covering
      closure lifetime across for/with/filter scopes, recursion, mutual
      recursion, nested macros, caller/kwargs/varargs and error parity; plus
      a macro-focused structured corpus of 33k templates over eleven seeds
      (`scripts/macro_fuzz.py`, fallback census in `scripts/macro_fb.py`):
      zero mismatches, and the only fallback reasons in the whole corpus are
      the two codegen SyntaxError classes above.
* [x] `with`/autoescape scope modifiers via ScopedEvalContextModifier
      (slice C-v6).
* [x] cache parsed ASTs per source (slice C-v7a): split `compile_ast` /
      `eval_parsed` entry points plus an LRU handle cache in `rust_eval`,
      keyed by lexer settings + source + name + filename; parse failures
      cached too.  Warm cached-AST evaluation measured ~7x cheaper than cold
      (`scripts/bench_perf.py`: statement template 143.5 us -> 20.3 us;
      expression 29.7 -> 10.0).  Full suite green with the cache active.
* [X] performance pass gate retargeted (2026-11-01, PLAN_C_DONE.md R2): warm
      rust no longer needs to *beat* python's compiled cached render -- a
      comparability band replaces the beat-gate: warm rust <= 4x python warm
      on both bench shapes AND < 50 us absolute per render, cold rust stays
      >= 3x faster than python cold.  Measured today (this machine,
      N=3000): expression warm 10.5 vs 5.0 us (2.1x), statement warm 21.1
      vs 6.3 us (3.3x); cold wins 7.8x / 5.2x -- both shapes already sit
      inside the band, so no performance blocker remains for Phase C.
* [ ] differential renderer validation: folded into C-v8a/C-v8c -- the wired
      suite plus instrumentation counts are the evidence; corpora gate on
      success-path parity only, error-path divergences recorded in
      ACCEPTED_DIVERGENCES.md instead of chased (PLAN_C_DONE.md R1).
* [ ] slice C-v8a: wire the evaluator into `Template.render` behind a
      `JUTSU_RUST_EVAL` flag (default off during the slice) -- eligibility
      predicate, per-template serve verdict with compile-time scan/analyses,
      lazy python codegen on fallback and module paths; suite green with the
      flag on plus instrumentation of rust-served renders.
      Status 2026-11-02 (in progress, wiring landed): rust `check_supported`
      entry point (scan + layout + cross-template-node refusal), python-side
      `rust_eval.eligible` / `serve_verdict` / `render_served`, per-template
      verdict cached on `Template._rust_serve`, source retention through
      `from_code`/loaders.  Default suite green at 1036 passed with the
      wiring in place; under `JUTSU_RUST_EVAL=1` five divergences remain
      (listed in the D1/D2 follow-up entry, updated).  Still to do: resolve
      or refuse those five shapes at scan time, serve-side tests and render
      instrumentation.
* [ ] slice C-v8b: cross-template serving -- include/import/extends targets
      served by rust when eligible instead of implicitly python-compiled via
      the loaders; join-path/tname semantics preserved (join-path suite is
      the anchor).
* [ ] slice C-v8c: gate measurement and default flip -- rerun benches against
      the band, corpora under success-gate policy, full suite with
      `JUTSU_RUST_EVAL=1` default-on; python codegen stays reachable via
      `=0`; evidence into STATUS.md.
* [ ] optional polish C-v9 (no gates attached): the measured hoists/caches
      from PROMPT_PERF_2B items B1..B5 plus the runtime.rs mirrors, landed
      only where measured and proven; post-flip backlog.

## Phase D -- extensions & ecosystem compatibility
Split into independently scoped packages for parallel execution; see
PLAN_D_PARALLEL.md for file ownership, conflict matrix and waves.
* [x] D1 extension gating & fallback correctness: loopcontrols / do /
      i18n / debug environments render exactly as upstream jinja does; the
      documented rule (extension contributes tags/filter_stream/preprocess
      => python parse+codegen) holds end-to-end including the wired render
      path; no-tags extensions stay rust-eligible.  Dispatch after C-v8a.
      Landed in tests/test_ext_rust.py (23 tests, new files only): per-
      extension upstream differentials incl. installed translations and
      plural forms; instrumented parser-selection for all eight environment
      shapes; extraction probes through the python parse path; the
      C-v8a `rust_eval.eligible` predicate asserted read-only (extensions,
      async, sandbox, native exclusion, flag-off behavior, per-environment
      verdicts).  Forked tests/test_ext.py green unchanged under default and
      JUTSU_RUST_EVAL=1; zero src edits.
* [x] D2 third-party extension compatibility & API story: synthetic
      third-party extension built and tested end-to-end; recorded decision
      memo on import paths and whether an api shim is needed (new files
      only).  Dispatch immediately.  Landed in tests/test_ext_thirdparty.py
      plus the extpkg fixture package and docs/EXT_COMPAT.md (14 tests, new
      files only): decisions recorded -- extensions must subclass
      jutsu.ext.Extension; code importing real jinja2 keeps upstream's
      engine; NO jutsu.ext.api shim needed (measured API surface identical:
      attr/bind/call_method/filter_stream/parse/preprocess/priority/tags);
      the jinja2-named alias package deferred to the Phase-F drop-in
      experiment.  Measured hazard: registration is duck-typed so an
      upstream-based extension is silently accepted into a jutsu environment
      and renders through a mixed AST (jinja2.nodes.Call over jutsu.nodes.Const) -- documented as an unsupported latent dependency, not a feature.
* [ ] D1/D2 follow-up for the integration owner (found while testing the
      wired render path): with JUTSU_RUST_EVAL=1 seven suite tests fail in
      files that use no extensions at all.  test_core_tags
      TestForLoop::test_cycling and test_lexnparse TestSyntax::test_call
      raise `RuntimeError: served template declined: 'dynamic call
      arguments'` -- a compile-time serve verdict disagrees with the
      per-render scan; the test_regression pass_context trio,
      test_tests::test_multiple_tests and test_debug::test_runtime_error are
      output/traceback divergences from rust serving.  All sit inside
      PLAN_C_DONE.md's scope (R1 decline path, error paths recorded not
      chased), not D1's; the extension suites pass under both settings.
      Update 2026-11-02: the dynamic-call pair is RESOLVED -- the scan now
      rejects dynamic call / filter / test arguments (`*a` / `**k`) at
      compile time, so a cached serve verdict can never disagree with the
      per-render scan; both tests pass under the flag.  Five remain: the
      pass_context trio, test_multiple_tests (suspected cause: each
      side-effecting test callable fires twice under rust serving where
      python's compiled code short-circuits the `or`) and
      test_runtime_error -- output/traceback divergences from rust serving.
* [ ] D2 follow-up: decide whether registration should reject non-jutsu
      Extension subclasses instead of duck-accepting them -- today a mixed-
      AST render works purely by structural coincidence, so nothing guards
      against silent breakage if either package's node internals diverge.
* [ ] D3 loaders & bytecache pure-python audit: every loader class plus
      BytecodeCache proven working with rust engines default-on; profiling
      memo justifying pure-python status.  Dispatch immediately.
* [ ] D4 NativeEnvironment parity: minimum = native environments excluded
      from rust serving, existing native tests green with the rust core
      active (dispatch after C-v8a); stretch = serve simple native templates
      from rust via a mirrored output mode (post-flip, coordinated).
* [ ] D5 backlog (not dispatched): extension hot paths in rust parser/
      evaluator, dynamic call arguments support, async runtime modeling --
      all currently correct-by-fallback; revisit only with post-flip
      evidence.

## Phase E -- JavaScript / wasm packaging
* [ ] slice E0 (PLAN_JS.md): JS scaffold + value bridge.  Status 2026-11-02
      (in progress): `js/package.json` and `js/src/values.mjs` written (the
      truncated file left by the interrupted session repaired: reprElement,
      objectTypeRepr, isPlainObjectish, Undefined class close); parses clean
      under node --check.  Not started: filters.mjs, tests.mjs,
      environment.mjs, index.mjs, js/test/ suites, golden-value script.
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
