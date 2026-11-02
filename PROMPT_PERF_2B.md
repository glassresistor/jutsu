# PROMPT: Slice C-v7b -- Performance pass part 2 (warm-path fixed overheads)

> **SUPERSEDED IN PARTS (2026-11-01).**  The numeric exit gates in section 3
> ("rust <= python" warm) and the wiring recommendation in B6 no longer
> apply: the retarget decision (PLAN_C_DONE.md) replaced the beat-gate with
> a comparability band that today's measurements already satisfy, and moved
> the runtime.rs mirrors to an optional post-flip backlog.  Items B1..B5
> survive as *optional measured polish* (slice C-v9): land them only where
> measured to win and proven by dedicated probes; the probes get committed
> as regression tests even when they kill an optimization.  Do not gate any
> default flip on this document -- see PLAN_C_DONE.md for the actual
> remaining Phase-C slices (C-v8a wiring, C-v8b cross-template serving,
> C-v8c measurement & flip).

You are a careful junior Rust/Python engineer working on the `jutsu` project.
This document is your complete work order. It is self-contained: everything you
need to know about the repository, the current state, the exact task, how to
measure it, how to prove correctness, and when to stop. Work through it
top-to-bottom; do not improvise scope changes.

---

## 0. Who/what/where

`jutsu` (this repo, `./jutsu`) is a fork of `pallets/jinja` (Jinja 3.1.x) whose
engine internals are being rewritten in Rust while the Python package stays the
public interface. The lexer and parser are done and default-on in Rust.
A tree-walking evaluator (`crates/jutsu-core/src/eval.rs`) has been grown
slice by slice (C-v0 .. C-v6) until it can render, entirely in Rust,
everything the compiled Python engine can -- expressions, statements,
scoping, macros, call blocks, block inheritance (plain + scoped), includes,
imports, autoescape/Markup/volatile regions. It is validated *differentially*:
same template rendered both ways compared byte-for-byte including exception
type and message; anything the slice cannot express reports "unsupported" so
the caller falls back to Python rather than ever guessing.

The evaluator is **not wired into `Template.render` yet** -- it is reachable
only through `src/jutsu/rust_eval.py::evaluate()` (used by tests and scripts).
The reason it is not wired: warm Rust evaluation must first become *faster*
than Python's compiled cached render. That is the whole content of this slice.

### Repo map (paths relative to repo root `./jutsu`)

```
crates/jutsu-core/src/eval.rs      the evaluator (~4.1k lines) -- YOUR MAIN FILE
crates/jutsu-core/src/lib.rs       pyo3 glue: tokenize / parse / compile_ast / eval_parsed / eval_template
src/jutsu/rust_eval.py             python driver for the evaluator (cache + fallback logic)
src/jutsu/runtime.py               Context/EvalContext/LoopContext/Macro/... (python engine runtime)
src/jutsu/compiler.py              the python code generator -- your SEMANTIC ORACLE for every question
src/jutsu/environment.py           Environment / Template.render / new_context
src/jutsu/{parser,nodes,lexer,...} forked jinja internals
tests/test_rust_eval.py            differential evaluator suite (39 tests)
scripts/bench_perf.py              the benchmark you must use (run: python scripts/bench_perf.py)
scripts/fuzz_v2.py                 statement corpus  (usage: python scripts/fuzz_v2.py <seed> <count>)
scripts/macro_fuzz.py              macro corpus
scripts/block_fuzz.py              inheritance-chain corpus
scripts/include_fuzz.py            include/import corpus
STATUS.md / TODO.md / PLAN_INTERP.md   living docs -- update them as your final step
```

### Build & test loop (all commands from repo root `./jutsu`)

```bash
# rust unit tests (must stay at 17 passing; you may ADD tests, never remove)
cd crates/jutsu-core && cargo test --quiet

# rebuild the native core used by the python package
cargo build --release -q            # from repo root uses workspace Cargo.toml
cp target/release/lib_jutsu.so src/jutsu/_jutsu.so

# full python suite (currently: 999 passed in ~10s)
python -m pytest -q --no-header

# benchmark (N=3000, prints the four-row table per template shape)
python scripts/bench_perf.py
```

Environment flags that exist and matter:
* `JUTSU_RUST_LEXER` (default 1; `=0` pure-python lexer)
* `JUTSU_RUST_PARSER` (default 1; `=0` python parser)
There is **no** `JUTSU_RUST_EVAL` flag yet -- the evaluator never touches
`Template.render`; only direct calls into `rust_eval.evaluate` use it.

---

## 1. The core principle of this codebase (read twice)

