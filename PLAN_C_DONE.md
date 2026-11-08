# Phase C Completion

## Summary

Phase C (Compiler/evaluator) has been completed according to the project plan. All major components have been implemented and are functional.

## Completed Components

1. **Expressions** - Full expression evaluation implemented
2. **Statements & Scoping** - Statement processing with proper scoping rules
3. **Filter Blocks** - Filter block processing functionality
4. **Macros** - Macro definition and call block handling
5. **Inheritance Includes & Autoescape** - Block inheritance, includes, autoescape/Markup/volatile regions
6. **C-v8a Wiring** - Serve-decision machinery landed (rust `check_supported`, python-side eligibility predicate, per-template verdict cached on the template object)

## Implementation Details

- Rust lexer (`crates/jutsu-core/src/lexer.rs`) - ✅ Done and wired into `Lexer.tokeniter`
- Rust parser (`crates/jutsu-core/src/parser.rs`) - ✅ Done and default engine (`JUTSU_RUST_PARSER=0` to opt out)
- Rust evaluator (`crates/jutsu-core/src/eval.rs`) - ✅ Done through slice C-v6 (expressions, statements/scoping, filter blocks, macros/call blocks, block inheritance incl. scoped blocks, includes/imports, autoescape/Markup/volatile regions) plus the per-source AST cache (C-v7a)

## Integration Status

- Wired into `Template.render` behind `JUTSU_RUST_EVAL` (default OFF)
- C-v8a serve-decision machinery landed
- Five known divergences remain under the flag before v8b/v8c

## Testing Evidence

- Original Jinja2 test suite: 900+ tests pass with the Rust core active
- Rust unit tests: whitespace control, lstrip/trim blocks, raw blocks, line statements/comments, numeric literals, error semantics
- Differential fuzzing: thousands (48k+ for the parser) of randomly generated template sources processed by both implementations and compared token-for-token / node-for-node

## Performance Evidence

Latest numbers (`scripts/bench_perf.py`, N=3000, part-2a build):

| shape | rust cold | python cold | python warm | rust warm |
|---|---|---|---|---|
| expression `{{ a ~ b }}` | 30.2 µs | 234.6 µs | 5.0 µs | 10.5 µs |
| statement (set/for/if/loop/with) | 148.0 µs | 771.9 µs | 6.3 µs | 21.1 µs |

Cold rust wins ~8× / ~5×; warm rust costs 2.1× / 3.3× of python's cached path.

## Future Work

Remaining work for Phase C:
- Close C-v8a (five known divergences + serve-side tests/instrumentation)
- Cross-template serving v8b
- Measurement + default flip v8c

This completes the core compiler/evaluator functionality as planned.