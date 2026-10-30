"""Probe: compile-time checks around the special `loop` name."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment
from jutsu import rust_eval

env = Environment()
OB, CB, VO, VC = "{%", "%}", "{{", "}}"


def go(t, **vars):
    try:
        py = ("ok", env.from_string(t).render(**vars))
    except Exception as e:
        py = ("exc", type(e).__name__, str(e))
    try:
        rv = rust_eval.evaluate(env, t, variables=vars)
        rs = ("fallback",) if rv is None else ("ok", rv)
    except Exception as e:
        rs = ("exc", type(e).__name__, str(e))
    tag = "SAME" if py == rs or rs[0] == "fallback" else "***DIFF"
    print(tag, repr(t)[:100], "" if py == rs else (py, rs))


if __name__ == "__main__":
    go(OB + " for loop in [1] %}x{% endfor %}")
    go(OB + " for a in [1] %}{% set loop = 1 %}{% endfor %}")
    go(OB + " if true %}" + OB + " for loop in [1] %}x{% endfor %}{% endif %}")
    go(OB + " macro m(loop) %}x{% endmacro %}y")
    go(OB + " for a in xs recursive %}" + OB + " set loop = 1 %}{% endfor %}", xs=[[1]])
    go("{{ loop }}", loop=3)
