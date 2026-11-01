"""Reproduce a recorded fuzz mismatch with the fuzz VARS."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment
from jutsu.runtime import Namespace

VARS = {
    "a": {"b": 2, "c": "s", "attr": 1},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "ns": Namespace(),
    "loop": {"first": True},
    "foo": None,
    "bar": lambda z=3: z * 2,
    "c": {1: "one", 2: "two"},
}

src = open("scripts/mismatch.txt").read().split("---\n", 1)[1].rsplit("\n===", 1)[0]

env = Environment()
try:
    print("PY:", repr(env.from_string(src).render(**VARS)))
except Exception as e:
    print("PY EXC:", type(e).__name__, str(e))
from jutsu import rust_eval
try:
    rv = rust_eval.evaluate(env, src, variables=VARS)
    print("RUST:", repr(rv))
except Exception as e:
    print("RUST EXC:", type(e).__name__, str(e))
print("--- compiled:")
print(env.compile(env.parse(src), "t", raw=True))
