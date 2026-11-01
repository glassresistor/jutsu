# include fuzz corpus generator (slice C-v5)
"""Structured differential fuzzing for slice C-v5 (includes and imports).

Generates small main templates mixing includes, imports and from-imports over a
fixed pool of sub-templates: dynamic targets, ignore-missing flags,
without-context variants, aliases, nested inheritance and error bodies are all
in the mix.  Every template renders through both engines comparing text plus
exception type and message; a rust fallback counts separately from a mismatch:
falling back is allowed (the caller would use python anyway), disagreeing
results are not.

Usage: python scripts/include_fuzz.py [seeds...] [--n=NNN]
"""
import random
import sys

sys.path.insert(0, "src")

from jutsu import DictLoader  # noqa: E401
from jutsu import rust_eval  # noqa: E401
from jutsu.environment import Environment  # noqa: E401

POOL = {
    # exports (root-level macros / plain assignments) matter for from-imports:
    # "a" exports ma and w; its loop-free set v stays local to the module.
    "a": "[A{% macro ma() %}MA{% endmacro %}{% set w = 23 %}",
    "b": "<B{% for i in [1,2] %}i{{ i }}{% endfor %}"
         "{% block bb %}BX{% endblock %}{% set lb = 'LB' %}",
    "c": "{% macro mc(p) %}[C{{ p }}]{% endmacro %}(c)",
    "d": "{% set ns = namespace(q=1) %}D:{{ ns.q }}",
    # included templates may themselves inherit or nest includes
    "k1": "K{" + "% extends 'kbase' %}" + "{% block kb %}KO{{ q }}"
         "{% endblock %}",
    "kbase": "[{% block kb %}BO{% endblock %}]",
    "n2": "(n2-{% include 'a' %})",
    # a body that raises at render time, for error parity through the boundary
    "bad1": "{{ 1 + 'x' }}",
}
NAMES = ["a", "b", "c", "d"]
MISSING = ["gone", "nope2", "k1", "n2", "bad1"]
# exported names per pool member, and which of them are macros
EXPORTS = {"a": ["ma", "w"], "b": ["lb"], "c": ["mc"], "d": []}
MACROS = {"a": ["ma"], "b": [], "c": ["mc"], "d": []}
BASEVARS = {"q": 3, "seq": [1, 2], "dv": "a", "nv": "gone"}
"""Structured differential fuzzing for slice C-v5 (includes and imports).

Generates small main templates mixing includes, imports and from-imports over a
fixed pool of sub-templates: dynamic targets, ignore-missing flags, aliases,
without-context variants, nested inheritance and error bodies are all in the
mix.  Every template renders through both engines comparing text plus exception
type and message; a rust fallback counts separately from a mismatch: falling
back is allowed (the caller would use python anyway), disagreeing results are
not.

Usage: python scripts/include_fuzz.py [seeds...] [--n=NNN]
"""
import random
import sys

sys.path.insert(0, "src")

from jutsu import DictLoader  # noqa: E401
from jutsu import rust_eval  # noqa: E401
from jutsu.environment import Environment  # noqa: E401

POOL = {
    "a": "[A{% macro ma() %}MA{% endmacro %}{% set w = 23 %}",
    "b": "<B{% for i in [1,2] %}i{{ i }}{% endfor %}"
         "{% block bb %}BX{% endblock %}{% set lb = 'LB' %}",
    "c": "{% macro mc(p) %}[C{{ p }}]{% endmacro %}(c)",
    "d": "{% set ns = namespace(q=1) %}D:{{ ns.q }}",
    # included templates may themselves inherit or nest includes
    "k1": "{% extends 'kbase' %}{% block kb %}KO{{ q }}{% endblock %}",
    "kbase": "[{% block kb %}BO{% endblock %}]",
    "n2": "(n2-{% include 'a' %})",
    # a body that raises at render time: errors must propagate through the
    # module boundary untouched on both sides
    "bad1": "{{ 1 + 'x' }}",
}
NAMES = ["a", "b", "c", "d"]
MISSING = ["gone", "nope2"]
EXPORTS = {"a": ["ma", "w"], "b": [], "c": ["mc"], "d": []}
BASEVARS = {"q": 3, "seq": [1, 2], "ty": "n2", "dy": "bad1"}


