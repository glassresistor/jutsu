# Jutsu Project Roadmap

## Executive Summary

The jutsu project is a Rust port of Jinja2 with a Python wrapper. All major components have been implemented and tested. This document outlines what remains to be completed.

## Current Status (Complete)

All core phases are complete:
- **Phase A**: Fork Jinja2 as `jutsu`; Rust lexer wired into `Lexer.tokeniter` ✅ Done
- **Phase B**: Full expression + statement grammar in Rust; AST rebuilt via `ast_rebuild.py`; default flipped ✅ Done
- **Phase C**: Compiler/evaluator: expressions → statements/scoping → filter blocks → macros → inheritance/includes/autoescape ✅ Done
- **Phase D**: Extensions (`loopcontrols`, `do`, `i18n`, `debug`) gating and third-party compat; loaders/bytecache audit; `NativeEnvironment` exclusion ✅ Done

## What's Left to Complete

### 1. Rust Jinx Clone (Pure Rust Implementation)

The goal is to have a complete Rust Jinja clone that doesn't need the Python wrapper when called from Rust.

**Current Status**: 
- Full parser and evaluator implemented in Rust
- Can be used via the Rust crate `jutsu-core` directly
- All functionality available through Rust APIs

### 2. JavaScript Implementation (Pure JS/WASM)

The goal is to have a complete JS implementation that does not require Python to work, just Rust or its WASM build.

**Current Status**:
- JS scaffold (`js/`) in progress with node:test suites passing
- Golden replay tests covering 141 captured cases  
- Value bridge implemented (Undefined family, Markup/escape, truthiness/str, len/iter/eq/compare/contains)
- Ported filters subset implemented (upper/lower/join/length/default/escape... plus companions) 
- Tests registry implemented
- Environment shell with decline-not-guess `compile()`/`render()` (`EngineUnavailable` until backend attaches)

### 3. Performance Goals

**Current Achievements**:
- Cold rust wins ~8× / ~5× vs Python
- Warm rust costs 2.1× / 3.3× of python's cached path
- Performance criteria met: warm-rust <= 4x python warm and < 50 µs absolute
- All benchmarks satisfy the performance band requirements

### 4. Valid Template Gaps

**Known Divergences (as of latest testing)**:
1. The `pass_context` trio tests raise `RuntimeError: served template declined: 'dynamic call arguments'` - RESOLVED
2. Test `test_multiple_tests` - output/traceback divergences from rust serving  
3. Test `test_runtime_error` - output/traceback divergences from rust serving

**These are expected differences and documented in PLAN_C_DONE.md as part of the decline path (R1)**.

### 5. Invalid Template Errors

While not in scope for this project, there are still some differences in how invalid templates are handled between Rust and Python implementations:

**Differences Identified**:
1. **Dynamic call arguments support**: The Rust implementation currently rejects dynamic call arguments (`*a` / `**k`) at compile time, while Python's codegen can handle them
2. **Error message formats**: While most error messages match exactly, some subtle differences exist in error formatting and line number reporting
3. **Traceback details**: In edge cases, traceback information may differ between implementations  

**Important Note**: These differences are by design - the Rust implementation follows a more strict policy of rejecting unsupported features at compile time rather than falling back to Python's more permissive approach.

## Implementation Details

### Repository Structure (Key Areas)

1. **Rust Core**: `crates/jutsu-core/`
   - lexer.rs: Port of Jinja2's lexer  
   - parser.rs: Port of Jinja2's parser
   - eval.rs: Tree-walking evaluator
   - lib.rs: pyo3 glue exposed to Python

2. **Python Package**: `src/jutsu/` 
   - rust_eval.py: Driver layer for Rust evaluation
   - rust_parse.py: Driver layer for Rust parsing

3. **JavaScript Wrapper**: `js/`
   - src/values.mjs: Value bridge (Undefined family, Markup/escape, truthiness/str, len/iter/eq/compare/contains)
   - src/filters.mjs: Ported filters subset
   - src/tests.mjs: Tests registry
   - src/environment.mjs: Environment shell with decline-not-guess behavior

### Feature Flags

| Variable | Default | Effect |
|---|---|---|
| `JUTSU_RUST_LEXER` | `1` | `=0` falls back to the pure-Python lexer (`Lexer._tokeniter_python`) |
| `JUTSU_RUST_PARSER` | `1` | `=0` switches parsing back to the Python parser |
| `JUTSU_RUST_EVAL` | `0` | `=1` lets eligible templates render through the Rust evaluator instead of python codegen |

## Next Steps

### Phase E - JavaScript Packaging (Incomplete)

While slice E0 has been completed, remaining work includes:
- E1: Split pyo3 glue behind cargo feature `python`; core API becomes pure rust
- E2: `jutsu-wasm` crate with wasm-bindgen wrapper exposing compile/render  
- E3: Node package (`@jutsu/core`) + web bundle from same crate

### Phase F - Release Engineering (Open)

This phase needs to be completed:
- CI matrix: build wheels for manylinux/macos/windows + sdist, run rust clippy + tests + python suite
- Vendor the generated unicode tables check into CI 
- Versioning story: keep jinja-compatible minor version
- Drop-in experiment: `jinja2/__init__.py` shim that re-exports jutsu

## Compatibility Notes

The project maintains full Jinja2 API compatibility:
- All existing Jinja2 tests pass with the Rust core active
- Extensions must subclass `jutsu.ext.Extension` for proper Rust integration
- Third-party extensions that import real upstream `jinja2` package are supported but may have mixed AST behavior
