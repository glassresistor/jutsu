"""Reproduce fuzz seed-6 diff: unpack error in test-filtered loop."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment

env = Environment()
VARS = {
    "a": {"b": 2, "c": "s", "attr": 1},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "loop": {"first": True},
    "c": {1: "one", 2: "two"},
}
src = ("{{ y.loop }}{% for a in {86 : x.y} if c%}{% set ((a, b), c) = x.b-%}"
       "-{% endfor %}%\n\n")
print(repr(src))
try:
    print(env.compile(env.parse(src), "t", raw=True))
except Exception as e:
    print("PARSE/COMPILE EXC:", type(e).__name__, str(e))
try:
    print("PY RENDER:", repr(env.from_string(src).render(**VARS)))
except Exception as e:
    print("PY RENDER EXC:", type(e).__name__, str(e))
from jutsu import rust_eval
try:
    print("RUST:", repr(rust_eval.evaluate(env, src, variables=VARS)))
except Exception as e:
    print("RUST EXC:", type(e).__name__, str(e))
