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
| Rust evaluator (`crates/jutsu-core/src/eval.rs`) | **slice C-v2/C-v3a/C-v3b: statements, scoping and macros/call blocks evaluate fully in rust** -- if/elif/else, for (incl. `if=` test filters, else branches, recursive loops with `loop()` recursion, full LoopContext features), set/set-block(+filter), filter blocks, with, namespace refs, `{% macro %}` / `{% call %}` with caller/kwargs/varargs closures; compiled-engine scope semantics reproduced (slot frames, alias-copy on store, per-scope resolve loads, root writeback+exported_vars). Not wired into `Template.render` yet; validated differentially incl. exception type/message parity |
| Code generator / runtime | still python (next targets: block inheritance, includes/imports) |

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

## Evaluator port notes (slice C-v1)

The evaluator consumes the rust parser's `Av` tree directly -- no python node
rebuild, no codegen.  Every runtime value is a real python object and every
operation goes through the same helper the compiled engine uses
(`Context.resolve`, `Environment.getattr/getitem/call`,
`Environment._filter_test_common`, `runtime.str_join`), so behaviour matches
by construction rather than by reimplementation: undefined objects raise the
same `UndefinedError`, chained comparisons short-circuit like python, and a
failing operation propagates the original exception object untouched (the
compiled engine re-raises it through `Environment.handle_exception`).  Parse
errors get the same `translated` treatment `Environment.compile` applies so
`str()` of them carries just the message.

Validated by fixed cases plus a structured corpus of 6000 generated expression
templates comparing rendered text *and* exception type/message; anything the
slice cannot express -- statements, dynamic call arguments, autoescape/Markup
(the code generator folds constant output children through the non-markup-aware
`Concat.as_const`), environment finalize functions, volatile eval contexts --
is reported as unsupported so callers fall back to the python engine.

## Evaluator port notes (slice C-v2)

Statements execute with the compiled engine's scope model reproduced in rust:
a runtime scope stack mirrors the code generator's frames, and each hard scope
(root, for-body, for-else, with, set-block body) owns slot variables whose
initialization follows the first source occurrence of the name -- parameters
are not loaded, stores alias-copy from the nearest owning ancestor slot (or
stay `missing`), and loads own a slot only when no scope in the chain owns it
(then they resolve from the python context at each entry).  Tuple/list targets
unpack with python's own sequence-unpack semantics and error messages; the
special `loop` variable is provided by a live `LoopContext` (extended loops),
and recursive loops register their loop object so calling it renders the next
recursion level in rust exactly like the generated `loop(...)` call.
Root-level assignments write back into `context.vars` and update
`exported_vars`; stores inside loops/blocks stay local (`_loop_vars`).  The
`{% with %}` values evaluate against the outer scopes (the generator visits
each expression with the parent frame).

A pre-evaluation scan reproduces the code generator's compile-time checks over
the whole tree: nodes outside the evaluated subset (even in never-executed
positions), missing filter/test names, and assignment to `loop` inside any
for-subtree (raised as the same translated `TemplateAssertionError`).  This
closes the gap where python fails at compile time for code the evaluator would
never reach (`{% if false %}{% for loop in [] %}...` etc.).

Perf evidence (statement template, 2000 iterations, this machine): python
parse+compile 735 us, compile+render 770 us, *cached* render 6.2 us; rust
parse+eval 129 us (~6x faster cold, still far behind a warm python cache --
the AST-cache prerequisite before flipping any default remains).

Validated by 40 fixed statement/scope cases plus a structured corpus of 12k
generated full templates across three seeds comparing rendered text *and*
exception type/message (`tests/test_rust_eval.py`, `scripts/fuzz_v2.py`);
zero mismatches.  Deferred on purpose: macros, filter/call blocks, block
inheritance and includes (dedicated slices C-v3/C-v4), dynamic call arguments,
autoescape/Markup, environment finalize functions and volatile eval contexts
-- all reported as unsupported so callers fall back to the python engine.

