"""Macro-focused structured differential fuzzing (slice C-v3b)."""
import random
import sys

sys.path.insert(0, "src")
sys.path.insert(0, "tests")

from jutsu.environment import Environment
from jutsu import rust_eval


class Obj:
    bar = 7

    def __init__(self):
        self.baz = "q"

    def __call__(self, x=1):
        return f"called:{x}"


def make_vars():
    def t(a=1, b=2, **kw):
        parts = [f"a={a}", f"b={b}"]
        c = kw.pop("caller", None)
        if c is not None:
            parts.append("caller:" + str(c()))
        rest = sorted(kw.items())
        return "|".join(parts + [f"{k}={v}" for k, v in rest])

    def strict(a):
        return f"strict:{a}"

    return {"t": t, "strict": strict, "foo": Obj(), "x": 3, "y": "ys",
            "a": [1, 2], "d": {"k": "v"}, "m": 5}


NAMES = ["p", "q", "r", "s"]
SPECIALS = ["caller", "kwargs", "varargs"]


class MGen:
    def __init__(self, rng):
        self.rng = rng

    def pick(self, *opts):
        return self.rng.choice(list(opts))

    def val(self):
        return self.pick("1", "'s'", "x", "y", "'a'", "2", "d", "'q'")

    def param_list(self):
        # duplicate parameter names make python's own compiler fail with a
        # SyntaxError embedding generated line numbers; keep them rare so the
        # interesting paths stay reachable.
        n = self.rng.randrange(0, 4)
        pool = NAMES + SPECIALS
        names = self.rng.sample(pool, min(n, len(pool))) if n else []
        if self.rng.random() < 0.02 and len(names) >= 1:
            names.append(names[0])  # rare duplicate corner
        parts = []
        for nm in names:
            if self.rng.random() < 0.5:
                parts.append(f"{nm}={self.val()}")
            else:
                parts.append(nm)
        return ", ".join(parts)

    def body(self, depth):
        out = ""
        for _ in range(self.rng.randrange(1, 3)):
            c = self.rng.random()
            if c < 0.35:
                out += "{{ " + self.expr() + " }}"
            elif c < 0.5 and depth < 2:
                out += "{% set " + self.rng.choice(NAMES) + " = " + self.val() + " %}"
            elif c < 0.62 and depth < 2:
                out += "{% if " + self.expr() + " %}" + self.body(depth + 1) \
                     + "{% else %}" + self.body(depth + 1) + "{% endif %}"
            elif c < 0.72 and depth < 2:
                out += "{% for " + self.rng.choice(NAMES) + " in a %}" \
                     + self.body(depth + 1) + "{% endfor %}"
            else:
                out += self.pick("text ", "|", "\n")
        return out

    def expr(self):
        c = self.rng.random()
        if c < 0.35:
            return self.rng.choice(NAMES + ["caller", "kwargs", "varargs", "x", "y"])
        if c < 0.55:
            inner = self.rng.choice(NAMES + ["caller", "kwargs", "varargs"])
            args = self.args()
            return f"{inner}({args})"
        if c < 0.7:
            return f"{self.expr()} ~ {self.val()}"
        return self.val()

    def args(self):
        out = []
        n = self.rng.randrange(0, 3)
        for _ in range(n):
            out.append(self.val())
        keys = []
        k = self.rng.randrange(0, 2)
        for _ in range(k):
            key = self.pick("k", "z", "caller")
            if self.rng.random() < 0.12:
                # python-keyword kwargs take the code generator's dict-literal
                # workaround, where duplicate keys silently lose to the last
                key = self.pick("class", "if", "lambda")
            keys.append(key)
            out.append(f"{key}={self.val()}")
        # duplicate keyword arguments: python's codegen turns these into a
        # SyntaxError unless the workaround path applies
        if len(keys) == 1 and self.rng.random() < 0.25:
            out.append(f"{keys[0]}={self.val()}")
        return ", ".join(out)

    def macro_def(self):
        nm = self.pick("mac", "m2", "outer")
        params = self.param_list()
        sig = f"({params})" if params else ""
        return "{% macro " + nm + sig + " %}" + self.body(0) + "{% endmacro %}"

    def call_block(self):
        params = self.param_list()
        target = self.pick("t(x)", "t()", "strict(x)", "mac(x)")
        sig = f"({params}) " if params else ""
        return "{% call" + sig + target + " %}" + self.body(0) + "{% endcall %}"

    def template(self):
        out = ""
        n = self.rng.randrange(1, 5)
        for _ in range(n):
            c = self.rng.random()
            if c < 0.4:
                out += self.macro_def()
            elif c < 0.6:
                out += self.call_block()
            elif c < 0.75:
                out += "{% for y in a %}" + self.macro_def() + "{{ mac() }}" \
                     + "{% endfor %}"
            elif c < 0.85:
                out += "{% with q=1 %}" + self.macro_def() + "{{ mac() }}" \
                     + "{% endwith %}"
            else:
                out += self.body(0)
        return out


def py_render(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


def rust_render(env, src, vars_):
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


def run(seed, n):
    rng = random.Random(seed)
    g = MGen(rng)
    stats = {}
    diffs = []
    for _ in range(n):
        src = g.template()
        vars_ = make_vars()
        env = Environment()
        p = py_render(env, src, vars_)
        r = rust_render(env, src, vars_)
        key = (r[0] == p[0]) and (r != ("fallback",))
        stats[str(r[0])] = stats.get(str(r[0]), 0) + 1
        if r != ("fallback",) and r != p:
            diffs.append((src, p, r))
    print(f"seed={seed} n={n}", stats, "DIFFS:", len(diffs))
    for src, p, r in diffs[:6]:
        print("----\nSRC:", src)
        print("  PY :", p)
        print("  RST:", r)
    if diffs:
        with open("scripts/macro_mismatch.txt", "w") as fh:
            for i, (src, p, r) in enumerate(diffs):
                fh.write(f"[{i}] seed={seed}\nPY={p!r}\nRST={r!r}\n---\n{src}\n===\n")
    return len(diffs)


if __name__ == "__main__":
    seeds = tuple(int(x) for x in sys.argv[1:]) or (1,)
    total = 0
    for s in seeds:
        total += run(s, 3000)
    print("TOTAL", total)
