# Plan: porting jinja2's parser to Rust

This is the working instruction set for porting `src/jutsu/parser.py` (~1040
lines, recursive descent over the token stream) into `crates/jutsu-core`.  It
follows the same methodology that made the lexer port safe: keep the original
implementation reachable, validate the port differentially, and only flip
defaults after parity is proven.

## Why not "just rewrite it"

The parser builds `jutsu.nodes.*` objects consumed by the compiler.  Two
constraints drive the design:

1. **Arbitrary-precision literals.**  Token values (int/float/string) are
   produced by `Lexer.wrap()` in python (`int(s, 0)` supports huge integers).
   A rust parser that re-parses literal text would overflow; instead the rust
   parser receives *already converted* token values and passes them through as
   opaque pyobjects.
2. **Extensions.**  `Parser` instances are handed to extension parse functions
   (`ext.py`: loopcontrol, i18n, ...), which may themselves call back into the
   python parser machinery.  The rust parser is therefore only authoritative
   when no extensions contribute tags/streams; otherwise we fall back.

## Encoding (rust AST -> python nodes)

The rust parser returns nested tuples:

    ("__node__", ClassName, lineno, [args...])
    ("__list__", [items...])
    ("__val__", pyobject)          # literal passed through untouched

`src/jutsu/ast_rebuild.py` turns those into `nodes.*` instances via a registry
built from the `fields` tuples of each node class.  This keeps field order and
constructor arity honest: if rust ever emits an unknown class or wrong arg
count, rebuild fails loudly rather than silently.

## Validation strategy (three layers)

1. **Dump equality** (`tests/test_rust_parser.py`): rebuild the rust AST and
   compare `node.dump()` against the python parser's tree for every template in
   the repo test resources plus a fuzz corpus of generated templates.
2. **Line-number equality**: walk both trees in lockstep comparing `.lineno`
   (`dump()` omits attributes, so this is an explicit extra check).
3. **Full suite**: once parity holds, flip `Parser.parse` to prefer the rust
   core (flag: `JUTSU_RUST_PARSER`, default on when no extensions are active)
   and run the original 911-test suite unchanged.

## Work items

* [x] PLAN/TODO docs
* [x] `parser.rs`: TokenStream mirror (`current`/`look`/`skip`/`push`/`expect`)
* [x] expression grammar: condexpr/or/and/not/compare/math1/concat/math2/pow/
      unary/primary/tuple/list/dict/postfix/filter_expr/subscript/subscribed/
      call_args/call/filter/test
* [x] statement grammar: set/for/if/with/autoescape/block/extends/include/
      import/from_import/call_block/filter_block/macro/print/assign_target/
      signature
* [x] `subparse` / `parse` (template body, data buffering, end-token stack)
* [x] error messages parity (`fail`, `_fail_ut_eof`, expect-style errors)
* [x] `ast_rebuild.py` + registry over all concrete node classes
* [x] pyo3 glue: `parse(tokens) -> AST | (__error__, msg, lineno)`
* [x] differential tests (repo templates + fuzz), dump + lineno equality
* [x] flip default, full suite green, commit

## Lessons recorded

* The python `TokenStream` iterator advances *before* yielding, so consuming
  it into a list loses the last token fetched before a lexer error.  Consume
  the wrapped generator directly instead (`rust_parse.wrapped_tokens`).  A
  partial token is a real, consumable token; the error surfaces only on the
  fetch *past* it -- mirrored in rust by `tok_at`/`next_tok` raising the
  stored lexer error exactly where python's generator would.
* CPython `repr()` short-escapes only `\n`, `\t`, `\r`; everything else uses
  `\xHH`/`\uXXXX`/`\UXXXXXXXX`.  When a string contains both quote kinds the
  single quote is kept, apostrophes are escaped and double quotes are left
  alone (`pyrepr.rs`).
