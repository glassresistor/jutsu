# Plan: getting Phase C done under retargeted requirements (2026-11-01)

This document supersedes the completion criteria of PLAN_INTERP.md for the
remaining Phase-C work, and supersedes the numeric exit gates of
PROMPT_PERF_2B.md (its B1..B5 items survive as optional measured polish,
C-v9 below).  Decision context: the evaluator stalled because two open
obligations had no reachable end -- (a) making warm rust *beat* python's
compiled cached render, and (b) chasing exception-type/message parity on
every error shape the fuzzers find.  Both are reclassified here.

## The retarget decision

Measured baseline (this machine, `scripts/bench_perf.py`, N=3000,
part-2a build):

| shape | rust cold | python cold | python warm | rust warm | ratio warm |
|---|---|---|---|---|---|
| expression `{{ a ~ b }}` | 30.2 us | 234.6 us | 5.0 us | 10.5 us | 2.1x |
| statement (set/for/if/loop/with) | 148.0 us | 771.9 us | 6.3 us | 21.1 us | 3.3x |

The "rust <= python" gate of PROMPT_PERF_2B is unreachable for a
tree-walking interpreter without the full `runtime.rs` mirror project
(re-implementing Context/LoopContext/Undefined in rust), which trades away
the match-by-construction trust that makes this codebase's green boards
meaningful.  And python's own warm path pays roughly the same Context-
construction tax (~4 us of its ~5-6 us total), so even a mirrored runtime
would land *near* python, not far below it.

### R1 -- success parity is the only correctness gate

* REQUIRED: for every template shape that renders *successfully* under the
  python engine, the rust engine must either render byte-for-byte identical
  text or decline cleanly (scan-time unsupported -> python fallback) before
  emitting anything.  The never-guess invariant is untouched.
* RELINQUISHED: reproducing exception type/message on error paths is no
  longer an obligation to chase.  The forked suite's own error assertions
  still pass because runtime errors propagate as the very same python
  exception objects by construction -- but whenever a shape makes that
  uncertain, the correct move is decline/fallback, not new parity code.
* Harness change: differential corpora gate on zero success-path mismatches
  and zero crashes; ok/exc counts stay informational.  Error-path
  divergences get recorded in `ACCEPTED_DIVERGENCES.md`, not chased.

### R2 -- hot-load comparability band replaces the beat-gate

Hot (warm, cached) loads only need to be *similar* to python's, not faster.
Operational definition: warm rust <= 4x python warm on both bench shapes
AND absolute per-render cost < 50 us; cold rust stays >= 3x faster than
python cold.  Today's measurements sit inside the band already (2.1x /
3.3x, all numbers single-digit or low-double-digit microseconds).  The
`runtime.rs` mirrors TODO item moves from prerequisite to optional
post-flip optimization backlog.

### R3 -- accepted trade-off (recorded)

Flipping the default makes repeated renders of a rust-served template cost
~2-4x python's warm cached path.  The wins are cold/first-load (~5-8x) and
the future warm-path work; per-flip numbers get recorded at C-v8c.

### Closed as accepted divergences (were open "deferred" items)

* parent-side error parity for inheritance -> keep fallback, won't-fix;
* async environments (`enable_async`) -> fall back wholesale, guard +
  regression test already exist;
* render-time errors inside *invoked* macro bodies swallowed into
  unconsumed generator objects -> un-comparable by python's own machinery,
  closed;
* dynamic call arguments (`*a`/`**k`) and dynamic filter/test arguments ->
  remain scan-time fallbacks: correct via the python path, perf-neutral;
  supporting them is optional backlog, not a Phase-C blocker.

## Remaining slices to get C done

### C-v8a -- wiring scaffolding (behind `JUTSU_RUST_EVAL`, default off)

Wire the evaluator into `Template.render` at last:

* Eligibility predicate (`rust_eval.eligible(environment)`): core present,
  flag on, no extension contributes tags/filter_stream/preprocess (same
  predicate as `_extensions_contribute`), not async, not sandboxed, default
  `Environment` class only (native types excluded until D4).
