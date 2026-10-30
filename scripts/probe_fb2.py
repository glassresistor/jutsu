"""Probe filter-block semantics: rust evaluator vs compiled engine."""
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
    print(tag, repr(t)[:110], "" if py == rs else (py, rs))


if __name__ == "__main__":
    go(OB + " filter upper " + CB + "hi " + VO + "'x'" + VC + OB + " endfilter " + CB)
    go(OB + " filter join('') " + CB + "a{{ 'b' }}c" + OB + " endfilter " + CB)
    go(OB + " filter upper|replace('I','Y') " + CB + "hi" + OB + " endfilter " + CB)
    go(OB + " set q = 1 " + CB + OB + " filter join('') " + CB
       + OB + " set q = 'z' %}" + VO + "q" + VC + OB + " endfilter " + CB + "|{{ q }}|")
    go(OB + " filter upper %}" + OB + " endfilter ")
    go(OB + " filter nosuch %}x" + OB + " endfilter " + CB)
    go(OB + " filter x|upper %}y" + OB + " endfilter ", x=lambda s: s + "!")
    go(OB + " for i in [1,2] " + CB + OB + " filter join('') " + CB + "a{{i}}"
       + OB + " endfilter " + CB + OB + " endfor " + CB)
    go(OB + " filter upper %}" + VO + " nosuch.attr " + VC + OB + " endfilter " + CB)
