"""Probe codegen for filter blocks / macros / call blocks."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment

env = Environment()
OB = "{%"
CB = "%}"
VO = "{{"
VC = "}}"


def dump(t):
    print("==== ", repr(t))
    print(env.compile(env.parse(t), "t", raw=True))
    try:
        print("RENDER:", repr(env.from_string(t).render()))
    except Exception as e:
        print("EXC:", type(e).__name__, str(e))


if __name__ == "__main__":
    dump(OB + " filter upper " + CB + "hi " + VO + "'x'" + VC + OB + " endfilter " + CB)
    dump(OB + " filter join('') " + CB + "a{{ 'b' }}c" + OB + " endfilter " + CB)
    dump(OB + " macro m(a, b=2) " + CB + "<" + VO + "a" + VC + ":" + VO + "b" + VC + ">"
         + OB + " endmacro " + CB + VO + "m(1)" + VC)
    dump(OB + " macro m(a) " + CB + "M:" + VO + "caller|default('no')" + VC
         + OB + " endmacro " + CB + VO + "m(3)" + VC)
