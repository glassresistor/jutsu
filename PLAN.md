# Jutsu Project Plan

## Executive Summary

This document outlines the completed work for the jutsu project, which is a Rust port of Jinja2 with a Python wrapper. Based on repository history and current implementation status, phases C and D have been completed as indicated by commit 56e1f9a.

## Completed Phases

### Phase C - Compiler/evaluator

**Status: Completed**

Completed tasks:
- Expressions → statements/scoping
- Filter blocks
- Macros
- Inheritance/includes/autoescape
- C-v8a wiring completed (see `PLAN_C_DONE.md`)

Implementation details:
- Rust lexer (`crates/jutsu-core/src/lexer.rs`) - ✅ Done
- Rust parser (`crates/jutsu-core/src/parser.rs`) - ✅ Done
- Rust evaluator (`crates/jutsu-core/src/eval.rs`) - ✅ Done
- C-v8a serve-decision machinery landed (rust `check_supported`, python-side eligibility predicate, per-template verdict cached on the template object)

### Phase D - Extensions

**Status: Completed**

Completed tasks:
- Extension gating and fallback mechanisms
- Third-party extension compatibility
- D1 gating/fallback and D2 third-party extension compat landed

Implementation details:
- Extensions that contribute tags, `filter_stream`, or `preprocess` force fallback to Python parser
- Pure runtime extensions (filters/tests only) may still be served by rust

## Current Implementation State

### Feature Flags

- `JUTSU_RUST_LEXER` = 1 (default) - Uses Rust lexer
- `JUTSU_RUST_PARSER` = 1 (default) - Uses Rust parser
- `JUTSU_RUST_EVAL` = 0 (default) - Uses Python evaluator (Rust eval off)

### Compatibility

- Full Jinja2 API compatibility maintained
- Extensions compatible with new system

## Performance Evidence

Latest numbers (`scripts/bench_perf.py`, N=3000, part-2a build):

| shape | rust cold | python cold | python warm | rust warm |
|---|---|---|---|---|
| expression `{{ a ~ b }}` | 30.2 µs | 234.6 µs | 5.0 µs | 10.5 µs |
| statement (set/for/if/loop/with) | 148.0 µs | 771.9 µs | 6.3 µs | 21.1 µs |

Cold rust wins ~8× / ~5×; warm rust costs 2.1× / 3.3× of python's cached path.

## Roadmap Progress

### Phase E - JavaScript

- JS scaffold (`js/`) in progress with node:test suites passing
- Golden replay tests covering 141 captured cases

### Phase F - Release Engineering

- Not yet started

## Notes

The project has reached a stable state where core functionality is implemented and tested. The default behavior still uses Python evaluation for compatibility, but Rust evaluation can be enabled via the `JUTSU_RUST_EVAL` environment variable.