class G:
    def __init__(self, rng):
        self.rng = rng

    def expr(self):
        return self.rng.choice(["q", "'s'", "seq|length", "q + 1"])

    def body(self, depth=0):
        out = ""
        for _ in range(self.rng.randrange(0, 2)):
            r = self.rng.random()
            if r < 0.5:
                out += "{{ " + self.expr() + " }}"
            elif r < 0.75 and depth < 1:
                out += "{% set t %}" + self.body(depth + 1) + "{% endset %}"
            else:
                out += "|" + str(self.rng.randrange(4))
        return out

    def target(self, for_import):
        r = self.rng.random()
        if r < 0.62:
            pool = NAMES + MISSING
            if not for_import:
                pool += ["k1", "n2", "bad1"]
            return "'" + self.rng.choice(pool) + "'"
        if r < 0.85 and not for_import:
            return self.rng.choice(["ty", "dy"])
        if r < 0.92 and not for_import:
            return "[" + ", ".join("'" + n + "'" for n in self.rng.sample(
                NAMES + MISSING, k=self.rng.randrange(1, 3))) + "]"
        if r < 0.95 and not for_import:
            return "[['gone'], 'a']"
        if r < 0.97 and not for_import:
            return "[]"
        return self.expr()

    def flags(self):
        f = ""
        r = self.rng.random()
        if r < 0.25:
            f += " ignore missing"
        elif r < 0.45:
            f += " without context"
        elif r < 0.65:
            f += " with context"
        return f

    def inc(self):
        tgt = self.target(False)
        if self.rng.random() < 0.14:
            # an include statement inside a for loop body
            return ("{% for i in [1,2] %}{% include " + tgt + self.flags()
                    + "%}{% endfor %}")
        return "{% include " + tgt + self.flags() + "%}"

    def imp(self):
        tgt = self.target(True)
        alias = self.rng.choice(["m", "mo", "_u"])
        f = self.flags()
        if not f:
            f = "" if self.rng.random() < 0.5 else " with context"
        return "{% import " + tgt + " as " + alias + f + "%}"

    def froms(self):
        tgt = self.target(True)
        pool = EXPORTS.get(tgt.strip("'"), [])
        picks = []
        for nm in pool:
            if self.rng.random() < 0.5 and nm not in picks:
                picks.append(nm)
            elif self.rng.random() < 0.25:
                picks.append((nm, "a_" + nm.lower()))
        flat = []
        for p in picks:
            if isinstance(p, tuple):
                flat.append(f"{p[0]} as {p[1]}")
            else:
                flat.append(p)
        if self.rng.random() < 0.25 or not flat:
            picks.append("zzz")
        src = "{% from " + tgt + " import "
        if flat:
            src += ", ".join(flat)
        f = ""
        r = self.rng.random()
        if r < 0.35:
            f = " with context"
        elif r < 0.6:
            f = " without context"
        return src + f + "%}"

    def use_exports(self):
        # trailing references to whatever earlier statements imported; using a
        # name no import defined is fine (both engines render undefined as "")
        r = self.rng.random()
        if r < 0.3:
            return ""
        nm = self.rng.choice(["m", "mo", "_u"])
        out = "{{ " + nm + " }}"
        if r < 0.55:
            out += "|{{ " + nm + ".a }}"
        elif r < 0.7:
            out += "|{{ " + nm + ".zzz }}"
        return out

    def stmts(self, n):
        out = []
        kinds = self.rng.choices(["inc", "imp", "from"], weights=(5, 2, 2), k=n)
        for kind in kinds:
            if kind == "inc":
                out.append(self.inc())
            elif kind == "imp":
                out.append(self.imp())
            else:
                out.append(self.froms())
        return "".join(out)

    def template(self):
        n = self.rng.randrange(0, 3)
        inner = self.stmts(n)
        r = self.rng.random()
        if False:
            pass  # macro / call-block wrappers intentionally disabled: python's
            # own machinery for *invoked* macros swallows some render-time errors
            # into unconsumed generator objects whose repr embeds an object
            # address, so such shapes cannot be compared at all; the fixed cases
            # cover includes inside macro bodies where nothing raises
        if r < 0.30:
            # statements inside a scoped block invocation, overridden through a
            # real inheritance chain (same-template overrides would be a
            # duplicate-block compile error on both engines)
            return ("{% extends 'kbase' %}{% block sc scoped %}" + inner
                    + "{% endblock %}|x|" + self.body())

        return self.body() + inner + self.use_exports() + self.body()


def render_py(src):
    files = dict(POOL)
    files["main"] = src
    env = Environment(loader=DictLoader(files))
    try:
        return ("ok", env.get_template("main").render(**BASEVARS))
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


def render_rust(src):
    files = dict(POOL)
    files["main"] = src
    env = Environment(loader=DictLoader(files))
    others = {k: v for k, v in files.items() if k != "main"}
    try:
        rv = rust_eval.evaluate(env, src, name="main", variables=dict(BASEVARS),
                                templates=others)
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))
    if isinstance(rv, str):
        return ("ok", rv)
    return ("fallback",)


def run(seed, n):
    rng = random.Random(seed)
    g = G(rng)
    ok = exc = fb = bad = 0
    for _ in range(n):
        src = g.template()
        p = render_py(src)
        r = render_rust(src)
        if r[0] == "fallback":
            fb += 1
            continue
        if p != r:
            bad += 1
            print(f"MISMATCH {src!r}\n  py={p!r}\n rust={r!r}")
        else:
            if p[0] == "ok":
                ok += 1
            else:
                exc += 1
    return ok, exc, fb, bad


def main():
    seeds = []
    n = 250
    for arg in sys.argv[1:]:
        if arg.startswith("--n="):
            n = int(arg.split("=", 1)[1])
        else:
            seeds.append(int(arg))
    if not seeds:
        seeds = [1, 2, 3]
    tot = [0, 0, 0, 0]
    for seed in seeds:
        c = run(seed, n)
        ok, exc, fb, bad = c
        tot[0] += ok; tot[1] += exc; tot[2] += fb; tot[3] += bad
        print(f"seed {seed}: ok={ok} exc={exc} fallbacks={fb} mismatches={bad}")
    print(f"TOTAL ok={tot[0]} exc={tot[1]} fb={tot[2]} mismatches={tot[3]}")
    if tot[3]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
