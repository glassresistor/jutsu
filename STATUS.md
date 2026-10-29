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
| Code generator / runtime | still python (next targets) |

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

## Roadmap

1. **Parser** (`jutsu/parser.py` -> rust): done; wired into `Parser.parse`
   with differential validation against the python parser's node trees.
2. **Runtime/interpreter**: a tree-walking evaluator in rust, first as a
   differential renderer used to validate against the python code path,
   then as the default engine once filter/test parity is proven.
3. **Extensions** (`i18n`, `autoescape`, `looplevel`): keep working through the
   python layer; port the hot paths later.
4. **JavaScript/wasm**: share `crates/jutsu-core` via wasm-bindgen once the core
   API is decoupled from pyo3 (move pyo3 glue behind a cargo feature).

## Building

    # python only (no rust build):
    pip install -e .            # uses maturin backend
    # or manually:
    cargo build --release -m crates/jutsu-core/Cargo.toml \
      && cp target/release/lib_jutsu.so src/jutsu/_jutsu.so

Tests run against `src/` directly via pytest's `pythonpath = ["src"]`, so no
install step is required during development.
