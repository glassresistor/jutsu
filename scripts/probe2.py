import sys; sys.path.insert(0,"src")
from jutsu.environment import Environment
env = Environment()
OB, CB, VO, VC = "{%","%}","{{","}}"
def dump(t):
    print("==== ", repr(t))
    code = env.compile(env.parse(t), "t", raw=True)
    print(code)
    try:
        print("RENDER:", repr(env.from_string(t).render()))
    except Exception as e:
        print("EXC:", type(e).__name__, str(e))

tpl0 = OB+" for i in [1,2] "+CB+VO+" x"+VC+OB+" set x = i "+CB+VO+" x"+VC+OB+" endfor "+CB+"[{{ x }}]"


tpl2 = OB+" set x = 'X' "+CB+OB+" for i in [1,2] "+CB+VO+" x"+VC+OB+" set x = x ~ 'a' "+CB+VO+" x"+VC+OB+" endfor "+CB+"|{{ x }}|"
dump(tpl2)
tpl3 = OB+" for i in [1,2] "+CB+VO+" y"+VC+OB+" endfor "+CB+OB+" set y = 7 "+CB+OB+" for i in [1,2] "+CB+VO+" y"+VC+OB+" endfor "+CB
dump(tpl3)
