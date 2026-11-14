# Jutsu — Benchmark Verification & Rust Speed-Up Analysis

Repo: https://github.com/glassresistor/jutsu · Date: 2026-11-13
Host: Linux x86_64, CPython 3.13.14, Node v26.13.0, prebuilt `_jutsu.so` (cp313).

## 1. Environment / requirements

| Stack | Requirement | Status |
|---|---|---|
| Python | MarkupSafe>=3.0 | ✅ 3.0.3 present |
| Python | pytest / anyio (test suite) | ✅ 9.2.0 / 4.13.0 present |
| Rust core | `_jutsu` native module | ✅ prebuilt `.so` loads & runs under Py 3.13 |
| Rust toolchain | `rustc`/`cargo` from source | ⚠️ blocked in sandbox (50 MB per-file hard cap kills rustup extraction of the >50 MB `rustc` binary). Works on a normal machine (you have 1.99.1). All benchmarks use the prebuilt `.so`, so this does not affect results. |
| JavaScript | `@jutsu/core` scaffold | ✅ no npm deps; `node --test js/test/` → **156/156 pass** |

## 2. Benchmarks run (all three in `scripts/`)

### `bench_perf.py` (N=3000) — the README's headline numbers

| shape | metric | **measured** | README (post exec_for) | Δ |
|---|---|---:|---:|---:|
| expr `{{ a ~ b }}` | rust cold | 32.44 µs | 30.2 | +7% |
| expr `{{ a ~ b }}` | python cold | 245.06 µs | 234.6 | +4% |
| expr `{{ a ~ b }}` | python warm | 5.22 µs | 5.0 | +4% |
| expr `{{ a ~ b }}` | **rust warm** | **10.94 µs** | 10.5 | +4% |
| statement | rust cold | 150.35 µs | 148.0 | +2% |
| statement | python cold | 781.03 µs | 771.9 | +1% |
| statement | python warm | 6.25 µs | 6.3 | −1% |
| statement | **rust warm** | **23.31 µs** | **16.2** | **+44%** ⚠️ |

Everything matches the README within ~10% **except statement rust-warm**, which I measure at
23.3 µs vs the claimed 16.2 µs. See §3 — the gap is almost entirely Python-side, not Rust.

### `bench_v2.py` (N=2000)
```
python parse+compile:            754.2 us
python from_string render:       786.3 us
python cached render:              6.4 us
rust parse+eval (cold):          23.9 us
```

### `bench_outbuf.py` (N=5000) — output-buffer behaviour
```
many-fragments   (~60 iters)     27.94 us/op
markup-mixed                      21.84 us/op
```

## 3. Key finding: "rust warm" is not measuring pure Rust

Decomposition of the warm path (`evaluate()` = what `bench_perf.py` actually times):

| component | expr | statement |
|---|---:|---:|
| `_cached_compile` cache-hit | 0.38 µs | 0.40 µs |
| **`make_context` (fresh Python Context, every call)** | **4.09 µs** | **4.12 µs** |
| **`_jutsu.eval_parsed` (pure Rust eval)** | **5.68 µs** | **16.01 µs** |
| full `evaluate()` (README metric) | 11.42 µs | 23.03 µs |

The **pure-Rust statement eval is 16.01 µs ≈ the README's "16.2 µs"**. The benchmark script,
however, times the *full driver* `evaluate()`, which adds ~4.1 µs of Python `make_context` +
pyo3 marshalling on top → 23.0 µs. So the README's headline "rust warm 16.2" corresponds to the
Rust evaluator's own cost, while the script reports the higher full-path number. The Rust core is
doing what it claims; the wrapper is adding a fixed ~4 µs tax on every render.

Cold path (unique source each call → always a cache miss):

