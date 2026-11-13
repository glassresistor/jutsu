import sys, time; sys.path.insert(0, "src")
from jutsu import rust_eval
from jutsu.environment import Environment
env = Environment()

def chrono(fn, n=8000):
    fn(); t0=time.perf_counter()
    for _ in range(n): fn()
    return (time.perf_counter()-t0)/n*1e6

parts = [
    ("empty", ""),
    ("set only", "{% set x = \'X\' %}"),
    ("set+out", "{% set x = \'X\' %}{{ x }}"),
    ("for 3 idx", "{% for i in [1,2,3] %}{{ loop.index }}{% endfor %}"),
    ("for 3 body", "{% for i in [1,2,3] %}{{ i }}{% endfor %}"),
    ("for if", "{% for i in [1,2,3] %}{% if i % 2 %}o{% else %}e{% endif %}{% endfor %}"),
    ("full stmt", "{% set x = \'X\' %}{% for i in [1,2,3] %}{% if i % 2 %}odd {{ loop.index }}{% else %}even {{ loop.index }}{% endif %}{% endfor %}|{{ x }}|{% with y=3 %}{{ y }}{% endwith %}"),
    ("with only", "{% with y=3 %}{{ y }}{% endwith %}"),
    ("range for 60", "{% for i in range(60) %}{{ i }}{% endfor %}"),
]
for label, src in parts:
    rust_eval.evaluate(env, src)
    ast = rust_eval._cached_compile(env, src, None, None)
    ctx = rust_eval.make_context(env, None, None)
    t = chrono(lambda: rust_eval._jutsu.eval_parsed(ast, ctx, rust_eval._runtime, None, {}))
    print("%-14s %7.2f us" % (label, t))