**Match by construction, never by reimplementation.** Every runtime operation
in `eval.rs` goes through the *same* python object and method the compiled
engine uses (`Context.resolve_or_missing`, `Environment.getattr/getitem/call`,
`Environment._filter_test_common`, `runtime.new_context`, python's own
`Macro.__call__`, `Context.super`, `Context.derived`, ...). Undefined objects
are created by the environment's own factory. Exceptions propagate as the very
same python exception objects. This is why 999 tests + ~100k fuzz templates
green with zero mismatches is *trustworthy*: behaviour is not re-derived, it is
shared.

Consequences for you:
* If a speedup would require implementing something in Rust that python's
  engine also does, you must justify byte-identical outcomes, ideally by
  showing the compiled engine resolves/bakes the same thing at compile time.
* Any change you cannot prove identical -> keep the old behaviour (or fall
  back to "unsupported"). A lost microsecond is fine; a wrong render is fatal.
* The python code generator (`src/jutsu/compiler.py`) is your oracle. When in
  doubt what the compiled engine does with, say, filter lookups, read the
  `visit_*` methods there.

Second invariant (from `eval.rs` header comment): **no borrow of a `RefCell`
field may be held across a python call.** Helpers collect values first, drop
borrows, then call python. Violating this = runtime panic under recursion.

---

## 2. State of the world (facts you may trust)

* Python suite: **999 passed** (~10 s). Rust `cargo test`: **17 passed**.
* Latest commit: "Performance pass part 2a" -- caches `escape`/`Markup`/
  `str_join`/`markup_join` handles in `Session` instead of a per-node
  `rt().getattr(...)` lookup.
* Slice C-v7a added the per-source parsed-AST cache: `_jutsu.compile_ast`
  produces a `ParsedAst` handle (tree, or a stored deterministic lexer/parse
  failure); `_jutsu.eval_parsed` evaluates such a handle with exactly the same
  result tuples as the single-shot `eval_template`. Driver caches handles in
  an LRU of 1024 keyed by lexer-settings tuple + source + template name + filename.
* Differential corpora (rerun expectations, from recorded commits -- counts
  must stay IDENTICAL after your changes, at zero mismatches):
  - `scripts/fuzz_v2.py` seeds 1-2 x8000: **ok=1343 exc=6576 fb=81 diffs=0**
    (note: the ok/exc split per seed was recorded as 1343/1267/1361 across
    seeds; use `seed 1 count 8000` -> ok=1343 exc=6576 fb=81)
  - `scripts/macro_fuzz.py` seed 1 x3000: DIFFS 0
  - `scripts/block_fuzz.py` seed 1 x150 chains: green, mismatches=0
  - `scripts/include_fuzz.py` seed 1 n=400: **ok=211 exc=189 fb=0 mismatches=0**
* Known deferred items you must NOT touch (see section 9).

---
n## 3. The slice: C-v7b -- warm-path fixed overheads

### Goal (single sentence)

Cut the **per-render fixed overheads** of the cached-AST Rust evaluation path
so that warm `rust_eval.evaluate` beats warm python compiled cached render on
both benchmark shapes.

### Numeric exit gates (measured with `scripts/bench_perf.py`, N=3000, this machine)

| shape | python warm (target to beat) | rust warm today | gate |
|---|---|---|---|
| expression `{{ a ~ b }}` | ~5.0 us | ~10.2 us | **rust <= python** |
| statement (set+for+if+loop.index+with) | ~6.3 us | ~20.3 us | **rust <= python** |

If you cannot reach the gates, that is an acceptable outcome -- see "abandon
rules". What is NOT acceptable: shipping a slice whose numbers are unmeasured,
or whose correctness was checked only by "the suite passes".

### Baseline decomposition (measured on this machine 2026-11-01; re-measure
before you touch anything -- your machine may differ)

`{{ a ~ b }}`, warm cached AST, total `rust_eval.evaluate` = **10.3 us**:

| component | cost | notes |
|---|---|---|
| cache lookup (`_lexer_key` build + tuple hash + OrderedDict ops) | ~0.25 us | already cheap; str hashes cached by CPython |
| `prepare_sources(env, None)` | ~0.05 us | fast path |
| `make_context` -> `runtime.new_context` (fresh python Context per render) | **~4.1 us** | ~40% of the whole warm cost! |
| `_jutsu.eval_parsed` with pre-built ctx+AST (`{{ a ~ b }}`) | ~3.9-4.6 us | includes `Session::build` floor |

