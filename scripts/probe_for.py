"""Scratch probe: dump compiled code for loops and check var semantics."""
import sys
sys.path.insert(0, "src")
from jutsu.environment import Environment

env = Environment()
OB = "{" + "%"
CB = "%" + "}"
VO = "{{"
VC = "}}"
tpl = (
    OB + " for i in [1] " + CB
    + OB + " set y = 2 " + CB
    + VO + " y " + VC
    + OB + " endfor " + CB
    + "[" + VO + " y " + VC + "]"
)
print(repr(tpl))
print(env.compile(env.parse(tpl), "t", raw=True))
print("render:", env.from_string(tpl).render())
