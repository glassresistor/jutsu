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


def tf(*args, **kw):
    c = kw.get("caller")
    if c is not None:
        return "tf-invoked:" + str(c())
    return "tf"


def vars_():
    return {"x": 0, "xs": [1, 2], "o": Obj(), "t": lambda a=1, b=2: f"a={a},b={b}",
            "tf": tf,
            "f": lambda *a, **k: f"pos:{list(a)}|kw:{sorted(k.items())}"}


CASES = [
    '{% with q = 1 %}{% macro m() %}[{{ q }}]{% endmacro %}{% set g = m %}{% endwith %}({{ g() }})',
    '{% for i in xs %}{% macro m() %}[{{ i }}]{% endmacro %}{% set g = m %}{% endfor %}({{ g() }})',
    '{% macro outer() %}{% macro inner() %}[{{ x }}]{% endmacro %}{% set g = inner %}{% endmacro %}{{ outer() }}|({{ g() }})',
    "{% macro m() %}M{% endmacro %}{% set keep = '' %}{% call m() %}{% set keep = caller %}{% endcall %}",
    '{% with q = 1 %}{% macro m() %}[{{ q }}{{ x }}]{% endmacro %}{{ m() }}{% endwith %}',
    '{% filter upper %}{% macro m() %}hi{% endmacro %}a{{ m() }}{% endfilter %}',
    '{% call f(caller=caller) %}x{% endcall %}',
    '{% macro joiner(i) %}{% if i < 4 %}{{ i }}{{ joiner(i + 1) }}{% endif %}{% endmacro %}{{ joiner(0) }}',
    '{% call tf() %}cb-body{% endcall %}',
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