Control measurements: plain `runtime.new_context(...)` alone = **3.8 us**;
python's own warm render total is only ~5.0 us of which `tpl.new_context({})`
alone is **~4.0 us** -- i.e. BOTH engines pay roughly the same Context-
construction tax, and python's compiled root function body costs only ~1 us.
The Rust side additionally pays `Session::build` (roughly 2 us: ~19 python
attribute lookups + Rc/HashMap construction per render) plus interpreter walk
plus a whole-tree `scan` + `check_layout` + root symbol analysis on every
render (compile-time work that the compiled engine does once at compile time).

### Hypothesis decomposition (verify with instrumentation, do not trust this table blindly)

| hypothesis | where | expected warm win |
|---|---|---|
| H1: `Session::build` re-looks-up module-level constants (`runtime.missing/LoopContext/Namespace/escape/Markup/str_join/markup_join`, `builtins.slice`) ~7+ lookups per render | `eval.rs::Session::build` | 0.5-2 us |
| H2: root-frame symbol analysis (`exec_unit_inner`: walk_fsf + build_kinds + spec1_walk over the whole body) runs per render, not per cached AST | `eval.rs::exec_unit_inner` | small for tiny templates, scales with tree size |
| H3: `scan()` + `check_layout()` whole-tree walks run per render | `evaluate_with_sources` (the entry point calling scan/check_layout/prepare_unit/register_blocks) | scales with tree size |
| H4: python-side Context construction (`make_context`) is irreducible on the Rust side but reducible in *count* once wired into Template.render | `rust_eval.py::make_context` | none now; context for later wiring |

Instrument cheaply before optimizing anything:
1. From python, time `_jutsu.eval_parsed` with pre-built ast+ctx vs full
   `evaluate` (that isolates driver overhead) -- the bench script already
   gives you warm/cold split.
2. To isolate Session::build/scan/layout from evaluation proper, add temporary
   `std::time::Instant` prints inside `evaluate_with_sources` behind an
   env-var gate (`JUTSU_EVAL_TRACE=1`) or write a throwaway Rust unit test;
   remove the instrumentation before committing.
3. Cross-check: evaluate `{{ 1 }}` (trivial tree, full scan+build cost) vs
   `{{ a ~ b }}` vs the statement template -- the difference localizes the
   per-node tax.

---

## 4. Work items (execute in this order; each is one commit)

Every item: measure before/after with `scripts/bench_perf.py`, run the full
python suite + cargo test after every change, rerun the corpora in section 2
at the recorded counts, commit only when everything is green. If a measured
candidate wins less than ~0.3 us warm on both shapes, **skip it and record
that** rather than complicating the code.

### B1 -- Hoist immutable handle lookups into a process-global cache

*Where:* `Session::build` in `eval.rs`. Today every render does ~19 python
attribute lookups (`ctx.environment`, `eval_ctx`, `volatile`, `autoescape`
policy check, `sandboxed`, `is_async`, `filters`+downcast, `tests`+downcast,
`builtins.slice`, `finalize.is_none`, then 7 `rt.getattr` + `undefined`).

*Plan:* the values fetched from the **runtime module** (`missing`, `LoopContext`,
`Namespace`, `escape`, `Markup`, `str_join`, `markup_join`) and from
`builtins.slice` are module-level objects of a single fixed python process.
Store them once in a lazily-initialized global cache (use
`pyo3::sync::OnceLockExt` on a `static`, or a thread-local; document the
strong-reference retention as intentional -- these are immortal module
constants, and holding them costs nothing). The **environment-dependent**
lookups (`env.filters`, `env.tests`, `env.undefined`, guards: volatile,
non-bool autoescape policy, sandboxed, is_async, finalize) must stay per-render:
they observe live environment state and the current architecture's fallback
decisions depend on reading them fresh each render.

*Parity justification (write this in the commit message):* the compiled engine
bakes these very objects into generated module code at compile time (the
`write_commons`/dependency-resolution machinery in `compiler.py` resolves
helpers once per template module), so hoisting a lookup from per-render to
per-process is strictly *closer* to compiled semantics, not further -- the
runtime module attributes cannot change during a render, and if user code
monkeypatches `jutsu.runtime.escape` between renders, the compiled engine
would keep using the baked object anyway.

*Required new differential test (add to `tests/test_rust_eval.py`):*
a probe that monkeypatches one hoisted runtime attribute (e.g.
`jutsu.runtime.str_join`) between two renders of the same source through BOTH
the python compiled engine and `rust_eval.evaluate`, asserting both engines
agree with each other *and* documenting whether they agree with the patched or
original object. If python re-reads at render time where you hoped it bakes,
ABANDON the hoist for that attribute and keep it in Session::build.

### B2 -- Cache per-unit compile-time analyses inside the ParsedAst handle

