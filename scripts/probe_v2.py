"""Quick differential probe for C-v2 statement semantics."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment
from jutsu.runtime import Namespace
from jutsu import rust_eval

env = Environment()
OB, CB, VO, VC = "{%", "%}", "{{", "}}"

def r(t):
    try:
        py = ("ok", env.from_string(t).render())
    except Exception as e:
        py = ("exc", type(e).__name__, str(e))
    try:
        rv = rust_eval.evaluate(env, t)
        if rv is None:
            rs = ("fallback",)
        else:
            rs = ("ok", rv)
    except Exception as e:
        rs = ("exc", type(e).__name__, str(e))
    mark = "  SAME" if py == rs or (rs[0]=="fallback") else "  *** DIFF"
    print(mark, repr(t), py, rs)

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


cases = [
    OB+" set x = 1 "+CB+VO+"x"+VC,
    OB+" for i in [1,2] "+CB+VO+"i"+VC+OB+" endfor "+CB,
    OB+" if 1 "+CB+"yes"+OB+" else "+CB+"no"+OB+" endif "+CB,
    OB+" if false "+CB+"a"+OB+" elif true "+CB+"b"+OB+" else "+CB+"c"+OB+" endif "+CB,
    OB+" set x = 'X' "+CB+OB+" for i in [1,2] "+CB+VO+" x"+VC+OB+" set x = x ~ 'a' "+CB+VO+" x"+VC+OB+" endfor "+CB+"|{{ x }}|",
    OB+" for i in [1,2] "+CB+VO+" y"+VC+OB+" endfor "+CB+OB+" set y = 7 "+CB+OB+" for i in [1,2] "+CB+VO+" y"+VC+OB+" endfor "+CB,
    OB+" if 1 "+CB+OB+" set z = 3 "+CB+VO+" z"+VC+OB+" endif "+CB+"|{{ z }}|",
    OB+" for i in [1,2] "+CB+VO+" loop.index"+VC+OB+" if i==1 "+CB+OB+" set q = 9 "+CB+VO+" q"+VC+OB+" endif"+CB+OB+" endfor "+CB+"({{ q }})",
    OB+" for i in [1,2] "+CB+VO+" loop.cycle('a','b')"+VC+"/"+VC+VO+"loop.first"+VC+OB+" endfor "+CB,
    "x"+OB+" else "+CB+"y"+OB+" endif "+CB+"!",
    OB+" for a in [] "+CB+"A"+OB+" else "+CB+"E"+OB+" endfor "+CB,
    OB+" for a in [1] "+CB+"A"+OB+" else "+CB+"E"+OB+" endfor "+CB,
    OB+" set x %body"+OB+" endset "+CB+VO+"x"+VC,
    OB+" set x | upper %"+OB+" hi "+OB+" endset "+CB+VO+"x"+VC,
    OB+" with a=1 "+CB+VO+"a"+VC+OB+" endwith "+CB+"|{{ a }}",
    OB+" for a, b in [(1,2)] "+CB+VO+"a"+VC+VO+"b"+VC+OB+" endfor "+CB,
    OB+" set ns = namespace(v=0) "+CB+OB+" for i in [1,2] "+CB+OB+" set ns.v = ns.v + i "+CB+OB+" endfor "+CB+VO+" ns.v "+VC,
    "{{ 5 if true else 6 }}",
]
for c in cases:
    r(c)

# --- C-v2 statement / scoping probes -------------------------------------
go(OB + " set y = 2 " + CB + OB + " with y=y+1, z=3 " + CB + VO + "y" + VC
   + VO + "z" + VC + OB + " endwith " + CB + ":" + VO + "y" + VC)
go(OB + " for a in [1] " + CB + OB + " set b | upper %" + VO + "a" + VC
   + OB + " endset " + CB + VO + "b" + VC + OB + " endfor " + CB + "(" + VO + "b" + VC + ")")
go(OB + " for a in [1,2] " + CB + OB + " set b | upper %" + VO + "a" + VC
   + OB + " endset " + CB + VO + "b" + VC + OB + " endfor " + CB + "(" + VO + "b" + VC + ")")
go(OB + " for i in [1,2] " + CB + OB + " if loop.last " + CB + "|L" + OB + " endif " + CB
   + OB + " endfor " + CB)
go(OB + " for i in xs " + CB + VO + "loop.index" + VC + ":" + VO + "loop.revindex" + VC + ":"
   + VO + "loop.length" + VC + ":" + VO + "loop.previtem" + VC + ":" + VO + "loop.nextitem|default('n')" + VC
   + "(" + VO + "loop.changed(i % 2)" + VC + ")" + OB + " endfor " + CB, xs=[1, 2, 3])
go(OB + " for i in [1] " + CB + VO + " loop.revindex0 " + VC + OB + " endfor " + CB)
go(OB + " for k, v in tree recursive " + CB + "<" + VO + " loop.depth " + VC + ":" + VO + "k" + VC + ">"
   + OB + " if v " + CB + VO + " loop(v) " + VC + OB + " endif " + CB + OB + " endfor " + CB,
   tree=[(1, [(2, []), None])])
go(OB + " for i in [1] " + CB + VO + " loop(i) " + VC + OB + " endfor " + CB)
go(OB + " for a,b in 5 " + CB + "x" + OB + " endfor " + CB)
go(OB + " set (a,(b,c)) = (1,2,3)" + CB)
go(OB + " set ns = namespace() " + CB + OB + " set ns.a = 1 " + CB + VO + "ns.a" + VC)
go(OB + " set q = 1 " + CB + OB + " set q.b = 1 " + CB)
go(OB + " for i in [1,2] " + CB + OB + " set w = i*2 " + CB + OB + " endfor " + CB + "(" + VO + "w" + VC + ")")
go(OB + " if true " + CB + OB + " set z = 3 " + CB + VO + "z" + VC + OB + " endif " + CB + "|{{ z }}|")
go(OB + " for i in [1,2] " + CB + VO + "loop.cycle('a','b')" + VC + "/" + VO + "loop.first" + VC
   + OB + " endfor " + CB)
go(OB + " macro m(a) " + CB + "M" + VO + "a" + VC + OB + " endmacro " + CB + "x")
go(OB + " filter upper " + CB + "hi" + OB + " endfilter " + CB)
go(OB + " block b " + CB + "B" + OB + " endblock " + CB)
go(OB + " extends 'x' " + CB)
go(OB + " include 'x' " + CB)
go(OB + " import 'x' as m " + CB)