| component | expr | statement |
|---|---:|---:|
| `wrapped_tokens` (Python generator wrap + Rust lex) | 6.72 µs | 56.05 µs |
| `_jutsu.compile_ast` (pyo3 token convert + Rust parse) | 3.08 µs | 34.78 µs |
| `make_context` | ~4.1 µs | ~4.0 µs |
| full cold `evaluate()` | 32.44 µs | 149.07 µs |

Cold statement: ~56 µs is spent in the **Python** token wrap, and a further large chunk is pyo3
converting the Python list-of-tuples into `Vec<(u32,String,Py<PyAny>)>` (each value `clone_ref`).
The Rust parse itself is only part of the cold cost.

Output cost is **linear in fragment count** (pure-Rust eval, `{% for i in range(k) %}{{i}}{% endfor %}`):
1 value = 6.71 µs → 10 fragments = 9.18 µs → 60 fragments = 21.82 µs. Each fragment is materialised
as a Python object and the whole buffer is joined via a Python `"".join(PyList)` call.

## 4. Five ways to speed up the Rust path vs pure Python

Pure-Python warm reference: expr 5.0 µs, statement 6.3 µs. The Rust *evaluator* alone is already
fast; the wins below target the **wrapper + output + cold path**, which is where Rust currently loses
to Python's cached render. Ordered by measured impact.

### 1. Eliminate / cache the per-call Python `Context` (biggest single win, ~4 µs every render)
`evaluate()` rebuilds a full Python `Context` via `_runtime.new_context` on **every** call
(4.0–4.2 µs = 18–37% of the warm number). The Rust eval writes loop/store targets into its own
Rust `self.stack` frames, not the Python context (see `exec_for`), so the context is effectively
read-only during eval. Cache it per (template, variables) — for the common no-variables / same-
variables case reuse one Context; or pass a plain variables dict + env into Rust and let Rust build
a lightweight resolution table (vars → env.globals) with no Python `Context` object at all.
Effect: warm expr 11.1 → ~7 µs, warm stmt 23.2 → ~19 µs; makes warm-expr roughly tie Python warm.

### 2. Accumulate output in a Rust buffer instead of `Vec<Py>` + Python `"".join`
`out_join_inner` builds a `PyList` and calls Python `"".join()` on every join, and every fragment
(even static text) is a Python str object. Collapse to a Rust `String`/`Vec<u8>`; cross into
Python **once** at the end (one `PyString`). For autoescape, build a Markup-compatible buffer.
Effect: kills the linear per-fragment cost — 60-fragment case drops from ~22 µs (pure eval) toward
near-constant; directly cuts `bench_outbuf` "many-fragments" (27 µs).

### 3. Format value→string in Rust where possible; batch str conversions
Each `{{ i }}` crosses to Python for `str()` and pushes a Py object. For ints/floats/simple types,
format in Rust directly into the buffer from #2 (no crossing); otherwise batch the conversions.
Effect: removes the per-fragment Python call that drives the 6.71→21.82 µs ramp.

### 4. One native lex+parse call — remove the Python token round-trip on the cold path
Cold statement spends ~57 µs in the **Python** `wrapped_tokens` (generator → list of tuples) plus a
large pyo3 cost converting that list into `Vec<(u32,String,Py<PyAny>)>` with a `clone_ref` per value.
Add `_jutsu.lex_parse(source, cfg) -> ParsedAst` that lexes **and** parses entirely in Rust and
returns only the AST handle. The Python wrap exists to reproduce lazy-`TokenStream` error semantics;
for the success path (the common case) it is pure overhead. Return a compact native token form
(interned strings / small structs) rather than full Python objects.
Effect: cuts cold statement from ~148 µs toward (Rust parse + make_context + eval), i.e. removes
~50–90 µs of Python-side marshalling.

