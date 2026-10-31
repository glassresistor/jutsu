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
* [ ] block inheritance machinery (super(), blocks dict, scoped blocks,
      required blocks) ported onto the interpreter
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
      reciter-wrapping generator exists.  Statement fuzz corpus grown to ~80k
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
      Validated: 41 fixed macro cases (`tests/test_rust_eval.py`) covering
      closure lifetime across for/with/filter scopes, recursion, mutual
      recursion, nested macros, caller/kwargs/varargs and error parity; plus
      a macro-focused structured corpus of 27k templates over eight seeds
      (`scripts/macro_fuzz.py`, fallback census in `scripts/macro_fb.py`):
      zero mismatches, and the only fallback reasons in the whole corpus are
      the two codegen SyntaxError classes above.
* [ ] `with`/autoescape scope modifiers via ScopedEvalContextModifier
* [ ] performance pass: avoid pyo3 round-trips for hot attribute lookups where
      safe; measure against python-compiled baseline and only keep wins.
      Baseline measured after slice C-v1 (simple `{{ a ~ b }}`, 2000 iterations,
      this machine): python parse+compile+render 234 us/op, rust parse+evaluate
      19 us/op (12x faster), but a *cached* python template renders in 5 us/op.
      Conclusion: the rust evaluator only wins for templates that are rendered
      once or twice (e.g. `Environment.from_string`), so before flipping any
      default it must cache parsed ASTs per source (and eventually compiled
      state) -- otherwise the python code path stays faster for warm caches.
      Same conclusion after slice C-v2 (statement template, 2000 iters):
      rust parse+eval 129 us vs python compile+render 770 us (~6x cold), but
      cached python render only 6.2 us.
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
