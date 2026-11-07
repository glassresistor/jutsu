# jutsu - porting jinja2 to Rust

`jutsu` is a fork of [pallets/jinja](https://github.com/pallets/jinja) (Jinja 3.1.x)
whose long-term goal is to replace the Python implementation with a Rust core,
shipped to Python (and later JavaScript/wasm) as packages that keep working with
the existing Jinja2 extensions and ecosystem.

## Layout

    pyproject.toml            maturin build backend (rust core + python package)
    Cargo.toml                rust workspace
    crates/jutsu-core/        the rust core (pyo3 extension module `_jutsu`)
      src/lib.rs              pyo3 glue exposed to python (`tokenize`)
      src/lexer.rs            port of jinja2's lexer (regex state machine -> hand-written matchers)
      src/unicode_tables.rs   generated tables (scripts/gen_unicode_tables.py) matching CPython `re`
    src/jutsu/                the python package (forked from jinja2, imports renamed)
    tests/                    forked jinja2 test suite + differential rust tests

## Status

| Component | State |
|---|---|
| Python fork (`jinja2` -> `jutsu`) | done; full original test suite runs unchanged |
| Rust lexer (`crates/jutsu-core/src/lexer.rs`) | **done and wired into `Lexer.tokeniter`** |
| Rust parser (`crates/jutsu-core/src/parser.rs`) | **done and wired into `Parser.parse`** (default on; `JUTSU_RUST_PARSER=0` switches back) |
| Packaging (maturin wheel with rust core) | done (`maturin build -r -m crates/jutsu-core/Cargo.toml`) |
| Rust evaluator (`crates/jutsu-core/src/eval.rs`) | **done through slice C-v6** -- statements/scoping/macros/call blocks (C-v2/C-v3a/C-v3b), plain block inheritance (C-v4a), scoped blocks via derived eval contexts (C-v4b), includes/imports with join-path/tname semantics (C-v5), autoescape/Markup/volatile regions with the compile-time fold graph (C-v6); per-source parsed-AST cache with cached failure handles (slice C-v7a). **Wired into `Template.render` behind `JUTSU_RUST_EVAL` (default off)**: slice C-v8a wiring landed -- rust `check_supported` serve verdict (scan + layout, refusing parse-error ASTs and Extends/Include/Import nodes), python-side `rust_eval.eligible` / `serve_verdict` / `render_served`, per-template verdict cached on `Template._rust_serve`, template source retention through `from_code`/loaders. Five known divergences remain under the flag before C-v8a closes (see Working state 2026-11-02) |
| Code generator / runtime | still python by design: bytecode emission was rejected (`PLAN_C_DONE.md`); codegen stays reachable as the lazy fallback path on a decline verdict |
| JS wrapper (`js/`, `@jutsu/core` scaffold, Phase E slice E0) | **in progress**: value bridge (`values.mjs`: Undefined family with python message shapes, Markup/escape/soft_str, truthiness/pyStr/len/iter/eq/compare/contains rules, type mapping notes), ported filters subset (`filters.mjs`) and tests registry (`tests.mjs`), `Environment` shell (`environment.mjs`: registration, undefined-factory selection by name/class/subclass, autoescape callable plumbing, getitem/getattr/callFilter/callTest, decline-not-guess compile/render/attachBackend per decision D-E3), public surface `index.mjs`; node:test suites plus a golden replay harness capturing CPython behavior (`scripts/js_golden.py` -> `js/test/golden.json`, 141 cases incl. negative list subscripts). No npm dependencies. Remaining before E0 closes: pass-kind and lookup-matrix rule suites beyond golden replays |

The lexer port is validated three ways:

1. The original jinja2 test suite (911 tests) passes with the rust core active.
   `JUTSU_RUST_LEXER=0` switches back to the pure-python implementation at any
   time (`Lexer._tokeniter_python`).
2. Rust unit tests (`cargo test`): 14 tests covering whitespace control,
   lstrip/trim blocks, raw blocks, line statements/comments, numeric literals,
   unbalanced-brace errors and end-of-input semantics.
3. Differential fuzzing (`tests/test_rust_lexer.py`): thousands of random
   template sources across 8 lexer configurations are tokenized by both
   implementations and compared token-for-token, including error messages,
   line numbers and partial output produced before an error (the original
   `tokeniter` is a generator, so partial tokens must be yielded before the
   exception).

### Lexer port notes

Jinja's lexer is a regex-driven state machine. The rust port replaces the regex
engine with hand-written matchers over `Vec<char>` (python indexes by code
point), while reproducing CPython `re` semantics exactly:

* character classes (`\s`, `\d`, the generated identifier class) come from
  tables generated out of CPython itself (`scripts/gen_unicode_tables.py`).
* Numeric literals reproduce subtle backtracking behaviour, e.g. `1_2` matches
  as an integer but not as a float, and `010` lexes as integer `0` followed by
  name `10`.
* The raw begin tag (`{% raw %}`) only accepts the `-be\s* | be` variants (no
  `+be`, no trim-blocks suffix), unlike every other end tag.
* Failure rules `(.)` cannot match at end of input, so a dangling `{#` or
  `{% raw %}` silently ends the stream instead of raising.
* Python's `repr()` formatting is reproduced for error messages (`unexpected
  char '\'' at N`).

## Parser port notes

The rust parser consumes *wrapped* tokens (literal conversion already done by
the lexer's `wrap()`), so arbitrary-precision integers and python float
formatting pass through as opaque pyobjects (`__val__` entries) and are never
re-parsed in rust.  The AST is serialized as nested tuples and rebuilt into
`jutsu.nodes.*` instances by `jutsu/ast_rebuild.py` via a registry derived
from the node classes' `fields` -- unknown classes or wrong arities fail
loudly.

Validation layers (all green):

1. Dump equality plus lockstep tree comparison including line numbers and
   attributes (`cmp_tree` in `tests/test_rust_parser.py`).
2. Differential fuzzing: 48k+ random splices across six seeds, custom
   delimiters, repo fixture templates.
3. The original test suite (925 tests) runs with the rust parser as the
   default engine; instrumentation confirmed 1109 parses through rust, 54 of
   them error paths whose messages matched exactly, and only extension-enabled
   environments falling back to the python parser.

Extensions gate the switch: any extension contributing tags (i18n `trans`,
`do`, loopcontrol's `break`/`continue`, debug), or overriding
`filter_stream`/`preprocess`, forces the python parser -- see
`_extensions_contribute()` in `parser.py`.

## Evaluator port notes (slice C-v1)

The evaluator consumes the rust parser's `Av` tree directly -- no python node
rebuild, no codegen.  Every runtime value is a real python object and every
operation goes through the same helper the compiled engine uses
(`Context.resolve`, `Environment.getattr/getitem/call`,
`Environment._filter_test_common`, `runtime.str_join`), so behaviour matches
by construction rather than by reimplementation: undefined objects raise the
same `UndefinedError`, chained comparisons short-circuit like python, and a
failing operation propagates the original exception object untouched (the
compiled engine re-raises it through `Environment.handle_exception`).  Parse
errors get the same `translated` treatment `Environment.compile` applies so
`str()` of them carries just the message.

Validated by fixed cases plus a structured corpus of 6000 generated expression
templates comparing rendered text *and* exception type/message; anything the
slice cannot express -- statements, dynamic call arguments, autoescape/Markup
(the code generator folds constant output children through the non-markup-aware
`Concat.as_const`), environment finalize functions, volatile eval contexts --
is reported as unsupported so callers fall back to the python engine.

## Evaluator port notes (slice C-v2)

Statements execute with the compiled engine's scope model reproduced in rust:
a runtime scope stack mirrors the code generator's frames, and each hard scope
(root, for-body, for-else, with, set-block body) owns slot variables whose
initialization follows the first source occurrence of the name -- parameters
are not loaded, stores alias-copy from the nearest owning ancestor slot (or
stay `missing`), and loads own a slot only when no scope in the chain owns it
(then they resolve from the python context at each entry).  Tuple/list targets
unpack with python's own sequence-unpack semantics and error messages; the
special `loop` variable is provided by a live `LoopContext` (extended loops),
and recursive loops register their loop object so calling it renders the next
recursion level in rust exactly like the generated `loop(...)` call.
Root-level assignments write back into `context.vars` and update
`exported_vars`; stores inside loops/blocks stay local (`_loop_vars`).  The
`{% with %}` values evaluate against the outer scopes (the generator visits
each expression with the parent frame).

A pre-evaluation scan reproduces the code generator's compile-time checks over
the whole tree: nodes outside the evaluated subset (even in never-executed
positions), missing filter/test names, and assignment to `loop` inside any
for-subtree (raised as the same translated `TemplateAssertionError`).  This
closes the gap where python fails at compile time for code the evaluator would
never reach (`{% if false %}{% for loop in [] %}...` etc.).

Perf evidence (statement template, 2000 iterations, this machine): python
parse+compile 735 us, compile+render 770 us, *cached* render 6.2 us; rust
parse+eval 129 us (~6x faster cold, still far behind a warm python cache --
the AST-cache prerequisite before flipping any default remains).

Validated by 40 fixed statement/scope cases plus a structured corpus of 12k
generated full templates across three seeds comparing rendered text *and*
exception type/message (`tests/test_rust_eval.py`, `scripts/fuzz_v2.py`);
zero mismatches.  Deferred on purpose: macros, filter/call blocks, block
inheritance and includes (dedicated slices C-v3/C-v4), dynamic call arguments,
autoescape/Markup, environment finalize functions and volatile eval contexts
-- all reported as unsupported so callers fall back to the python engine.

The `if=` test of a for loop runs in its own frame (mirroring the generated
filter function): its loads resolve from the context and never observe a store
of the same name made by the loop body -- merging those symbols would make
`{% for a in xs if t %}{% set t = ... %}` skip every iteration.  Recursive
loops combined with `if=` are reported as unsupported until the reciter-wrapping
generator path exists.

Slice C-v3a adds filter blocks: body renders into a buffer inside its own
scope frame, the implicit filter target receives the joined buffer and the
chain is applied through `_filter_test_common`, then the result goes through
the normal output finalize.  The symbol-occurrence order follows
`analyze_node` (filter expression before body) for both filter blocks and set
blocks; loop frames analyze target+body before the test expression registers
its loads -- matching when a later occurrence must not override an earlier
decision.

## Evaluator port notes (slice C-v3b)

Macros and call blocks are the first part of the evaluator that must outlive
the statement that defined them, so the evaluator shares its `Session` through
an `Rc` and every macro closure keeps the *defining frame chain* as
`Vec<Rc<RefCell<Scope>>>`: popped frames stay alive (and writable) through
those `Rc`s exactly like the compiled engine's python closures over root
locals, so a macro invoked later sees the current value of a captured
variable -- including variables assigned after the definition.

A `{% macro %}` creates a real `runtime.Macro` python object whose `func` is a
rust callable (`MacroFunc` pyclass), so argument matching, arity errors,
`macro 'm' takes not more than N argument(s)`, the two-values-for-caller
guard and the `<Macro name>` repr all run through python's own `Macro.__call__`
-- byte-identical semantics by construction.  Invoking enters a fresh scope
frame initialized from the define-time symbol analysis (`macro_body` plus the
occurrence order parameters, defaults, body), binds parameters (a missing one
either evaluates its default expression inside the macro frame or builds
`environment.undefined("parameter 'x' was not provided", name=...)`), renders
the buffered body and returns it joined through `Environment.concat`.
Implicit `caller`/`kwargs`/`varargs` follow the code generator: undeclared
loads of the specials mark `accesses_*`, an explicit parameter drops the
name from tracking, and only the *last* `caller` parameter is remembered for
the "must be omitted or be given a default" assertion.

One systematic divergence is handled by falling back instead of trying to
reproduce it: python's code generator writes keyword arguments into generated
source, so a duplicated key (`{{ f(a=1, a=2) }}`, `{{ x|filter(b=1, b=2) }}`,
a call block whose target already spells `caller=`) or a duplicated macro
parameter makes CPython fail with a `SyntaxError` whose message embeds
generated line numbers (`keyword argument repeated: a (<template>, line 12)`)
that no interpreter can echo.  The scan reports those unsupported; when any
keyword key is itself a python keyword the generator takes its dict-literal
workaround instead, where duplicates simply lose to the later entry -- which
is what the interpreter does, so those render normally.

Async-enabled environments (`Environment(enable_async=True)`) fall back
unconditionally as well -- found while validating this slice: even the
synchronous `render` path of an async environment awaits coroutines and
consumes async iterables, which needs the python async runtime modelled
before it can be served (`test_async_falls_back`).

Validated by 33 differential macro/call-block cases (closure lifetime across
`for`/`with`/filter scopes, recursion and mutual recursion, nested macros,
the caller specials, error parity) plus 8 codegen-syntax fallback cases and
4 semantics locks (`tests/test_rust_eval.py`), and a macro-focused structured
corpus of 33k generated templates over eleven seeds comparing rendered text
*and* exception type/message (`scripts/macro_fuzz.py`): zero mismatches; the
only fallback reasons in the whole corpus are the two codegen `SyntaxError`
cases above.

Branch-aware symbol merging (gap found by fuzz seed 2, now closed): the
occurrence walk collects structured occurrences per `if` statement (the test,
then one group per branch, with the elif chain as nested structure), and the
slot-kind builder evaluates each branch group against an independent copy of
the region state before merging them exactly like python's
`Symbols.copy`/`branch_update`: names stored only inside branches become
RESOLVE loads (ALIAS when an ancestor scope owns them) -- a store that may
never have executed cannot shadow the context value with a missing slot.
Nested ifs recurse; stores already made unconditionally before the merge are
not demoted.  Validated by nine fixed branch-merge cases including the
original seed-2 template plus fuzz seeds 1-3 x8k and the full suite.

## Roadmap

1. **Parser** (`jutsu/parser.py` -> rust): done; wired into `Parser.parse`
   with differential validation against the python parser's node trees.
2. **Runtime/interpreter**: a tree-walking evaluator in rust, first as a
   differential renderer used to validate against the python code path,
   then as the default engine once filter/test parity is proven. Done through
   slice C-v6 plus cache v7a; wiring v8a landed behind `JUTSU_RUST_EVAL`
   (default off); remaining slices per PLAN_C_DONE.md: close v8a, cross-
   template serving v8b, measurement + default flip v8c.
3. **Extensions** (`loopcontrols`, `do`, `i18n`, `debug`): keep working through
   the python layer (gating rule settled in D1: any extension contributing
   tags/filter_stream/preprocess forces the python path); third-party compat
   story settled in D2 (`docs/EXT_COMPAT.md`); loaders/bytecache audit (D3)
   and `NativeEnvironment` exclusion (D4) open.
4. **JavaScript/wasm**: scaffold landed in `js/` (slice E0: value bridge,
   filters subset, tests registry, Environment shell; node suite green at
   157 incl. golden replay); share `crates/jutsu-core` via wasm-bindgen once
   the core API is decoupled from pyo3 (E1: move pyo3 glue behind a cargo
   feature; E2 wasm crate; E3 packaging).

## Building

    # python only (no rust build):
    pip install -e .            # uses maturin backend
    # or manually:
    cargo build --release -m crates/jutsu-core/Cargo.toml \
      && cp target/release/lib_jutsu.so src/jutsu/_jutsu.so

Tests run against `src/` directly via pytest's `pythonpath = ["src"]`, so no
install step is required during development.


## Evaluator port notes (slice C-v4a)

Plain template inheritance now runs inside rust: `{% extends %}` chains, block
invocation with the code generator's runtime guards (`extends_so_far` /
`has_known_extends`, mirrored dynamically because a non-executed branch that
contains an `extends` has no observable effect anyway), `super()` implemented by
registering the rust block functions into python's own `Context.blocks` so
python's `Context.super` / `BlockReference` -- including the "there is no parent
block called ..." undefined message and repeated renders -- run unchanged, and
`self` via a real `runtime.TemplateReference`. Required blocks check their list
length before invoking, raising the same translated `TemplateRuntimeError`.

Scoping follows codegen: block functions get an independent symbol table (loads
always resolve from the python context, stores stay local through the new
`local_only` marker), while soft frames of `if` preserve toplevel/rootlevel and
every other construct resets them -- matching which nested blocks are guarded or
dropped. Extends is only legal where codegen accepts it (root body or inside
`if` branches); anything below a block/macro/call-block boundary falls back, as
do scoped blocks (derived eval contexts), non-string extends targets and parents
that cannot be loaded/parsed from the provided sources -- python's loaders stay
in charge for those either way.

Validated by 13 fixed differential inheritance cases in `tests/test_rust_eval.py`
(multi-level chains, nested overrides, super at every level including repeated
calls, self references before and after override definitions, required blocks
both ways, dynamic extends through variables) plus a structured fuzz corpus of
random two/three-level template chains (`scripts/block_fuzz.py`, seeds 1-5 with
200 chains each): ok=58 exc=811 fallbacks=2131 mismatches=0; the only fallback
reason classes in the whole corpus are scoped blocks, duplicate block names
within one template (python's own compile-time failure) and extends parents that
are not provided or cannot be parsed -- where python raises too. Full suite 990
passed with the rust core active. Deferred follow-ups: scoped blocks' derived
context semantics, parent-side error parity instead of fallback, includes /
imports (`{% include %}`, `{% import %}`) as slice C-v5.

## Evaluator port notes (slice C-v4b)

Scoped blocks now run inside rust too. A scoped invocation evaluates its
`dump_local_context` equivalent at the call site: the live scope frames of the
interpreter instance are merged from the top down -- inner layers shadow outer
ones exactly like `Symbols.find_ref`, names owned but still `missing` stay out
of the derived dict (and still shadow, matching the single reference per name),
and the walk stops after the first *boundary* frame: scopes pushed by template
roots and block functions are where a generated function's symbol chain ends,
so nothing below them is ever dumped. The mapping goes through python's own
`Context.derived`, which merges it over the current vars, copies the blocks
lists and shares the eval context -- byte-identical semantics by construction.
The derived object flows onward: block functions run with whatever caller
passed (`BlockFunc.__call__` no longer ignores its argument), `super()` /
`TemplateReference` bind on that same object, nested scoped invocations derive
from it again, and macro closures capture the context current at their own
definition (the generated code's closure over the nearest enclosing function's
`context` variable) rather than whatever caller passes later.

Two systematic mirrors rounded out the slice. First, visit_For's `extended_loop`
is forced by *any* scoped block below a for node -- not just by a `loop` load or
recursive mode -- so the rust loop switches to its LoopContext branch for those
templates as well (the stores of such a loop include the special variable, which
is what makes them dump into derived contexts). Second, test-filtered loops that
would need the code generator's reciter-wrapping / LoopContext-over-the-filter-
generator path are now rejected at scan time instead of mid-execution: an
unsupported report from inside a block function or macro body could no longer
turn into a clean engine switch (it would surface as a python `RuntimeError`
instead), and falling back before anything runs is always allowed -- the whole
statement corpus including never-executed positions keeps that behavior green.

Validated by 10 fixed differential scoped cases (`tests/test_rust_eval.py`:
visibility of loop stores through derived contexts, non-scoped invocations not
seeing them, for-else frames excluding the loop layer, parent loops driving a
derived invocation whose child override reads the parent's locals and chains
`super()` across two levels, scoped nested inside block functions where the
symbol chain ends at that frame, macros defined under a scoped invocation
closing over the derived context, scoped blocks in macro bodies seeing their
parameters as dumped stores, required+scoped raising `Required block 'rb' not
found`, dynamic extends with both branches agreeing) plus the inheritance fuzz
corpus rerun over seeds 1-5 x600 chains: ok=339 exc=3023 fallbacks=5638
mismatches=0 -- the scoped reason class disappeared entirely, and what remains
is exactly the two known deferred classes (duplicate block names within one
template where python's own codegen fails at compile time, unloadable/unparsable
extends parents). Statement corpora seeds 1-10 x8k (`scripts/fuzz_v2.py`), the
macro corpus seeds 1-7 x3k (`scripts/macro_fuzz.py`) and the full suite (991
tests) all green with the rust core active; `cargo test` unchanged at 17.
Remaining deferred: parent-side error parity instead of fallback, includes /
imports as slice C-v5.


## Evaluator port notes (slice C-v5)

Includes and imports now run inside rust as well, with python's loaders staying
fully in charge of the actual template loading.  The lookup function is picked
exactly the way codegen picks it -- a literal string goes to `get_template`, a
literal sequence to `select_template`, anything else (variables, expressions) to
`get_or_select_template` -- and every call passes *this module's* name as the
loader parent argument, so relative names resolve against the template that
contains the statement.  That last piece needed a notion of "current module":
the interpreter carries a `tname`, macro closures and block functions capture the
module they were compiled into (their own `CodeGenerator.name`), and an extends
parent's rendering swaps it for the parent's name and restores it afterwards --
one generator per template module, exactly like python.  A join-path suite with
a path-joining loader environment discriminates all three cases: a child-side
override's include resolves against the child module, a parent's own block body
against the parent, and a sub-directory sibling reaches its own directory file.

`ignore missing` catches only `TemplateNotFound` and wraps only the lookup --
the generated code has no try-block around the rendering itself -- so errors
raised by included bodies propagate untouched; select-list misses surface as
python's own `TemplatesNotFound` with the rendered name list, including the
"none of the templates given were found: ..." message.  With-context includes
call python's own `template.new_context(context.get_all(), True, dump_locals())`
and then iterate the root render function pushing its parts into the caller's
buffer -- reusing byte-for-byte the boundary-aware local dump introduced for
scoped blocks in C-v4b (the same walk that stops at generated-function symbol
chains), which is why stores made before the include reach the included body
while deeper frames stay invisible below boundaries.  Without-context includes
take `_get_default_module()._body_stream` with no arguments, matching compiled
engines and letting python cache the module on the `Template` object across
renders.

Imports mirror `visit_Import` / `visit_FromImport`: `{% import %}` builds a fresh
module through `make_module(all, True, locals)` when context flows in, otherwise
the cached default module -- *with* this context passed along so newly appearing
globals reach it -- and then stores per the codegen rules: slot store always, at
toplevel additionally a direct write into `context.vars`, plus dropping public
aliases from `exported_vars` (an imported module never re-exports).  Named
imports resolve through python's own `getattr(module, name, missing)` falling
back to an undefined built by the environment with codegen's exact message --
which quotes the module name and the original attribute name, not the alias.

Validated by 34 fixed differential cases (context splits, ignore-missing versus
strict raising `TemplateNotFound`, select lists incl. empty-list and nested-list
errors, dynamic targets through variables incl. undefined names, imports with
and without context inside loops / block bodies / macro bodies / caller bodies /
set- and filter-block buffers, underscore aliases keeping their exported_vars
treatment, included templates that themselves inherit or nest includes, runtime
errors crossing the module boundary) plus a join-path suite of three path
templates.  A structured include/import fuzz corpus (`scripts/include_fuzz.py`,
seeds 1-3 x400): ok=643 exc=557 fallbacks=0 mismatches=0 -- zero fallbacks, so
every generated shape is now decided by the rust engine.  Macro / call-block
wrappers are deliberately excluded from that generator: python's own machinery
swallows some render-time errors raised inside *invoked* macro bodies into
unconsumed generator objects whose repr embeds an object address, making those
shapes un-comparable rather than merely different; the fixed cases cover includes
inside macro and caller bodies where nothing raises.  Statement corpora seeds
1-2 x8k (`scripts/fuzz_v2.py`), macro corpus seed 1 x3k
(`scripts/macro_fuzz.py`) and inheritance chains seed 1 x150 all rerun green, the
full python suite passes at 993 tests with the rust core active, `cargo test`
unchanged at 17.

Slice C-v6 -- autoescape and volatile regions.  The code generator's compile-time evaluation context is now mirrored per region next to the live one, so constant folding (including python's non-markup-aware `Concat.as_const`), wrapper choice and markup joins follow the generated engine exactly; `{% autoescape %}` tags with foldable options fix wrappers at entry while volatile regions consult the live attribute per part.  Block functions compile from module-start defaults because python emits them after every scoped tag reverted, which also exposed a latent bug: silent block invocations nested inside scope tags after a known top-level extend were dropped wholesale instead of only while the frame chain stays toplevel (python renders such overrides twice -- once through the inheritance chain and once at their position).  Call blocks, filter blocks and recursion levels push raw objects into the caller buffer.  Fixed suites plus new autoescape parity and inheritance probes green; statement seed 1 x8k rerun ok=1343 exc=6576 fb=81 zero diffs, include/macro/block corpora rerun zero mismatches, suite at 994 tests, cargo test still 17.  Remaining deferred: parent-side error parity instead of fallback, async environments falling out wholesale,
and the performance pass.


Slice C-v7a -- performance pass part 1: per-source parsed AST cache in the
rust evaluator.  The core gained split entry points: compile_ast keeps a parse
outcome alive (the tree itself or a deterministic lexer/parse failure) as a
handle object, and eval_parsed evaluates such a handle with exactly the same
result tuples as the single-shot path, including re-raising stored lexer errors
with their original python arguments.  The driver caches handles keyed by the
lexer-settings tuple plus source, template name and filename -- names belong to
the key because wrapped lexer-error arguments bake them in at compile time;
failures cache too since they are deterministic for a key.  Handles live in an
LRU bounded at 1024 entries; cores without the split entry points keep working
through the old single-shot function untouched.  Benchmarks (scripts/
bench_perf.py, N=3000): expression template -- rust cold unique-source eval
29.7 us vs python cold parse+compile+render 235.8 us (~8x win), python warm
cached render 5.2 us vs rust warm cached-AST eval 10.0; statement template --
rust cold 143.5 vs python cold 769.8 (~5x win), python warm 6.3 vs rust warm
20.3, so caching costs about a seventh of the cold path but compiled execution
still wins for warm renders and the default stays unflipped.  Validation: five
new cache tests (hit-skips-lexing, lexerr reproducibility incl. per-name keys,
lexer-settings isolation, eviction roundtrip, cached unsupported reasons); all
corpora rerun with identical counts to before at zero mismatches; full suite
999 passed with the cache active, cargo test still 17.  Remaining deferred:
parent-side error parity instead of fallback, async environments falling out
wholesale, and the rest of the performance pass (cutting per-node pyo3 round-
trips toward warm-rust parity).

## Retarget (2026-11-01): relaxed completion criteria (PLAN_C_DONE.md)

Phase C stalled on two obligations with no reachable end: making warm rust
*beat* python's compiled cached render, and chasing exception-type/message
parity on every error shape.  Both are reclassified:

* **R1 success parity only.**  Templates that render successfully under
  python must render byte-for-byte identically from rust or decline
  cleanly; reproducing exceptions is no longer chased -- uncertain shapes
  fall back, error-path divergences get recorded as accepted.
  Parent-side inheritance error parity, async runtime modeling and the
  macro-body generator-swallow quirk are closed as accepted divergences.
* **R2 hot-load comparability band replaces the beat-gate.**  Warm (cached)
  loads only need to be similar to python's: <= 4x python warm on both
  bench shapes AND < 50 us absolute, with cold rust >= 3x faster than
  python cold.  Measured today (N=3000): expression rust-warm 10.5 us vs
  python-warm 5.0 (2.1x); statement rust-warm 21.1 vs 6.3 (3.3x); cold
  wins 7.8x / 5.2x -- the band holds already, so no perf blocker remains;
  the runtime.rs mirrors move to optional post-flip backlog.
* **R3 accepted trade-off.**  Repeated renders of rust-served templates
  will cost ~2-4x python's warm cached path after the flip; the wins are
  cold/first-load (~5-8x) and future warm-path work.

Remaining Phase-C slices: C-v8a (wire evaluator into `Template.render`
behind `JUTSU_RUST_EVAL`), C-v8b (cross-template serving), C-v8c (measure
& flip default); optional polish C-v9.  Phase D is split into four
parallel packages D1-D4 plus backlog D5 -- see PLAN_D_PARALLEL.md.

## Settled behavior: the extension fallback rule (Phase D, package D1)

The rule an ecosystem can rely on, now enforced end-to-end and covered by
tests/test_ext_rust.py:

* **Any extension contributing parse tags, `filter_stream` or `preprocess`
  forces the python parse+codegen path** -- and therefore never rust serving.
  All four shipped extensions do exactly that (`loopcontrols` contributes
  `break`/`continue`, `do` contributes `do`, `i18n` contributes `trans`,
  `debug` contributes `debug`), so they are always python-parsed and python-
  compiled.  The predicate is the same one the parser has used since
  PLAN_PARSER.md (`_extensions_contribute`) and, after C-v8a, the serve-side
  predicate `rust_eval.eligible` reuses it.
* **Pure runtime extensions -- filters and tests only -- stay rust-eligible.**
  Filters and tests resolve through the real python `Context`, so nothing
  needs parse-time help; a no-tags extension renders identically through the
  python engine and through the rust evaluator, and keeps selecting the rust
  parser.
* **Every shipped extension renders exactly as upstream jinja does.**  Each
  one was diffed against Jinja2 3.1.6 installed alongside this fork:
  loopcontrols and `do` match byte-for-byte; `debug` matches once the two
  unavoidable differences are normalized (the package prefix inside class
  reprs, and object addresses); i18n matches with translations installed,
  including plural forms -- and, shared with upstream, the trans block's
  literal singular/plural strings are *not* looked up in the mapping (only
  gettext callables are), while oldstyle ngettext returns the translated
  format string without substituting the count.
* **Error paths decline cleanly rather than guessing.**  The rust evaluator
  has no extension machinery, so extension syntax is a parse failure there;
  verified byte-identical to what python's own parser reports for the same
  source without the extension (`Encountered unknown tag 'break' ...`), which
  is exactly the decline R1 asks for.
* **i18n extraction keeps working through the python parse path.**
  `extract_from_ast`, `babel_extract` and `Environment.extract_translations`
  all verified; instrumented runs confirm that with i18n loaded the parse is
  python, so trans nodes exist for extraction to find.  (Plain environments
  may extract from a rust-parsed tree -- both produce equivalent ASTs.)
* **The C-v8a serve predicate matches the rule** and adds its own conditions:
  async, sandboxed and native value environments are never eligible, and
  nothing is eligible while `JUTSU_RUST_EVAL` is off.  Verdicts are per-
  environment-shape: two environments with different extension sets get
  independent answers (the B2 two-environment probe rule).

Validation for D1/D2 together: full python suite green at 1036 passed with
default engines; the three extension suites (test_ext.py forked unchanged,
test_ext_rust.py, test_ext_thirdparty.py) green under both default and
`JUTSU_RUST_EVAL=1`; cargo tests unchanged at 17; zero edits to existing src
files by either package.

Third-party extension compatibility is settled in docs/EXT_COMPAT.md: the
extension API surface measured identical to upstream (eight public members),
so no `jutsu.ext.api` shim is needed; extensions must subclass
`jutsu.ext.Extension`; code that keeps importing real `jinja2` gets
upstream's independent engine, with a documented hazard -- registration is
duck-typed, so such an extension is silently accepted into a jutsu
environment and renders through a mixed AST (jinja2.nodes.Call carrying
jutsu.nodes.Const children) purely by structural coincidence.  The
`jinja2`-named alias package question stays deferred to the Phase-F drop-in
experiment.

## Working state (2026-11-02): sweep after an interrupted multi-agent night

No session finished cleanly between commits f978110 and this line; the
working tree carried half-finished work from two in-flight slices.  This
sweep records what landed, what is still open, and cleans up scratch files
left inside the repo (moved to scratch/junk-2026-11-02/, not deleted).

* Slice C-v8a is IN PROGRESS, not complete.  Landed: rust `check_supported`
  serve-decision entry point; python-side eligibility predicate
  (`rust_eval.eligible`), per-template serve verdict cached on the template
  object, and render-through-rust wired into `Template.render` behind
  `JUTSU_RUST_EVAL` (default off); template source retention through
  `Environment.from_string` and the loaders' `from_code` path.  The scan now
  also rejects dynamic call / filter / test arguments at compile time, which
  closes the verdict-vs-scan disagreement behind the two 'dynamic call
  arguments' suite failures reported by D1.  Measured: default suite green at
  1036 passed with the wiring in place; cargo unchanged at 17; under
  `JUTSU_RUST_EVAL=1` five failures remain (5 failed, 1031 passed) -- all
  previously known output/traceback divergences, now down from seven.  Not
  done: serve-side tests, render instrumentation, and the remaining five
  shapes; exit criteria in PLAN_C_DONE.md still apply.
* Phase E slice E0 is STARTED, not complete.  `js/package.json` plus a
  repaired `js/src/values.mjs` (the file was truncated mid-function: the
  `reprElement` body, `objectTypeRepr`, `isPlainObjectish` and the closing
  brace of the `Undefined` class were missing; now completed against
  `runtime.py` / `utils.object_type_repr` message shapes).  Clean under
  node --check.  Filters, tests registry, environment scaffold, index and
  the js/test/ suites are not started; nothing in js/ is settled behavior
  yet -- no E0 rule is pinned until its differential test lands.

## Working state (2026-11-04): slice E0 wrap-up after the second crash

A session crashed again while extending `js/src/values.mjs` with bridge
helpers; this sweep commits what survived and pins one real divergence found
while checking it.  Landed here: the pending values diff (pyLen/pyIter/
pyEq/pyCompare/pyContains/reprElement family plus type-mapping notes), all
the js files recovered from before them (`filters`, `tests`, `environment`,
`index`, README, test suites and golden generator) verified to parse clean;
node --test green at 156 including four direct environment-shell rule tests
(constructor validation, undefined-factory selection by name/class/subclass,
autoescape callable plumbing, decline-not-guess compile/render/attachBackend
contract per decision D-E3).

Measured against engine truth: python `Environment.getitem` is literally
`obj[argument]`, so negative subscripts wrap on lists; JS tryItem rejected
them outright -- repaired and pinned as new golden cases gi-list-neg /
gi-list-neg-oob captured fresh from the real engine.  Golden file regenerates
deterministically (141 cases).

Deliberately NOT settled: pass-kind and lookup-matrix rule suites beyond what
golden replays cover were in flight when this session's own writes began
dropping mid-line; they are left out rather than committed half-written, so
later sessions must treat E0 as still open (TODO.md Phase E carries the
status).  The inert placeholder js/test/debug_int.mjs stays untracked.

## Working state (2026-11-06): verification sweep and documentation refresh

No code changed here; this sweep re-measures every board after two idle days
and brings README.md / STATUS.md in line with the codebase, including `js/`
(which the top status table had never described).

* Tree clean at fdd055c; nothing half-committed was left behind.
* Default python suite: **1036 passed** (~12 s).
* cargo unit tests unchanged at **17**.
* Full suite with `JUTSU_RUST_EVAL=1`: exactly the five known divergences
  (5 failed, 1031 passed): `TestBug::test_pass_context_loop_vars` (KeyError),
  `test_pass_context_in_blocks`, `test_pass_context_block_and_loop`
  (AssertionError), `test_tests::test_multiple_tests`, and
  `test_debug::test_runtime_error` -- identical to the counts recorded on
  2026-11-02, so nothing drifted in between.
* JS node suite: **157 passed** (`node --test js/test/` from the repo root;
  one more passing test than the 156 recorded on 2026-11-04, all green).
  The `js/test/debug_int.mjs` placeholder mentioned in the previous section
  no longer exists on disk (removed by a later sweep); nothing is untracked.
* Environment note for future sessions: the sandbox needs `trio` installed
  (`tests/conftest.py` imports it) and PYTHONPATH must keep the
  `.unsloth-packages` entries while adding `src` -- overwriting PYTHONPATH
  wholesale breaks conftest collection.