*Where:* `evaluate_with_sources` runs `scan`, `check_layout`, `prepare_unit`
(which parses parent units lazily) and `exec_unit_inner` re-runs root symbol
analysis (`walk_fsf` + `build_kinds` + `spec1_walk` for `self`) on **every**
render even though the AST handle is cached.

*Plan:* these analyses are pure functions of (the tree, the environment's
filters/tests dicts contents, and -- for scan's "missing filter/test name"
check -- live environment contents). Cache per-unit root-frame kinds + uses_self
+ scan verdict inside the `ParsedAst` handle keyed by nothing but validated
each render against a cheap fingerprint... **STOP -- that sentence is exactly
the trap.** Think about what python does instead:

A compiled `Template` performs scan-equivalent work (codegen's compile-time
checks: unsupported nodes cannot happen in compiled code; missing filter/test
names are checked at codegen time by the DependencyFinderVisitor) **once, at
compile time**, and never re-checks them even if `environment.filters` changes
later. So a cached verdict per (handle, environment-identity) would actually
MATCH python's compile-once semantics better than today's per-render scan.
But our driver cache is keyed by lexer settings + source + name -- two
environments sharing an identical source string share one handle! Verify with
a dedicated differential probe first:

```python
# probe idea (make it a real test):
e1 = Environment(); e2 = Environment()
e2.filters.pop('upper')
t1 = rust_eval.evaluate(e1, '{{ x|upper }}', variables={'x':'a'})
# does the SHARED cached handle under e2 raise / fall back exactly as
# python's separately-compiled template does?
t2 = rust_eval.evaluate(e2, '{{ x|upper }}', variables={'x':'a'})
```

Run the equivalent through python's two `Template` objects too and compare.
If any caching decision you are about to make cannot be made to pass that
probe for BOTH engines, do not cache; instead reduce the constant factor of
the walk (e.g. avoid re-allocating Vec<Occ> per scan of a large tree if the
cost measurement says it matters). Only implement the parts B2 can prove.
Record the probe outcomes as tests either way -- they are valuable even when
they force you to abandon the optimization.

### B3 -- Loop-execution symbol analysis (statement shape only)

`exec_for` calls `loop_kinds()` per **for-statement execution** (not per
iteration) -- already much better than codegen-per-render. For a template with
a single top-level loop this runs once per render; measure whether the
statement-shape gap is dominated by it (H2/H3 say maybe). Same rules as B2:
measure, prove or skip.

### B4 -- Driver micro-cleanups (`src/jutsu/rust_eval.py`)

* `evaluate()` calls `prepare_sources(environment, templates)` even when
  `templates is None` -- fine (fast path), leave it.
* `_cached_compile` rebuilds the 12-tuple lexer key per call (~0.14 us) and
  concatenates tuples before hashing; a small memo mapping `id(environment)`
  -> cached lexer tuple would save attribute-chain reads, BUT environments are
  garbage-collectable and ids recycle -- any id-keyed memo must hold strong
  references or become invalidation-prone. Measure first (`_lexer_key` build
  timed at 0.14 us; dict get with key 0.10 us). Likely skip; record.
* `make_context` passes `dict(variables or {})` -- an extra copy per render;
  check `runtime.new_context` semantics before removing it (it mutates the
  dict into `parent`). Prove or skip.

### B5 -- Per-node micro-round-trips (only if gates still missed)

