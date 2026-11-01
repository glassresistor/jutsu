"""Probe parity for slice C-v6 (autoescape, volatile regions, fold quirks)."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment
from jutsu import rust_eval

CASES = []
def plain(src, vars_=None): CASES.append(("plain", src, vars_ or {}))
def ae(src, vars_=None): CASES.append(("ae", src, vars_ or {}))

plain("{{ 'a' ~ none }}")
plain("{{ ['a' ~ none] }}")
plain("{{ ('x'|upper) ~ none }}")
plain("{{ 'abc'.foo ~ '' if True else 1 }}")
plain("{% set x = 'a' ~ none %}{{ x }}")
ae("{{ '<b>' }}", {"x": "<y>"})
ae("{{ 'a' ~ '<b>' }}")
ae("{{ ('<i>')|safe ~ '<j>' }}")
ae("{{ '&nbsp;'|safe }}{{ '<p>' }}")
ae("{% autoescape true %}static<b>{{ '<t>' }}</b>{% endautoescape %}")
ae("{% autoescape false %}static<b>{{ '<t>' }}</b>{% endautoescape %}", {"x": "<q>"})
ae("A{% block b1 %}<i>bb</i>{% endblock %}B")
ae("{% filter upper %}<u>{% endfilter %}")
ae("{% set x %}<s>{% endset %}[{{ x }}]", {})
ae("{% for i in [1] %}{% autoescape true %}L<i>{{ '<v>' }}</i>{% endautoescape %}{% endfor %}", {"x": "<q>"})
ae("{{ 'a' ~ none }}")

def env_for(kind): return Environment(autoescape=(kind == "ae"))

def py_render(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))

def rust_pair2(env, src, vars_):
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))

bad = 0
for kind, src, vars_ in CASES:
    env = env_for(kind)
    p = py_render(env, src, vars_)
    r = rust_pair2(env, src, vars_)
    mark = "OK" if p == r else "**DIFF**"
    if p != r: bad += 1; print(mark, kind, repr(src), "p=", p[0], "|", p[1:] and p, "\n       r=", r)
print("cases:", len(CASES), "diffs:", bad)