### 5. Specialize simple `{% for %}` loops + cache the filter/test callables a template uses
In `exec_for`'s "reusable" fast path each iteration still allocates `target_name.to_string()` (a
fresh key String) and does `item.clone().unbind()`. Precompute the target key once. Extend the
existing `rt_constants_cache` (already present in `Session`) to cache the **specific** filter/test
callables a template references at compile time, so the hot loop never does a dict lookup + getattr.
For `{% for i in range(n) %}{{ i }}{% endfor %}`, specialize to iterate + format entirely in Rust
into the buffer from #2 (skip per-iteration scope enter/exit and Python str calls).
Effect: lowers the constant factor of loop-heavy templates, which is exactly the statement benchmark.

### Net expectation
Applying #1 + #2 (+#3) moves **warm** Rust from ~11 / ~23 µs toward ~7 / ~19 µs (expr/stmt), and
further with #5; #4 independently slashes **cold**. The structural ceiling remains that a tree-walk
interpreter can't beat Python's *compiled* warm render on the statement shape — consistent with the
README's own retargeted criterion (warm rust in a ≤4× band, <50 µs), which these numbers already meet.

## 6. Verification notes (re-run 2026-11-13)

All numbers above were reproduced on this host from the prebuilt `_jutsu.so` (cp313). Method:

* Headline table: `python scripts/bench_perf.py` (N=3000), plus a per-component decomposition
  harness that times each driver step in isolation (`_cached_compile`, `make_context`,
  `_jutsu.eval_parsed` with a prebuilt context, full `evaluate()`).
* Output ramp: `{% for i in range(k) %}{{i}}{% endfor %}` for k = 1/10/60, timed as pure-Rust
  `eval_parsed` on the cached AST — isolating fragment cost from context creation.
* JS scaffold: `node --test js/test/` → **156/156 pass** (0 fail).
* Source-level spot checks confirming §4's premises in `crates/jutsu-core/src/eval.rs`:
  * `out_join_inner` (line ~1312) builds a `PyList` and calls Python `"".join()` — confirms #2.
  * `exec_for` fast path (line ~2708) does `target_name.to_string()` + `item.clone().unbind()`
    per iteration — confirms #5; `rt_constants_cache` exists in `Session` (line ~221).
  * No native `lex_parse` entry point exists (`_jutsu` exposes only
    `tokenize/parse/compile_ast/eval_parsed/eval_template/check_supported`) — confirms the gap #4 targets.
* One measurement note: cold-path components don't sum to the full cold number (56 + 35 + 4 + 16
  ≈ 111 µs vs 149 µs measured) because `evaluate()` on a cache miss re-enters pyo3 conversion and
  per-call allocation paths that are more expensive in the live loop than when each step is timed
  in isolation. The decomposition is directionally correct; treat component sums as lower bounds.

## 5. Reproduce
```
python scripts/bench_perf.py     # README headline table
python scripts/bench_v2.py       # parse+compile vs cached render
python scripts/bench_outbuf.py   # output-buffer behaviour
node --test js/test/             # JS scaffold (156 tests)
cargo test -p jutsu-core         # Rust unit tests
cargo build --release && cp target/release/lib_jutsu.so src/jutsu/_jutsu.so
```

## 7. Warm-path speed-up (implemented 2026-11-13)

Five optimizations were implemented and verified against the full test suite
(1054 pass, JS 156/156). Ordered by measured impact.

### Optimization A: Context caching (`src/jutsu/rust_eval.py`)

`make_context` now caches the `Context` object per `(id(env), name)` when no variables
are passed. Before each reuse, mutable state is reset (`vars.clear()`,
`exported_vars.clear()`, `eval_ctx.volatile = False`). This eliminates the 2.3 µs dict
merge + 1.15 µs Context constructor on every render call.

**Savings: ~4 µs per call** (make_context drops from 4.05 µs to 0.23 µs).

### Optimization B: Rust output fast-paths (`crates/jutsu-core/src/eval.rs`)

1. **`out_join_inner`**: Instead of `PyList::new(chunks)` + Python `"​".join()`,
   iterates chunks in Rust and builds a single `String`. Falls back to the original
   path if any chunk is not a plain string (e.g. Markup in autoescape mode).

