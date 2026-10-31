"""Inheritance-focused structured differential fuzzing (slice C-v4).

Generates small inheritance chains with random block bodies, `super()` calls,
nested blocks, required/scoped flags and dynamic extends, then renders every
template of the chain through both engines comparing text plus exception type
and message.  A rust fallback counts separately from a mismatch: falling back
is allowed (the caller would use python anyway), disagreeing results are not.

Usage: python scripts/block_fuzz.py [seeds...] [--n=NNN]
"""
import random
import sys

sys.path.insert(0, "src")

from jutsu import DictLoader  # noqa: E402
import jutsu.runtime as _runtime  # noqa: E402
from jutsu import rust_eval  # noqa: E402
from jutsu.rust_parse import wrapped_tokens  # noqa: E402
from jutsu.environment import Environment  # noqa: E402

VARS = {"x": 3, "ys": "YS", "seq": [1, 2]}
NAMES = ["a", "b", "c"]


class BGen:
    def __init__(self, rng):
        self.rng = rng

    def expr(self):
        return self.rng.choice(["x", "'s'", "ys", "seq|length", "x + 1"])

    def body(self, depth):
        out = ""
        for _ in range(self.rng.randrange(0, 3)):
            r = self.rng.random()
            if r < 0.45:
                out += "{{ " + self.expr() + " }}"
            elif r < 0.6 and depth < 2:
                out += "{% set q %}" + self.body(depth + 1) + "{% endset %}"
            else:
                out += "|" + str(self.rng.randrange(5))
        return out

    def block(self, name):
        flags = ""
        r = self.rng.random()
        if r < 0.12:
            flags = " required"
        elif r < 0.26:
            flags = " scoped"
        inner = self.body(0)
        if self.rng.random() < 0.45:
            nname = self.rng.choice(NAMES + ["deep", "nest"])
            inner += "{% block " + nname + " %}N{% endblock %}"
        if self.rng.random() < 0.45:
            return ("{% block " + name + flags + " %}<S{{ super() }}>I"
                    + inner + "{% endblock %}")
        return "{% block " + name + flags + " %}" + inner + "{% endblock %}"

    def self_ref(self):
        nm = self.rng.choice(NAMES + ["deep", "nest"])
        if self.rng.random() < 0.5:
            return "{{ self." + nm + "() }}"
        return ""

    def chain(self, levels=3):
        srcs = {}
        base_blocks = [self.block(n) for n in NAMES]
        srcs["t0"] = "".join(base_blocks) + self.body(0) + self.self_ref()
        prev = "t0"
        for lvl in range(1, levels):
            parts = []
            if self.rng.random() < 0.3:
                dyn = ("{% if x %}{% extends '" + prev + "' %}"
                       "{% else %}{% extends 't0' %}{% endif %}")
                parts.append(dyn)
            else:
                parts.append("{% extends '" + prev + "' %}")
            for nm in NAMES:
                if self.rng.random() < 0.55:
                    parts.append(self.block(nm))
            parts.append(self.body(0))
            parts.append(self.self_ref())
            key = "t" + str(lvl)
            srcs[key] = "".join(parts)
            prev = key
        return srcs


def py_render(env, name):
    try:
        return ("ok", env.get_template(name).render(**VARS))
    except Exception as e:  # noqa: BLE001
        return ("exc", type(e).__name__, str(e))


def rust_render(env, name, sources, reasons=None):
    try:
        rv = rust_eval.evaluate(
            env, sources[name], name=name, variables=VARS, templates=sources,
            with_reason=True)
    except Exception as e:  # noqa: BLE001 -- parse/runtime errors compare too
        return ("exc", type(e).__name__, str(e))
    if isinstance(rv, str):
        return ("ok", rv)
    if isinstance(rv, tuple):
        reason = rv[1]
        if reasons is not None:
            key = reason.split(" ")[0] if isinstance(reason, str) else str(reason)
            reasons[key] = reasons.get(key, 0) + 1
        return "fallback"
    raise AssertionError(f"unexpected result type {rv!r}")


def main():
    args = sys.argv[1:]
    n_each = 300
    seeds = []
    for a in args:
        if a.startswith("--n="):
            n_each = int(a.split("=")[1])
        else:
            seeds.append(int(a))
    if not seeds:
        seeds = [1, 2]
    ok = excn = fb = bad = 0
    reasons: dict[str, int] = {}
    for seed in seeds:
        rng = random.Random(seed)
        g = BGen(rng)
        for _ in range(n_each):
            sources = g.chain()
            penv = Environment(loader=DictLoader(sources))
            renv = Environment(loader=DictLoader(sources))
            for nm in sorted(sources.keys()):
                pv = py_render(penv, nm)
                rr = rust_render(renv, nm, sources, reasons)
                if rr == "fallback":
                    fb += 1
                    continue
                if pv != rr:
                    bad += 1
                    print("MISMATCH seed=%s tmpl=%s" % (seed, nm))
                    print("   py:", pv)
                    print("  rst:", rr)
                    print(" src:", sources[nm][:240])
                else:
                    if pv[0] == "ok":
                        ok += 1
                    elif pv[0] == "exc":
                        excn += 1
        top = sorted(reasons.items(), key=lambda kv: -kv[1])[:8]
        print("seed %s cumulative: ok=%d exc=%d fb=%d bad=%d; reasons=%r"
              % (seed, ok, excn, fb, bad, top))
    print("TOTAL ok=%d exc=%d fallbacks=%d mismatches=%d" % (ok, excn, fb, bad))


if __name__ == "__main__":
    main()
