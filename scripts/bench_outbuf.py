import sys; sys.path.insert(0, "src")
import time
from jutsu import rust_eval
from jutsu.environment import Environment
env = Environment()
N = 5000
for label, src in [
    ("many-fragments", "{% for i in range(60) %}{{i}}{% endfor %}"),
    ("markup-mixed",   "{{ safe }}|{% for i in range(30) %}{{i}}{% endfor %}|{{ safe }}"),
]:
    rust_eval._AST_CACHE.clear()
    vars_ = {"safe": "<b>x</b>"} if "safe" in src else {}
    for _ in range(50): rust_eval.evaluate(env, src, variables=vars_)   # warm AST cache
    t0 = time.perf_counter()
    for _ in range(N):  rust_eval.evaluate(env, src, variables=vars_)
    print(f"{label:<16} {(time.perf_counter()-t0)/N*1e6:8.2f} us/op")
