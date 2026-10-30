"""Structured statement-level cross-validation for the rust evaluator."""
import sys; sys.path.insert(0, "src")
import random
sys.path.insert(0, "tests")
from jutsu.environment import Environment
from jutsu.runtime import Namespace
from jutsu import rust_eval
from gen_templates import Gen


class Obj:
    bar = 7

    def __init__(self):
        self.baz = "q"

    def __eq__(self, other):
        return isinstance(other, Obj) and self.bar == other.bar

    def __hash__(self):
        return hash(self.bar)

    def __call__(self, x=1):
        return f"called:{x}"


VARS = {
    "a": {"b": 2, "c": "s", "attr": 1},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "ns": Namespace(),
    "loop": {"first": True},
    "foo": Obj(),
    "bar": lambda z=3: z * 2,
    "c": {1: "one", 2: "two"},
    "d": {"k": "v"},
}


def py_render(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))


def rust_render(env, src, vars_):
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))


def run(seed, n):
    rng = random.Random(seed)
    g = Gen(rng)
    stats = {"ok": 0, "exc": 0, "fb": 0}
    diffs = []
    for _ in range(n):
        src = g.template()
        env = Environment()
        p = py_render(env, src, VARS)
        r = rust_render(env, src, VARS)
        if r[0] == "fallback":
            stats["fb"] += 1
            continue
        if r != p:
            diffs.append((src, p, r))
        else:
            stats[p[0]] = stats.get(p[0], 0) + 1
    print(f"seed={seed} n={n}", stats, "DIFFS:", len(diffs))
    for src, p, r in diffs[:8]:
        print("---", repr(src)[:200])
        print("   py :", p)
        print("   rst:", r)
    return len(diffs)


if __name__ == "__main__":
    total = 0
    for seed in (1, 2, 3):
        total += run(seed, 4000)
    print("TOTAL DIFFS", total)
    sys.exit(1 if total else 0)