Candidates from the open TODO wording ("attribute access, iteration,
LoopContext and filter-call paths"): e.g. `self.s.X.bind(self.py).clone()`
patterns returning fresh handles per call site; `lookup()` walking the scope
chain allocating for missing names; Output-exec path calling `self.lit(...)`
per TemplateData child (data nodes are concatenated into strings per output
node -- python bakes data children into module literals!). Investigating
whether consecutive `TemplateData` siblings could be merged at parse time is
legitimate measurement, but changing the parser/AST is OUT OF SCOPE for this
slice -- if that's where the win lives, record it as evidence for a follow-up
slice instead of doing it.

### B6 -- Final gate & wiring decision (documentation only)

After B1-B5, rerun `scripts/bench_perf.py`. If BOTH gates pass, you have
earned the right to propose flipping `Template.render` to prefer the Rust
evaluator -- but **do not do it in this slice**. Instead write a short
recommendation into STATUS.md ("warm rust now beats python-compiled on both
shapes; default flip is ready as its own slice") and stop.

---

## 5. Validation protocol (every commit, no exceptions)

1. `cargo test --quiet` -> must be >= 17 passing, your new tests included.
2. Full python suite -> `999 passed` baseline; if you add tests, the count
   rises and every added test must pass.
3. Corpora reruns (section 2 counts must reproduce exactly at zero mismatches).
4. `python scripts/bench_perf.py` before/after table appended to your notes;
   final numbers go into STATUS.md.
5. Never leave the suite red at commit time; never commit half an item.

---

## 6. Guardrails and known traps (each has burned this project before)

* Byte-identical semantics by construction -- see section 1. Do not
  reimplement python behaviours in Rust when a call-through exists.
* RefCell borrows must never span a python call (`eval.rs` header invariant).
* `Session` is shared via `Rc` and macro closures keep defining frame chains;
  any global cache you add must be thread-safety conscious: pyo3's
  `Python<'py>` token discipline; use `OnceLockExt`/thread-local, never a
  `static Bound<...>`.
* A cached failure is only valid if it is deterministic for its key (the AST
  cache caches lexer failures because their arguments bake name/filename at
  wrap time -- reason the same way before caching anything).
* Python quirks already decided as fallbacks, do not "fix" them here: async
  environments (`enable_async=True`) fall back wholesale; codegen SyntaxErrors
  (duplicate keyword args / duplicate macro params) report unsupported;
  render-time errors inside *invoked* macro bodies can be swallowed into
  unconsumed generator objects with addresses in their reprs -- those shapes
  are simply not comparable.
* Never change the public python API, parser behaviour, lexer behaviour, or
  the AST encoding. The evaluator subset must only ever grow via explicit
  slices with differential proof.

---

## 7. Deliverables & conventions

* One commit per work item (B1..), plus a final docs commit if needed.
* Commit message style: imperative subject line under ~72 chars, then a body
  paragraph in the established house style -- what changed, WHY it is
  byte-identical/justified, and the full validation evidence (test counts,
  corpus counts, bench numbers). Example shape:

```
Performance pass part 2b: hoist runtime-module constant lookups into a
process-global handle cache.  Session::build re-read seven immortal
module attributes plus builtins.slice on every render; the compiled
generator bakes exactly these objects per template module at compile
time, so hoisting is closer to compiled semantics than re-reading.
Environment-dependent guards and lookups stay per-render ... Validation:
cargo test 19/19 (two new hoist probes), python suite 1002 passed,
corpora rerun identical counts at zero mismatches, bench N=3000:
expression warm 10.2 -> X.X us, statement warm 20.3 -> Y.Y us ...
```

* Final report (write to `PERF_2B_REPORT.md` in repo root AND summarize into
  STATUS.md): before/after bench table per item, per-item decision
  (implemented / measured-and-skipped with numbers / abandoned with the parity
  reason), new test inventory, and whether the numeric gates were met.
* Update `TODO.md` checkboxes for what you completed; keep the wording style
  of existing entries (they are dense, evidence-heavy).

---

## 8. Abandon rules & stopping criteria

* An optimization that cannot be proven byte-identical by a dedicated probe ->abandon it, and commit the probe as a regression test anyway.
* If after B1+B2(+B3) the gates are met, stop -- do not gold-plate.
* If after all items the gates cannot be met without violating section 1 or
  6, write the findings report explaining what the residual floor is
  (e.g. Context construction cost shared with python, interpreter walk vs
  compiled bytecode) and what structural slice would be needed instead
  (the `runtime.rs` mirrors item in TODO.md). That report IS a successful
  outcome for this slice.
* Timebox: if you find yourself stuck >~2 hours on one probe, commit your
  state with the probes as tests and hand off in the report.

---

## 9. Explicitly OUT OF SCOPE (do not touch, even to "improve")

* Wiring into `Template.render` / flipping any default.
* Parent-side error parity for inheritance; async environment support;
  dynamic call arguments; anything currently reported as unsupported -- the
  subset grows only via dedicated slices.
* Extensions (Phase D), wasm/JS (Phase E), packaging/CI (Phase F).
* Lexer, parser, AST encoding, `ast_rebuild.py`, python-side runtime classes.
* Renaming files/symbols; reformatting unrelated code.

---

## 10. Starter checklist (do these first, in order)

1. `cd jutsu && cargo build --release -q && cp target/release/lib_jutsu.so src/jutsu/_jutsu.so`
2. `python -m pytest -q --no-header` -> confirm 999 passed (~10 s).
3. `cargo test --quiet` in `crates/jutsu-core` -> confirm 17 passing.
4. `python scripts/bench_perf.py` -> record your machine's baseline table.
5. Instrument (section 3), verify the hypothesis decomposition, then begin B1.

Good luck. Precision over cleverness: this project's trust comes from never
being wrong about a render, not from being fast yet.