* Serve decision per template: at first render (or compile), run the rust
  scan/analyses once and store the verdict next to the cached AST handle on
  the `Template` object; supported -> render through `eval_parsed` with the
  real `new_context(...)` ctx; unsupported -> lazily run python codegen and
  mark the template python-served forever (verdict flip must never depend
  on mid-render state).
* This also moves scan/check_layout/prepare_unit/root symbol analysis from
  per-render to per-template-compile -- today they run per render only
  because `evaluate()` has no Template object.  That relocation is the big
  warm-path win for the statement shape (hypotheses H2/H3 of
  PROMPT_PERF_2B) and needs no risky cross-environment caching.
* Integration surfaces to enumerate and cover (probe before deciding):
  `Template.render` / `render_async` / `render_to_string`, `.module` /
  `_get_default_module` / `_body_stream` (lazy python compile on demand),
  blocks introspection APIs, pickling paths, `Environment.compile_templates`
  and bytecache interplay, `meta.find_undeclared`.
* Validation: full suite green with the flag explicitly on;
  instrumentation counts how many suite renders actually served from rust
  vs fell back (this count IS the evidence for the "differential renderer
  validation" TODO item); corpora rerun at zero success-path mismatches
  under the new gate; cargo tests >= 17.

### C-v8b -- cross-template serving

Today includes/imports reach referenced templates through python's compiled
machinery (`template.new_context`, `_get_default_module()._body_stream`),
which implicitly compiles every referenced file through python codegen -- so
multi-file apps would only ever serve the entry template from rust.

* Goal: referenced templates are themselves served by rust when eligible,
  and python-compiled only when they must not be; join-path/`tname`
  semantics preserved (the existing join-path suite is the regression
  anchor).
* Probe the two designs first (PROMPT_PERF_2B's B2 lesson: any caching or
  serving decision must survive the two-environment probe): (a) recursive
  evaluate with source maps inside the evaluator, vs (b) Template objects
  carry their own serve verdict and the evaluator calls their render-rust
  path through a py callback.  Choose by measurement + parity evidence.
* Validation: multi-file matrix (path-joining loader env), include corpus
  rerun at recorded counts, plus proof-by-instrumentation that python
codegen never runs for fully rust-served chains.

### C-v8c -- gate measurement and default flip

* Rerun `scripts/bench_perf.py`; verify the comparability band (R2) holds
  with wiring in place; rerun every corpus under the success-gate policy;
  run the full suite with `JUTSU_RUST_EVAL` default-on.
* Flip: evaluator preferred for eligible plain environments; python codegen
  stays reachable via `JUTSU_RUST_EVAL=0` (house rule: original
  implementation always reachable).
* Evidence goes into STATUS.md/TODO.md in house style, including the R3
  trade-off numbers.

### C-v9 -- optional polish (no gates attached)

The measured candidates from PROMPT_PERF_2B B1..B5 (runtime-module constant
hoists, per-unit analysis caching if provable, driver micro-cleanups), each
landed only if measured to win and proven by dedicated probes; the probes
get committed as regression tests even when they kill the optimization.
Post-flip backlog, alongside the `runtime.rs` mirrors item.

## Dispatch order

    C-v8a --> { C-v8b , C-v9 } (parallel) --> C-v8c (flip last)

Phase D packages (see PLAN_D_PARALLEL.md) start with D2/D3 immediately; D1
and D4 consume the eligibility predicate defined in C-v8a.

## Validation protocol (every slice, house rules)

1. `cargo test --quiet` >= 17 passing, new tests included.
2. Full python suite green with rust active; never leave suite red at
   commit time.
3. Corpora reruns: zero success-path mismatches, zero crashes; error-path
   divergences recorded in ACCEPTED_DIVERGENCES.md instead of chased.
4. Bench table before/after appended to notes; final numbers into STATUS.md.
5. One commit per item, dense house-style messages with the evidence.
