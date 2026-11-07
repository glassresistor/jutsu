# jutsu

**Jinja2, ported to Rust — with a Python wrapper.**

`jutsu` is a fork of [pallets/jinja](https://github.com/pallets/jinja) (Jinja 3.1.x)
in which the engine internals are being rewritten in Rust, while the Python package stays the public interface. The end state is a native compiled core that
keeps working with the existing Jinja2 extensions, loaders and ecosystem.

> **Status: work in progress.** The lexer, parser and evaluator already run in
> Rust; code generation stays Python by design. The evaluator is wired into
> `Template.render` behind `JUTSU_RUST_EVAL` but not yet the default (slice
> C-v8a wiring landed — see `PLAN_C_DONE.md`). A JavaScript scaffold for the
> future `@jutsu/core` package lives in `js/` (slice E0). Nothing here is
> installed or published yet.

## What it will be

When finished, `jutsu` will be a **full Rust port of Jinja2 shipped behind a
Python wrapper**: the same `Environment` / `Template` API you already know, with
a compiled Rust core doing lexing, parsing and evaluation. Concretely:

```
┌─────────────────────────────────────────────┐
│  Python layer (src/jutsu/)                  │
│    Environment, Template, loaders,          │
│    filters, tests, extensions, Undefined…   │
│              │               │              │
│  Rust core (crates/jutsu-core)              │
│    lexer ─ parser ─ evaluator                │
└─────────────────────────────────────────────┘
```

The Python layer is not a thin shim to be discarded. Runtime values are real
Python objects at every step, and every operation goes through the same helpers
the compiled Jinja engine uses (`Context.resolve`, `Environment.getattr /
getitem / call`, `Environment._filter_test_common`, `runtime.str_join`). That way
behaviour matches *by construction* rather than by reimplementation: `Undefined`
raises the same `UndefinedError`, chained comparisons short-circuit like Python,
and a failing operation propagates the original exception object untouched.

### Planned API (drop-in for Jinja2)

```python
from jutsu import Environment, Template, FileSystemLoader

env = Environment(loader=FileSystemLoader("templates"))
tpl = env.get_template("index.html")
print(tpl.render(name="world", items=[1, 2, 3]))
```

The public surface is the Jinja2 one — `Environment`, `Template`, all the
loaders, `Undefined` / `StrictUndefined` / `ChainableUndefined` /
`DebugUndefined`, `select_autoescape`, the `pass_context` /
`pass_eval_context` decorators, and the full exception hierarchy.

## Current status

| Component | State |
|---|---|
| Python fork (`jinja2` → `jutsu`) | done; original test suite runs unchanged |
| Rust lexer (`crates/jutsu-core/src/lexer.rs`) | **done**, wired into `Lexer.tokeniter` |
| Rust parser (`crates/jutsu-core/src/parser.rs`) | **done**, default engine (`JUTSU_RUST_PARSER=0` to opt out) |
| Packaging (maturin wheel with Rust core) | done |
| Rust evaluator (`crates/jutsu-core/src/eval.rs`) | **done through slice C-v6** (expressions, statements/scoping, filter blocks, macros/call blocks, block inheritance incl. scoped blocks, includes/imports, autoescape/Markup/volatile regions) plus the per-source AST cache (C-v7a); wired into `Template.render` behind `JUTSU_RUST_EVAL` (**default off**) — the C-v8a serve-decision machinery landed (rust `check_supported`, python-side eligibility predicate, per-template verdict cached on the template object); five known divergences remain under the flag before v8b/v8c |
| Code generator / runtime | still Python by design: bytecode emission was rejected, and codegen stays reachable as the fallback path (`PLAN_C_DONE.md`) |
| JS wrapper (`js/`, `@jutsu/core` scaffold) | **slice E0 in progress**: value bridge (Undefined family, Markup/escape, truthiness/str, len/iter/eq/compare/contains), ported filters subset, tests registry, `Environment` shell with decline-not-guess `compile()`/`render()` (`EngineUnavailable` until a backend attaches); node suite green at 157 with a CPython-captured golden replay harness (`js/test/golden.json`) |

### Performance evidence so far

Latest numbers (`scripts/bench_perf.py`, N=3000, part-2a build):

| shape | rust cold | python cold | python warm | rust warm |
|---|---|---|---|---|
| expression `{{ a ~ b }}` | 30.2 µs | 234.6 µs | 5.0 µs | 10.5 µs |
| statement (set/for/if/loop/with) | 148.0 µs | 771.9 µs | 6.3 µs | 21.1 µs |

Cold rust wins ~8× / ~5×; warm rust costs 2.1× / 3.3× of python's cached path.
The completion criteria were retargeted (2026-11-01, `PLAN_C_DONE.md`): a
warm-rust *beat* gate was judged unreachable for a tree-walking interpreter,
so warm loads only need to sit in a comparability band (≤ 4× python warm and
< 50 µs absolute), which these numbers already satisfy.

## How correctness is proven

Every ported component is validated three ways, because "it works on my tests"
is not enough when the goal is a drop-in replacement:

1. **The original Jinja2 test suite** — 900+ tests pass with the Rust core
   active.
2. **Rust unit tests** (`cargo test`) — whitespace control, lstrip/trim blocks,
   raw blocks, line statements/comments, numeric literals, error semantics.
3. **Differential fuzzing** — thousands (48k+ for the parser) of randomly
   generated template sources are processed by *both* implementations and
   compared token-for-token / node-for-node, including error messages, line
   numbers, and partial output produced before an error. The Python
   `tokeniter` is a generator, so partial tokens must be yielded before the
   exception — a detail a naive port would miss.

### Subtleties the ports had to reproduce

Jinja's lexer is a regex-driven state machine; the Rust port replaces the regex
engine with hand-written matchers over `Vec<char>` (Python indexes by code
point) while reproducing CPython `re` semantics exactly:

* Character classes come from tables generated out of CPython itself
  (`scripts/gen_unicode_tables.py`).
* Numeric literals reproduce subtle backtracking: `1_2` matches as an integer
  but not as a float; `010` lexes as integer `0` followed by name `10`.
* The raw begin tag (`{% raw %}`) only accepts the `-be\s* | be` variants — no
  `+be`, no trim-blocks suffix, unlike every other end tag.
* Failure rules `(.)` cannot match at end of input, so a dangling `{#` or
  `{% raw %}` silently ends the stream instead of raising.
* Python's `repr()` formatting is reproduced for error messages
  (`unexpected char '\'' at N`).

Extensions gate the switch: any extension that contributes tags (i18n `trans`,
   `do`, loopcontrol's `break`/`continue`, debug) or overrides
   `filter_stream`/`preprocess` forces the Python parser. The same predicate
   gates rust *serving* (`rust_eval.eligible`), which additionally excludes
   async, sandboxed and native-value environments.

## Roadmap

| Phase | Scope | State |
|---|---|---|
| **A** | Fork Jinja2 as `jutsu`; Rust lexer wired into `Lexer.tokeniter` | ✅ done |
| **B** | Full expression + statement grammar in Rust; AST rebuilt via `ast_rebuild.py`; default flipped | ✅ done |
| **C** | Compiler/evaluator: expressions → statements/scoping → filter blocks → macros → inheritance/includes/autoescape | 🟨 in progress — evaluator done through C-v6, AST cache C-v7a, and the v8a wiring landed behind `JUTSU_RUST_EVAL` (default off); remainder per `PLAN_C_DONE.md`: close C-v8a (five known divergences + serve-side tests/instrumentation), cross-template serving v8b, measurement + default flip v8c |
| **D** | Extensions (`loopcontrols`, `do`, `i18n`, `debug`) gating and third-party compat; loaders/bytecache audit; `NativeEnvironment` exclusion | 🟨 in progress — D1 gating/fallback and D2 third-party extension compat landed; D3/D4 open (dispatchable now, see `PLAN_D_PARALLEL.md`) |
| **E** | JS scaffold + value bridge; split pyo3 glue behind a cargo feature; ship `jutsu-wasm` / `@jutsu/core` for JavaScript | 🟨 started — slice E0 landed as far as the scaffold goes (`js/`, node suite green at 157 incl. golden replay); remaining: pass-kind/lookup-matrix rule suites, then E1 core decoupling, E2 wasm backend, E3 packaging |
| **F** | Release engineering: wheel matrix (manylinux/macOS/Windows), sdist, clippy, versioning story | ⬜ open |

Two structural decisions are already locked in:

* **Tree-walking interpreter in Rust over the Rust AST** — Python bytecode
  emission was rejected. Evidence: rebuilding Python nodes costs more than the
  whole Rust parse (Rust parse is ~5× the Python parser; parse+rebuild only
  ~1.4×), so evaluating directly on the Rust AST is the only path with real
  headroom.
* **Loaders and bytecode caches stay pure Python.** Filesystem/zip/module-cache
  work has no Rust value; revisit only if profiling says otherwise.

Phase E carries an explicit parity caveat: Python filters and extensions are
**not** available in JavaScript, so the JS build supports a documented subset
(upper/lower/join/length/default/escape… plus companions) rather than pretending
to be equivalent; anything unported is *absent* from the registry, never
approximated. The scaffold declines `compile()`/`render()` with
`EngineUnavailable` until a wasm backend attaches — no secret second
interpreter.

## Repository layout

```
pyproject.toml                maturin build backend (Rust core + Python package)
Cargo.toml                    Rust workspace
crates/jutsu-core/
  src/lib.rs                  pyo3 glue exposed to Python (`tokenize`)
  src/lexer.rs                port of Jinja2's lexer
  src/parser.rs               port of Jinja2's parser
  src/eval.rs                 tree-walking evaluator
  src/pyrepr.py … .rs         Python repr() formatting for error messages
  src/unicode_tables.rs       generated tables matching CPython `re`
src/jutsu/                    the Python package (forked from jinja2, imports renamed)
js/                           @jutsu/core JS scaffold (Phase E slice E0): src/*.mjs
                                value bridge, filters subset, tests registry,
                                Environment shell; test/ node:test suites + golden.json
tests/                        forked Jinja2 test suite + differential Rust tests
scripts/                      fuzz drivers, unicode-table generator, benchmarks,
                                js_golden.py (golden capture)
STATUS.md / TODO.md           per-phase status and the ordered to-do list
PLAN_PARSER.md / PLAN_INTERP.md  design docs for the parser and interpreter ports
PLAN_C_DONE.md                retargeted Phase-C criteria and remaining slices
PLAN_D_PARALLEL.md            Phase-D packages, file ownership, conflict matrix
PLAN_JS.md                    Phase-E plan (slices E0–E3) and recorded decisions
docs/EXT_COMPAT.md            D2 third-party extension compatibility memo
```

## Building and testing

Python-only install (no Rust build step needed):

```bash
pip install -e .              # uses the maturin backend
```

Or build the native core manually:

```bash
cargo build --release -m crates/jutsu-core/Cargo.toml \
  && cp target/release/lib_jutsu.so src/jutsu/_jutsu.so
```

Tests run against `src/` directly via pytest's `pythonpath = ["src"]`, so no
install step is required during development:

```bash
pytest                        # forked Jinja2 suite + differential tests
cargo test                    # Rust unit tests
node --test js/test/          # JS scaffold suites (no npm dependencies)
python scripts/js_golden.py   # regenerate the CPython golden capture
```

### Feature flags

| Variable | Default | Effect |
|---|---|---|
| `JUTSU_RUST_LEXER` | `1` | `=0` falls back to the pure-Python lexer (`Lexer._tokeniter_python`) |
| `JUTSU_RUST_PARSER` | `1` | `=0` switches parsing back to the Python parser |
| `JUTSU_RUST_EVAL` | `0` | `=1` lets eligible templates render through the Rust evaluator instead of python codegen (slice C-v8a; default flips at v8c) |

The Rust core is optional at import time: if `_jutsu` cannot be imported,
`jutsu` degrades to the pure-Python implementation rather than failing.

## Versioning and compatibility

The plan is to keep a Jinja-compatible minor version (`jutsu 3.1.x.y`) so that
"drop-in compatible with Jinja2" is a claim that can actually be tested —
including a `jinja2/__init__.py` shim that re-exports `jutsu` and runs Jinja's
*own* test suite against it as the ultimate parity gate.

## License

Same terms as upstream Jinja (BSD-3-Clause, Pallets) — see `LICENSE.txt`.

## Why "jutsu"?

Jinja is some weird rename of ninja also a weird rename of shinobi.  `jutsu` is unique enough
not to be confused, has same first letter and probably will make a cool icon.
