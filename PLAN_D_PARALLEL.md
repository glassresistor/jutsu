# Plan: Phase D split for parallel execution (2026-11-01)

Phase D = extensions & ecosystem compatibility, retargeted: with the rust
lexer/parser default-on and (after C-v8c) the evaluator default-on for
plain environments, everything an ecosystem touches -- extension API,
third-party extension packages, loaders, bytecache, NativeEnvironment --
must keep working.  Under the retargeted goals (PLAN_C_DONE.md R1-R4) no
performance obligation attaches to these paths: correctness and documented
behavior only.

The old Phase-D checklist decomposes into four independently scoped
packages plus a backlog.  Packages are designed so that multiple engineer
sessions can execute them concurrently: disjoint file ownership, no shared
fixtures, each with its own exit criterion and validation commands.

## Work packages

### D1 -- Extension gating & fallback correctness (dispatch after C-v8a)

Goal: environments with extensions render exactly as upstream jinja does,
with the documented rule enforced end-to-end: any extension contributing
tags, `filter_stream` or `preprocess` forces the python parse+codegen path
(and therefore never rust serving); pure runtime extensions (filters/tests
only) may still be served by rust.

Scope:
* Differential tests for each shipped extension: `loopcontrols`
  (`break`/`continue`), `do`, `i18n` (`trans`/`gettext`/`ngettext`,
  translations installed, plural forms), `debug` (`{% debug %}`); plus a
  custom no-tags extension proving the *other* side of the rule (rust
  eligible).
* Instrumented tests asserting parser selection (rust vs python) and
  evaluator eligibility per environment shape.
* i18n extraction paths (`extract_from_ast`, `babel_extract`) still work
  through the python parse path.
* Documentation section in STATUS.md describing the fallback rule as
  settled behavior.

Owned files: NEW `tests/test_ext_rust.py`; STATUS.md section (applied by
integration owner at merge).  `ext.py`, `parser.py`, `rust_eval.py` are
read-only for this package -- the eligibility predicate is defined in
C-v8a and consumed here; if gating needs adjustment, record it as a
follow-up rather than editing shared files.

Exit: forked `tests/test_ext.py` green unchanged with rust default engines;
new gating/eligibility tests green; extraction probes green; zero changes
to existing src files.

### D2 -- Third-party extension compatibility & API story (dispatch now)

Goal: decide and document how extensions written against the extension API
keep working in jutsu, given the package rename (`jinja2.*` -> `jutsu.*`).
The TODO left this open ("add a compat shim if needed") -- make it a
recorded decision.

Scope:
* A synthetic third-party extension package built and tested end-to-end
  (subclass of `jutsu.ext.Extension` with a custom tag + filter), installed
  into the test environment via `sys.path`/tmp packaging, exercised in an
  environment that also has rust cores active.
* Decision memo on the import-path story: extensions must subclass
  `jutsu.ext.Extension`; third-party code still importing real upstream
  `jinja2` gets upstream's machinery (two independent engines side by
  side -- document the hazard; whether jutsu ships a `jinja2`-named alias
  package is deferred to the Phase-F drop-in experiment, not decided here;
  whether a `jutsu.ext.api` re-export module is needed: measure the actual
  upstream extension API surface and record yes/no with rationale.
* `docs/EXT_COMPAT.md` (or README section) writing up both.

Owned files: NEW only -- `tests/test_ext_thirdparty.py`,
`docs/EXT_COMPAT.md`, fixtures under `tests/res/thirdparty/`.  Zero edits
to existing src files.  Fully parallel with everything.

Exit: synthetic extension test green; decision memo committed; docs land.

### D3 -- Loaders & bytecache pure-python audit (dispatch now)

Goal: prove `loaders.py` / `bccache.py` are untouched by the rust core and
work identically with rust engines default-on, and record the profiling
basis for "no rust value there".

Scope:
* Integration tests exercising every loader class that exists in this fork
  -- BaseLoader subclasses, FileSystemLoader, PackageLoader, DictLoader,
  FunctionLoader, PrefixLoader, ChoiceLoader -- plus `Environment.
  compile_templates` and the bytecache round-trip (`FileSystemBytecodeCache`
  store + reload), with templates rendered through the normal engine (and,
  after C-v8a lands, additionally through rust eligibility: loaded-but-
  rust-served templates must render identically).