The `if=` test of a for loop runs in its own frame (mirroring the generated
filter function): its loads resolve from the context and never observe a store
of the same name made by the loop body -- merging those symbols would make
`{% for a in xs if t %}{% set t = ... %}` skip every iteration.  Recursive
loops combined with `if=` are reported as unsupported until the reciter-wrapping
generator path exists.

Slice C-v3a adds filter blocks: body renders into a buffer inside its own
scope frame, the implicit filter target receives the joined buffer and the
chain is applied through `_filter_test_common`, then the result goes through
the normal output finalize.  The symbol-occurrence order follows
`analyze_node` (filter expression before body) for both filter blocks and set
blocks; loop frames analyze target+body before the test expression registers
its loads -- matching when a later occurrence must not override an earlier
decision.

## Evaluator port notes (slice C-v3b)

Macros and call blocks are the first part of the evaluator that must outlive
the statement that defined them, so the evaluator shares its `Session` through
an `Rc` and every macro closure keeps the *defining frame chain* as
`Vec<Rc<RefCell<Scope>>>`: popped frames stay alive (and writable) through
those `Rc`s exactly like the compiled engine's python closures over root
locals, so a macro invoked later sees the current value of a captured
variable -- including variables assigned after the definition.

A `{% macro %}` creates a real `runtime.Macro` python object whose `func` is a
rust callable (`MacroFunc` pyclass), so argument matching, arity errors,
`macro 'm' takes not more than N argument(s)`, the two-values-for-caller
guard and the `<Macro name>` repr all run through python's own `Macro.__call__`
-- byte-identical semantics by construction.  Invoking enters a fresh scope
frame initialized from the define-time symbol analysis (`macro_body` plus the
occurrence order parameters, defaults, body), binds parameters (a missing one
either evaluates its default expression inside the macro frame or builds
`environment.undefined("parameter 'x' was not provided", name=...)`), renders
the buffered body and returns it joined through `Environment.concat`.
Implicit `caller`/`kwargs`/`varargs` follow the code generator: undeclared
loads of the specials mark `accesses_*`, an explicit parameter drops the
name from tracking, and only the *last* `caller` parameter is remembered for
the "must be omitted or be given a default" assertion.

One systematic divergence is handled by falling back instead of trying to
reproduce it: python's code generator writes keyword arguments into generated
source, so a duplicated key (`{{ f(a=1, a=2) }}`, `{{ x|filter(b=1, b=2) }}`,
a call block whose target already spells `caller=`) or a duplicated macro
parameter makes CPython fail with a `SyntaxError` whose message embeds
generated line numbers (`keyword argument repeated: a (<template>, line 12)`)
that no interpreter can echo.  The scan reports those unsupported; when any
keyword key is itself a python keyword the generator takes its dict-literal
workaround instead, where duplicates simply lose to the later entry -- which
is what the interpreter does, so those render normally.

Validated by 41 fixed macro/call-block cases (closure lifetime across
`for`/`with`/filter scopes, recursion and mutual recursion, nested macros,
the caller specials, error parity) plus a macro-focused structured corpus of
27k generated templates over eight seeds comparing rendered text *and*
exception type/message (`scripts/macro_fuzz.py`): zero mismatches; the only
fallback reasons in the whole corpus are the two codegen `SyntaxError` cases
above.

Branch-aware symbol merging (gap found by fuzz seed 2, now closed): the
occurrence walk collects structured occurrences per `if` statement (the test,
then one group per branch, with the elif chain as nested structure), and the
slot-kind builder evaluates each branch group against an independent copy of
the region state before merging them exactly like python's
`Symbols.copy`/`branch_update`: names stored only inside branches become
RESOLVE loads (ALIAS when an ancestor scope owns them) -- a store that may
never have executed cannot shadow the context value with a missing slot.
Nested ifs recurse; stores already made unconditionally before the merge are
not demoted.  Validated by nine fixed branch-merge cases including the
original seed-2 template plus fuzz seeds 1-3 x8k and the full suite.

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
