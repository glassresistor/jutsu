"""Deeper probe for slice C-v6: volatile regions, nesting, inheritance."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment
from jutsu import DictLoader
from jutsu import rust_eval

CASES = [
    ("vol-true", True, "{% autoescape sv %}static<b>{{ '<t>' }}</b>{% endautoescape %}", {"sv": True}, None),
    ("vol-false", True, "{% autoescape sv %}static<b>{{ '<t>' }}</b>{% endautoescape %}", {"sv": False}, None),
    ("vol-undef", True, "{% autoescape nope %}st<i>{{ '<t>' }}</i>{% endautoescape %}", {}, None),
    ("nested-scoped", True, "{% autoescape true %}{% autoescape iv %}X{{ '<t>' }}{% endautoescape %}[{{ '<u>' }}]{% endautoescape %}", {"iv": False}, None),
    ("ae-nested-fold", True, "{% autoescape false %}{% autoescape true %}<n>{{ '<v>' }}</n>{% endautoescape %}Y{{ '<y>' }}{% endautoescape %}Z{{'<z>'}}", {}, None),
    ("vol-fb-target", True, "{% autoescape sv %}{% filter replace('<','!') %}<t>{% endfilter %}O{{ '<o>' }}{% endautoescape %}", {"sv": True}, None),
    ("ae-fb-target", True, "{% filter replace('<','!') %}<t>{% endfilter %}|{{ '<o>' }}", {}, None),
    ("vol-setblock", True, "{% autoescape sv %}{% set y %}<t>{% endset %}[{{ y }}]|{{ '<z>' if y else 'no' }}{% endautoescape %}Q{{ '<q>' }}", {"sv": True}, None),
    ("ae-setblock", True, "{% set y %}<t>{% endset %}[{{ y }}]", {}, None),
    ("vol-rec", True, "{% autoescape sv %}{% for i in xs recursive %}<i>{{ '<a>' ~ none }}</i>{% if loop.first %}{{ loop([]) }}{% endif %}{% endfor %}|{{ '<t>' }}{% endautoescape %}", {"sv": True, "xs": [1]}, None),
    ("ae-rec", True, "{% for i in xs recursive %}<r>{{ '<a>' ~ none }}</r>{% if loop.first %}{{ loop([]) }}{% endif %}{% endfor %}|{{ '<t>' }}", {"xs": [1]}, None),
    ("macro-in-region", True, "{% autoescape sv %}{% macro m() %}<m>{{ '<v>' }}</m>{% endmacro %}{% endautoescape %}A{{ m() }}B", {"sv": False}, None),
    ("plain-env-ae0", False, "{{ 'a' ~ none }}|{% autoescape true %}x<q>{{ '<t>' }}</q>{% endautoescape %}|{{ '<t>' }}", {}, None),
]

def py_render(ae_on, src, vars_):
    env = Environment(autoescape=ae_on)
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))

def rust_render(ae_on, src, vars_):
    env = Environment(autoescape=ae_on)
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return "fallback"
        return ("ok", rv)
    except Exception as exc:
        return ("exc", type(exc).__name__, str(exc))

def inherit_case(ae_on, chain, root, vars_):
    env = Environment(loader=DictLoader(chain), autoescape=ae_on)
    try:
        p = ("ok", env.get_template(root).render(**vars_))
    except Exception as exc:
        p = ("exc", type(exc).__name__, str(exc))
    env2 = Environment(loader=DictLoader(chain), autoescape=ae_on)
    try:
        rv = rust_eval.evaluate(env2, chain[root], name=root, variables=vars_, templates=chain)
        r = "fallback" if rv is None else ("ok", rv)
    except Exception as exc:
        r = ("exc", type(exc).__name__, str(exc))
    return p, r

bad = 0
for nm, ae_on, src, vars_, _x in CASES:
    p = py_render(ae_on, src, vars_)
    r = rust_render(ae_on, src, vars_)
    if p != r or isinstance(r, str):
        bad += 1
        print("**DIFF**", nm, "\np=", p, "\nr=", r)

CHAINS = {
    "P0": "{% autoescape true %}<p>{% block pb %}PB<i>{{ '<pv>' }}</i>{% endblock %}{% endautoescape %}|Q{{ '<q>' }}",
    "C0": '{% extends "P0" %}{% block pb %}CO<o>{{ "<o>" }}</o>{% endblock %}',
}
p, r = inherit_case(True, CHAINS, "C0", {})
print("inherit1:", p == r and not isinstance(r, str), p, r)
if p != r: bad += 1

CH2 = {
    "P": "{% block pb %}<pb>{{ '<p>' }}</pb>{% endblock %}",
    "C": '{% extends "P" %}{% autoescape true %}{% block pb %}<c>{{ "<c>" }}</c>{% endblock %}{% endautoescape %}T{{ "<t>" }}',
}
p, r = inherit_case(True, CH2, "C", {})
print("inherit2:", p == r and not isinstance(r, str), p, r)
if p != r: bad += 1

CH3 = {
    "P": "{% autoescape false %}{% block pb %}<pb>{{ '<p>' }}</pb>{% endblock %}R{{'<r>'}}{% endautoescape %}|S{{'<s>'}}",
    "C": '{% extends "P" %}',
}
p, r = inherit_case(True, CH3, "C", {})
print("inherit3:", p == r and not isinstance(r, str), p, r)
if p != r: bad += 1

print("cases:", len(CASES) + 3, "diffs:", bad)
