"""Probe recursive-loop codegen."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment

env = Environment()
OB, CB, VO, VC = "{%", "%}", "{{", "}}"


def dump(t):
    print("==== ", repr(t))
    print(env.compile(env.parse(t), "t", raw=True))
    try:
        print("RENDER:", repr(env.from_string(t).render()))
    except Exception as e:
        print("EXC:", type(e).__name__, str(e))


if __name__ == "__main__":
    tree = {"1": ["2", "3"], "4": []}
    t = (
        OB + " for k, v in tree|sort recursive " + CB
        + VO + " loop.depth " + VC + ":" + VO + "k" + VC
        + OB + " for c in v " + CB
        + VO + " loop(c) " + VC
        + OB + " endfor " + CB
        + OB + " endfor " + CB
    )
    dump(t)
    t2 = (
        OB + " for k, v in tree|sort recursive " + CB
        + VO + " loop.depth " + VC + ":" + VO + "k" + VC
        + OB + " for c in v " + CB
        + VO + " loop(c) " + VC
        + OB + " endfor " + CB
        + OB + " endfor " + CB
    )
    print(env.from_string(t2).render(tree=tree))
