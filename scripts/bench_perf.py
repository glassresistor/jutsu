"""Performance-pass evidence: cached rust evaluation vs python-compiled.

Measures cold and warm costs for both engines on the same templates, using
the per-source AST cache in `rust_eval` (see PLAN_INTERP.md).  Run directly:
    python scripts/bench_perf.py
"""
import sys; sys.path.insert(0, "src")
import time
from jutsu import rust_eval
from jutsu.environment import Environment

N = 3000

EXPR = "{{ a ~ b }}"
STMT = (
    "{% set x = 'X' %}{% for i in [1,2,3] %}"
    "{% if i % 2 %}odd {{ loop.index }}{% else %}even {{ loop.index }}{% endif %}"
    "{% endfor %}|{{ x }}|{% with y=3 %}{{ y }}{% endwith %}"
)

env = Environment()


def chrono(fn, n):
    fn()
    t0 = time.perf_counter()
    for _ in range(n):
        fn()
    return (time.perf_counter() - t0) / n * 1e6


def report(label, us):
    print(f"{label:<44} {us:9.2f} us/op")

for src in (EXPR, STMT):
    print("== template:", repr(src[:58] + ("..." if len(src) > 58 else "")))
    rust_eval._AST_CACHE.clear()

    report("rust cold unique-source eval", chrono(
        lambda: rust_eval.evaluate(env, src + str(time.perf_counter_ns())), N))

    n_cold = [0]
    def py_cold():
        env.from_string(src).render()  # fresh Template each time (parse+compile)
        n_cold[0] += 1
    report("python cold parse+compile+render", chrono(py_cold, N))

    tpl = env.from_string(src)
    report("python warm cached render", chrono(lambda: tpl.render(), N))

    rust_eval.evaluate(env, src)  # prime the AST cache
    report("rust warm eval (cached AST)", chrono(
        lambda: rust_eval.evaluate(env, src), N))

print(f"cache entries after run: {len(rust_eval._AST_CACHE)}")
