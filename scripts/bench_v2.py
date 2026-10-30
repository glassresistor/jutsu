"""Perf evidence for slice C-v2 (statement evaluation in rust)."""
import sys; sys.path.insert(0, "src")
import time
from jutsu.environment import Environment
from jutsu import rust_eval

env = Environment()
tpl = (
    "{% set x = 'X' %}{% for i in [1,2,3] %}"
    "{% if i % 2 %}odd {{ loop.index }}{% else %}even {{ loop.index }}{% endif %}"
    "{% endfor %}|{{ x }}|{% with y=3 %}{{ y }}{% endwith %}"
)
N = 2000

t0 = time.perf_counter()
for _ in range(N):
    env.compile(env.parse(tpl), "t")
t1 = time.perf_counter()
print(f"python parse+compile: {(t1-t0)/N*1e6:.1f} us")

t0 = time.perf_counter()
for _ in range(N):
    env.from_string(tpl).render()
t1 = time.perf_counter()
print(f"python from_string render (incl compile): {(t1-t0)/N*1e6:.1f} us")

compiled = env.from_string(tpl)
t0 = time.perf_counter()
for _ in range(N):
    compiled.render()
t1 = time.perf_counter()
print(f"python cached render: {(t1-t0)/N*1e6:.1f} us")

t0 = time.perf_counter()
for _ in range(N):
    rust_eval.evaluate(env, tpl)
t1 = time.perf_counter()
print(f"rust parse+eval: {(t1-t0)/N*1e6:.1f} us")
