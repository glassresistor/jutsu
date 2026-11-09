# Plan: Phase E -- JavaScript wrapper for jutsu (2026-11-01)

Status line: the Rust core today is a `cdylib` linked against pyo3; nothing in it
can be handed to wasm-bindgen until the pyo3 glue moves behind a cargo feature.
That change belongs to slice E1 and touches Rust -- this document plans around it
without requiring it, and the JS work that can proceed today is scoped as E0:
pure JavaScript, no Rust changes, no Python changes.

## Why a JS wrapper at all

The engine's public interface is a Python package, but the deployment targets
jutsu cares about (static sites, edge functions, browser-side component
templates) have no Python. The end state per TODO Phase E:

```
                    ┌──────────────────────────────┐
   node / browser   │  @jutsu/core  (js/)          │
                    │    Environment, render       │
                    │    value bridge (Undefined,  │
                    │    Markup/escape, Context)   │
                    └───────────┬──────────────────┘
                                │ wasm-bindgen boundary (E2)
                    ┌───────────▼──────────────────┐
                    │  jutsu-wasm crate            │
                    │    lexer | parser | eval     │
                    └───────────┬──────────────────┘
                              rust core
```

Two hard constraints inherited from the Python port:

1. **Match by construction is impossible in JS.** The Rust evaluator reaches
   into Python's runtime (`Context.resolve`, `Environment.getattr/getitem/
   call`, `str_join`) to get behavior for free. In JS there is no host runtime
   to borrow -- a value bridge must be written, so every semantic decision
   that Python gets from CPython (truthiness, str(), attribute vs item
   lookup, Undefined arithmetic) becomes an explicit JS rule that needs its
   own tests. E0 exists precisely to write and pin those rules *before* the
   wasm crate lands, so E2 becomes wiring rather than design.
2. **Decline cleanly (R1).** Anything unsupported must raise a typed error
   naming what it could not do -- never render a wrong answer, never silently
   fall back. The Python side's "unsupported" reports are the model.

## Slices

### E0 -- JS scaffold + value bridge (no Rust/Python touched; implementable now)

Goal: a node package `@jutsu/core` skeleton carrying everything that does not
need the compiled core:

* **Value bridge** (`js/src/values.mjs`): `Undefined` family mirroring
  `runtime.Undefined` semantics (prints as empty string, falsy, len 0, iterable
  empty; arithmetic/comparison/call raise `UndefinedError` with the same
  message shapes: `'foo' is undefined`, hint form, "has no element/attribute"
  forms); `Markup` + `escape`/`escape_silent`/`soft_str` mirroring markupsafe
  (the `__html__` protocol becomes a JS convention: objects exposing
  `__html__()` are trusted); `missing` sentinel; truthiness and str() rules.
* **Filters subset** (`js/src/filters.mjs`): upper/lower/join/length/default/
  escape + the cheap companions (replace, capitalize, trim, title, first, last,
  reverse, sort, map, select/reject, int/float, list, abs, d=alias) with
  semantics matched to `src/jutsu/filters.py` case-by-case.
* **Tests registry** (`js/src/tests.mjs`): defined, none, even, odd, lower,
  upper, greaterthan..., matching `tests.py` semantics for the subset.
* **Environment** (`js/src/environment.mjs`): filter/test registration,
  undefined factory selection (Undefined / StrictUndefined /
  ChainableUndefined / DebugUndefined), autoescape function plumbing -- and a
  `compile()`/`render()` that currently *decline* with `EngineUnavailable`
  until a backend is attached. Backend attach point: `env.attachBackend(wasm)`.
* **Test harness**: node's built-in test runner (`node --test js/test/`), no
  npm dependency (npm/wasm-pack are absent in this environment; the package
  stays dependency-free so it runs anywhere).

Exit criterion: bridge/filter/test suites green under `node --test`; every rule
that Python gets from CPython has a named JS rule + test; documented parity
caveats (python filters/extensions NOT available in JS).

### E1 -- core decoupling (Rust; blocked for this session by design)

`split pyo3 glue behind cargo feature python; core API becomes pure rust
(lexer::tokenize, parser::parse -> AST enum, render(cfg, ast, ctx))`. Blocked
here because the session constraint forbids Rust edits; owned by a Rust session.

### E2 -- `jutsu-wasm` crate + binden (blocked on E1)

wasm-bindgen wrapper exposing compile/render over a JS value bridge; the
bridge API is *defined* in E0 and consumed there, not invented at that point.

### E3 -- packaging: node package + web bundle from same crate; wasm-pack test
matrix; document parity caveats. (Blocked on E2.)

## Decisions recorded here

* **D-E1: the bridge is JS-side, not generated.** No code generation from
  Python type hints; the bridge is hand-written with differential tests
  against CPython behavior captured as golden values (python runs available).
* **D-E2: no npm dependency footprint.** Node built-ins only (`node:test`,
  ESM `.mjs`); wasm-pack tooling arrives with E2/E3 when there is a wasm blob
  to load. This keeps E0 verifiable in environments without network/npm.
* **D-E3: decline-not-guess.** A JS `Environment` without an attached engine
  backend throws `EngineUnavailable` on compile/render -- it does not ship a
  secret second interpreter. Filters/tests/bridge are only reachable *through*
  templates; standalone access is for the test suites and future extensions.
* **D-E4: parity scope.** E0 targets the documented Phase-E subset
  (upper/lower/join/length/default/escape...). Full filter parity is later
  work (tracked in TODO Phase E "full parity later"); every unported filter is
  *absent*, not approximated.

## Validation commands

    node --test js/test/            # E0 suites
    python scripts/js_golden.py     # optional: regenerate golden values from CPython

## File ownership

All new files under `js/`; PLAN_JS.md itself. No file in `src/jutsu/`,
`crates/`, or existing plans is touched by E0.