* A small profiling memo (cold-load costs dominated by IO/pickle, not
  lexing) justifying pure-python status; filed alongside.

Owned files: NEW `tests/test_loaders_rust.py`, `docs/LOADERS_RUST.md`;
zero edits to existing src files.  Fully parallel with everything.

Exit: new tests green; memo committed; existing loader/cache suites remain
green unchanged.

### D4 -- NativeEnvironment parity (minimum now, stretch after flip)

Goal: `nativetypes.py` works correctly with the rust core present.
Minimum accepted outcome: native environments are simply excluded from
rust serving (they never reach the evaluator), and the existing native
tests pass with the rust core active.  Stretch: serve simple native
templates from rust by mirroring native output semantics (native concat's
literal-eval fallback, raw-object finalize) in an evaluator output mode.

Scope:
* Minimum: tests asserting `NativeEnvironment`/`NativeTemplate` render
  correctly with rust cores default-on; instrumentation proving native envs
  never become rust-served (the C-v8a eligibility predicate excludes them;
  verify, don't assume).
* Stretch (only if the minimum lands green and only post-C-v8c): evaluator
  output-mode work in `eval.rs` + driver changes, gated behind its own
  flag, with dedicated differential corpus for native semantics.

Owned files: NEW `tests/test_nativetypes_rust.py`; `src/jutsu/nativetypes.py`
edits only if the minimum requires them (it should not); `eval.rs` touched
ONLY in the stretch phase and coordinated with C-v9 to avoid conflicts.

Exit: native suite green with rust active; exclusion instrumentation
green; stretch optional with its own evidence.

### D5 -- Backlog (not dispatched)

Explicitly deferred, revisit only when post-flip perf/feature evidence
appears: extension hot paths in the rust parser/evaluator (loopcontrols/do
tags served natively), dynamic call arguments support, async runtime
modeling.  These are currently correct-by-fallback.

## Parallelization map

    Wave 0 (today, zero dependencies):
        D2  (new files only)
        D3  (new files only)
        [C track runs C-v8a concurrently]
    Wave 1 (after C-v8a merges):
        D1  (consumes eligibility predicate)
        D4-minimum
        [C-v8b and C-v9 run in parallel per PLAN_C_DONE.md]
    Wave 2:
        C-v8c gate & flip last (needs v8b evidence)
        D4-stretch optional after the flip

D1..D3 share no files with each other; D4's minimum shares none either.
The only serialization points are (a) the eligibility predicate must exist
before D1/D4 verify against it, and (b) STATUS.md/TODO.md merge-time
updates.

## Conflict matrix

| shared surface | owner(s) | rule |
|---|---|---|
| `crates/jutsu-core/src/eval.rs` | C track only | D packages never edit; D4-stretch coordinated post-C-v9 |
| `src/jutsu/rust_eval.py` | C-v8a defines predicate | D1/D4 consume read-only; adjustments go to integration owner |
| `src/jutsu/environment.py` | C-v8a wiring only | no D edits |
| `src/jutsu/parser.py`, `ext.py` | stable (gating exists) | no D edits |
| existing test files | untouched | each package adds its own NEW test module |
| `STATUS.md` / `TODO.md` | integration owner | slice authors hand over dense entries at merge |

## Shared validation protocol (every package)

1. Full python suite green before and after; never leave suite red at
   commit time.
2. Success-gate policy (PLAN_C_DONE.md R1): byte-identical outputs for
   successful templates; error paths decline/fallback, divergences recorded
   not chased.
3. Corpora reruns required only when evaluator files change (D4-stretch).
4. One commit per package, dense house-style message with evidence counts;
   new tests must be added to the suite count and pass.
5. Each package may be dispatched as a self-contained PROMPT_D*.md work
   order generated from its section here (repo map, state of the world,
   slice items, validation protocol, guardrails, abandon rules) -- same
template as PROMPT_PERF_2B.md.
