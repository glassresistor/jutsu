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
    '{% macro outer() %}{% macro inner() %}i{% endmacro %}{% set g = inner %}{% endmacro %}{{ outer() }}|{{ g() }}',
    '{% macro even(i) %}{% if i == 0 %}yes{% elif i < 0 %}no{% else %}{{ odd(i - 1) }}{% endif %}{% endmacro %}{% macro odd(i) %}{{ even(i - 1) }}{% endmacro %}{{ even(4) }}',
    '{% macro m() %}<{{ loop.index }}>{% endmacro %}{% for i in xs %}{{ m() }}{% endfor %}',
    '{% for i in xs %}{% macro m2() %}<{{ loop.index }}>{% endmacro %}{{ m2() }}{% endfor %}',
    '{% for k, v in tree recursive %}{% macro mm() %}[{{ loop.depth }}]{% endmacro %}{{ mm() }}{% if v %}{{ mm(loop(v)) }}{% endif %}{% endfor %}',
    '{% call (a, b) f(a, 1) %}{{ caller(5, b=6) }}{% endcall %}',
    '{% macro m() %}ab{% endmacro %}{% set y = m() | upper %}[{{ y }}]',
    '{% macro ok(v) %}{{ v }}{% endmacro %}{% for i in xs if ok(i) %}f{% endfor %}',
    '{% call f() %}{% macro cm() %}c{% endmacro %}{{ cm() }}{% endcall %}',
    "{% macro a1() %}{% macro a2() %}{{ rv }}{% endmacro %}{{ a2() }}{% endmacro %}{% set rv = 'R' %}{{ a1() }}",
    '{% macro a1() %}{% macro a2() %}{% set cv = 3 %}{% endmacro %}{{ a2() }}{% endmacro %}{{ a1() }}|{{ cv }}',
    '{% macro m(a=bad) %}x{% endmacro %}{{ m() }}',
    '{% set o.f = 1 %}{% call o.bar %}x{% endcall %}',
    '{% macro m(a, b) %}{{ a }}{{ b }}{% endmacro %}{{ m(1, b=2) }}',
    '{% macro m() %}M{% endmacro %}{% call m() %}A{{ caller() }}{% endcall %}',
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
