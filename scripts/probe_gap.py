import sys; sys.path.insert(0, "src")
sys.path.insert(0, "tests")
from jutsu.environment import Environment
from jutsu.runtime import Namespace
from jutsu import rust_eval

VARS = {
    "a": {"b": 2, "c": "s", "attr": 1},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "ns": Namespace(),
    "loop": {"first": True},
    "c": {1: "one", 2: "two"},
    "d": {"k": "v"},
}

SRC = (
    '{% if x is defined%}{% else %}{% set (a, b) %}{% endset %}'
    '{% for a in x.y%}{% endfor %}{% endif %}| |'
    '{% set ((a, b), c) = ns[[true, True, None]:{0.2349 : -1.2891, 3 : true}:b.foo].bar.bar '
    '>= foo(x=1) != "quote" \'its\'%}'
)

env = Environment()
try:
    print("py :", ("ok", env.from_string(SRC).render(**VARS)))
except Exception as exc:
    print("py :", ("exc", type(exc).__name__, str(exc)))

try:
    rv = rust_eval.evaluate(env, SRC, variables=VARS)
    print("rst:", "fallback" if rv is None else ("ok", rv))
except Exception as exc:
    print("rst:", ("exc", type(exc).__name__, str(exc)))

# minimal reduction: store inside a branch, load later at root
MIN1 = '{% if x is defined %}{% else %}{% set b = 1 %}{% endif %}{{ b }}'
for src in (MIN1,):
    env2 = Environment()
    try:
        print("py :", ("ok", env2.from_string(src).render(**VARS)))
    except Exception as exc:
        print("py :", ("exc", type(exc).__name__, str(exc)))
    try:
        rv = rust_eval.evaluate(env2, src, variables=VARS)
        print("rst:", "fallback" if rv is None else ("ok", rv))
    except Exception as exc:
        print("rst:", ("exc", type(exc).__name__, str(exc)))

MIN2 = '{% if x is defined %}{% else %}{% set b = 1 %}{% endif %}{{ b }}{% set b2 = b %}'
env3 = Environment()
try:
    print("py :", ("ok", env3.from_string(MIN2).render(**VARS)))
except Exception as exc:
    print("py :", ("exc", type(exc).__name__, str(exc)))
try:
    rv = rust_eval.evaluate(env3, MIN2, variables=VARS)
    print("rst:", "fallback" if rv is None else ("ok", rv))
except Exception as exc:
    print("rst:", ("exc", type(exc).__name__, str(exc)))

MIN3 = '{% if x %}{% set b = 9 %}{% endif %}|{{ b }}'
env4 = Environment()
try:
    print("py :", ("ok", env4.from_string(MIN3).render(**VARS)))
except Exception as exc:
    print("py :", ("exc", type(exc).__name__, str(exc)))
try:
    rv = rust_eval.evaluate(env4, MIN3, variables=VARS)
    print("rst:", "fallback" if rv is None else ("ok", rv))
except Exception as exc:
    print("rst:", ("exc", type(exc).__name__, str(exc)))

def show(tag, src):
    env5 = Environment()
    try:
        p = ("ok", env5.from_string(src).render(**VARS))
    except Exception as exc:
        p = ("exc", type(exc).__name__, str(exc))
    try:
        rv = rust_eval.evaluate(env5, src, variables=VARS)
        r = "fallback" if rv is None else ("ok", rv)
    except Exception as exc:
        r = ("exc", type(exc).__name__, str(exc))
    print(tag, "py :", p)
    print(tag, "rst:", r)

MIN4 = '{% if x %}{% set b = 9 %}{% endif %}|{{ b }}{% set q = b %}|{{ q }}'
MIN5 = '{% if x %}{% else %}{% set b = 9 %}{% endif %}|{{ b }}'
MIN6 = '{% if x %}{% set b = 9 %}{% elseif x %}{% set b = 8 %}{% endif %}|{{ b }}'
MIN7 = '{% if x %}{% set b = 9 %}{% else %}{{{ b }}}{% endif %}'
for tag, src in (("m4", MIN4), ("m5", MIN5), ("m6", MIN6), ("m7", MIN7)):
    show(tag, src)
