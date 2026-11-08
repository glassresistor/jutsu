# Jutsu Project Status

## Overview

Jutsu is a Rust port of Jinja2 with a Python wrapper. The project aims to provide a compiled Rust core while maintaining full compatibility with the existing Jinja2 API.

## Current Status

### Phase Completion

- **A** - Fork Jinja2 as `jutsu`; Rust lexer wired into `Lexer.tokeniter` ✅ Done
- **B** - Full expression + statement grammar in Rust; AST rebuilt via `ast_rebuild.py`; default flipped ✅ Done
- **C** - Compiler/evaluator: expressions → statements/scoping → filter blocks → macros → inheritance/includes/autoescape ✅ Done
- **D** - Extensions (`loopcontrols`, `do`, `i18n`, `debug`) gating and third-party compat; loaders/bytecache audit; `NativeEnvironment` exclusion ✅ Done
- **E** - JS scaffold + value bridge; split pyo3 glue behind a cargo feature; ship `jutsu-wasm` / `@jutsu/core` for JavaScript 🟨 Started (slice E0 landed)
- **F** - Release engineering: wheel matrix (manylinux/macOS/Windows), sdist, clippy, versioning story ⬜ Open

## Implementation Details

### Core Components

- Rust lexer (`crates/jutsu-core/src/lexer.rs`) - ✅ Done
- Rust parser (`crates/jutsu-core/src/parser.rs`) - ✅ Done
- Rust evaluator (`crates/jutsu-core/src/eval.rs`) - ✅ Done

### Feature Flags

- `JUTSU_RUST_LEXER` = 1 (default) - Uses Rust lexer
- `JUTSU_RUST_PARSER` = 1 (default) - Uses Rust parser
- `JUTSU_RUST_EVAL` = 0 (default) - Uses Python evaluator (Rust eval off)

### Compatibility

- Full Jinja2 API compatibility maintained
- Third-party extensions compatible with new system
- Extensions that contribute tags, `filter_stream`, or `preprocess` force fallback to Python parser

## Performance

Performance benchmarks show significant improvements:

- Cold rust wins ~8× / ~5× vs Python
- Warm rust costs 2.1× / 3.3× of python's cached path

## Roadmap Status

### Phase C (Compiler/evaluator)
- All major components implemented
- C-v8a wiring completed and functional

### Phase D (Extensions)
- Extension gating and fallback mechanisms implemented
- Third-party extension compatibility landed

### Phase E (JavaScript)
- JS scaffold (`js/`) in progress with node:test suites passing
- Golden replay tests covering 141 captured cases

### Phase F (Release Engineering)
- Not yet started

## Notes

The project has reached a stable state where core functionality is implemented and tested. The default behavior still uses Python evaluation for compatibility, but Rust evaluation can be enabled via the `JUTSU_RUST_EVAL` environment variable.