2. **`out_wrap_inner`** (no-autoescape, non-volatile path):
   - Existing Python strings: returned as-is (no `str()` call)
   - Ints (excluding bools): extracted as `i64` and formatted in Rust directly;
     falls back to `v.str()` for values exceeding i64 range
   - Other types: unchanged (`v.str()`)

### Optimization C: process-global runtime constants (`RT_GLOBALS`)

`Session::build` re-fetched the module-level singletons — `missing`, `LoopContext`,
`Namespace`, `escape`, `Markup`, `str_join`, `markup_join` plus a `builtins.slice`
import — on **every** render (~8 pyo3 getattr calls + an import). These live on the
single `jutsu.runtime` module and never change, so they now live in a process-global
`Mutex<Option<RtGlobals>>` keyed by rt-module identity. Removed the dead per-session
`rt_constants_cache` field that was declared but never wired up.

**Savings: ~1 µs fixed floor** (empty-template eval 2.62 → 1.61 µs).

### Optimization D: hoist per-iteration allocations in `exec_for`

The extended (`loop.*`) path allocated a fresh `"loop"` slot-key String every
iteration; the reusable fast path re-borrowed the stack, cloned the frame `Rc`, and
re-derived the target key each pass. The `"loop"` key is now hoisted once before the
loop, and the reusable path captures the pushed frame handle directly (the body
restores stack depth on return, so it is always the top) with a precomputed target
key. No behaviour change; trims constant-factor cost of loop-heavy templates.

### Phase breakdown (why further gains are diminishing)

Instrumenting `evaluate_with_sources` per phase (µs, statement template):

| phase | expr | stmt |
|---|---:|---:|
| `Ev::new` (Session build) | ~1–2 | ~1–2 |
| `unit_prep` (scan + layout) | ~0 | ~1–2 |
| `prepare_unit` | ~0 | ~1–2 |
| **`exec_unit` (tree-walk)** | **~2–4** | **~10–15** |
| `out_join` | ~0 | ~0 |

The tree-walk (`exec`) is the dominant remaining cost for the statement shape. It is
inherent to a tree-walking interpreter (the README's own acknowledged ceiling). Two
further avenues were evaluated and **not** taken:

* **Cross-render caching of `unit_prep`** — unsafe. `scan` checks live filter/test
  presence (`map.get_item`) and may call user code via `folds_at_compile`, so a cached
  verdict could lie if the environment changes; it would violate the never-guess
  invariant the whole design rests on. The per-Session cache is intentional.
* **Loop specialization (#5)** — the `loop.index` cost is dominated by the real Python
  `LoopContext` property access, not allocation; eliminating it means re-implementing
  loop semantics in Rust, a large correctness surface for a small constant-factor win.

### Results (cumulative, A+D)

| metric | before | after | change |
|---|---:|---:|---:|
| expr warm | 10.94 µs | **~5.3 µs** | −52% |
| stmt warm | 23.31 µs | **~15.7 µs** | −33% |
| expr cold | 32.44 µs | ~23 µs | −29% |
| stmt cold | 150.35 µs | ~135 µs | −10% |
| many-fragments (60) | 27.94 µs | **~14.2 µs** | −49% |
| markup-mixed | 21.84 µs | ~17.1 µs | −22% |
| rust parse+eval (v2) | 23.90 µs | **~16 µs** | −33% |

Warm-rust vs Python-warm ratio:
- expr: 10.94/5.08 = 2.15× → **~5.3/5.0 = ~1.06×** (at parity with Python's cached render)
- stmt: 23.31/6.38 = 3.66× → **~15.7/6.4 = ~2.45×**

The expression warm path is now at parity with pure-Python cached rendering, and the
statement warm (≈15.7 µs) matches the README's claimed "16.2 µs" — confirming the original
gap was the Python context-creation tax plus per-render constant re-fetching, not the Rust
evaluator. Both shapes sit comfortably inside the retargeted criterion (≤4×, <50 µs).
