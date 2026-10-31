"""Hand-crafted macro/call-block parity probes (slice C-v3b hardening)."""
import sys
sys.path.insert(0, "src")
sys.path.insert(0, ".")

from jutsu.environment import Environment
from jutsu import rust_eval


class Obj:
    def __init__(self):
        self.bar = 7

    def __call__(self, x=1):
        return f"called:{x}"


def vars_():
    return {"x": 0, "xs": [1, 2], "o": Obj(), "t": lambda a=1, b=2: f"a={a},b={b}",
            "f": lambda *a, **k: f"pos:{list(a)}|kw:{sorted(k.items())}"}


CASES = [
    # closure over root locals updated after define
    "{% macro m() %}<{{ x }}>{% endmacro %}{% set x = 1 %}{{ m() }}",
    "{% macro m() %}<{{ x }}>{% endmacro %}{{ x }}{% set x = 2 %}{{ m() }}{{ m() }}",
    # store inside root before define (cell already exists)
    "{% set y = 5 %}{% macro m() %}[{{ y }}]{% endmacro %}{% set y = 6 %}{{ m() }}",
    # recursive macro
    "{% macro r(i) %}{% if i > 0 %}{{ i }}{{ r(i - 1) }}{% endif %}{% endmacro %}{{ r(3) }}",
    # nested macros closing over outer param
    "{% macro outer(x) %}{% macro inner(y) %}({{ x }}{{ y }}){% endmacro %}"
    "{{ inner(1) }}{% endmacro %}{{ outer(5) }}",
    # macro defined in a for body, called after the loop
    "{% for i in xs %}{% macro m() %}{{ i }}{% endmacro %}{% endfor %}{{ m() }}",
    # macro used inside the same loop iteration
    "{% for i in xs %}{% macro m() %}i={{ i }} {% endmacro %}{{ m() }}{% endfor %}",
    # implicit caller
    "{% macro m() %}A{{ caller() }}B{% endmacro %}{% call m() %}CB{% endcall %}",
    # explicit caller with default
    "{% macro m(x, caller=None) %}[{{ caller() }}]{% endmacro %}{% call m() %}CB{% endcall %}",
    # implicit varargs / kwargs
    "{% macro m(a) %}|{{ varargs }}|{{ kwargs.items()|list }}|{% endmacro %}{{ m(1, 2, 3, k=4) }}",
    # explicit varargs/kwargs params shadow the specials
    "{% macro m(varargs) %}<{{ varargs }}>{% endmacro %}{{ m('v') }}",
    # default expressions evaluated inside the macro frame
    "{% set z = 9 %}{% macro m(a=z) %}[{{ a }}]{% endmacro %}{{ m() }}",
    "{% macro m(a=z) %}[{{ a }}]{% endmacro %}{% set z = 3 %}{{ m() }}",
    # missing parameter -> undefined with hint
    "{% macro m(a) %}({{ a }}){% endmacro %}{{ m() }}",
    # macro object attributes and repr
    "{% macro m(a, b=2) %}x{% endmacro %}{{ m.name }}|{{ m.arguments }}|{{ m }}",
    "{% macro _priv() %}x{% endmacro %}{% set p = _priv() %}{{ p }}",
    # call block with parameters
    "{% call (a) f(a) %}body:{{ a }}{% endcall %}",
    "{% call (a, b=3) f(a, 1) %}{{ a }}{{ b }}{% endcall %}",
    # caller() inside a call block body sees the defining scope
    "{% set q = 'Q' %}{% call f() %}c={{ caller() }}{{ q }}{% endcall %}",
    # nested call blocks
    "{% macro m() %}m{% endmacro %}{% call m() %}{% call m() %}x{% endcall %}{% endcall %}",
    # calling a non-callable target inside a call block
    "{% set n = 5 %}{% call n() %}x{% endcall %}",
    # macro name shadowed by a store
    "{% macro m(a) %}{{ a }}{% endmacro %}{% set m = 5 %}{{ m(1) }}",
    # macro defined inside with, called outside
    "{% with q = 1 %}{% macro m() %}{{ q }}{% endmacro %}{% endwith %}{{ m() }}",
    # macro body that stores into a root var (cell writeback visible later)
    "{% macro m(v) %}{% set x = v %}{{ x }}{% endmacro %}{{ m(7) }}|{{ x }}",
    # call block whose target uses kwargs
    "{% call f(k=1) %}body{% endcall %}",
    # caller stored into a variable then invoked later
    "{% macro m() %}M{% endmacro %}{% call m() %}{% set c = caller %}{{ c() }}{% endcall %}",
    # macro inside a filter block
    "{% filter upper %}{% macro m() %}hi{% endmacro %}{{ m() }}{% endfilter %}",
    # macro defined in a for loop calling itself (recursion through name)
    "{% for i in xs %}{% macro r(j) %}{% if j %}{{ j }}{{ r(j - 1) }}{% endif %}{% endmacro %}"
    "{{ r(2) }} {% endfor %}",
    # loop variable visible to a macro defined in the loop (caller semantics)
    "{% for i in xs %}{% call f() %}{{ caller() }}{{ i }}{% endcall %}{% endfor %}",
    # macro invoked with too many / wrong args
    "{% macro m(a) %}x{% endmacro %}{{ m(1, 2) }}",
    "{% macro m(a) %}x{% endmacro %}{{ m(k=9) }}",
]


def run_py(src):
    env = Environment()
    try:
        return ("ok", env.from_string(src).render(**vars_()))
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


def run_rust(src):
    env = Environment()
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_())
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


def main():
    bad = 0
    for i, src in enumerate(CASES):
        p = run_py(src)
        r = run_rust(src)
        tag = "FALLBACK" if r[0] == "fallback" else ("OK" if r == p else "MISMATCH")
        if tag != "OK":
            bad += 1
        print(f"[{i:2d}] {tag}")
        print("    src=" + repr(src))
        print("    PY  =", p)
        print("    RST =", r)
    print("BAD:", bad, "/", len(CASES))


if __name__ == "__main__":
    main()
