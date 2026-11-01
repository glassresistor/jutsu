"""Compute python-engine expected values for v2 cases."""
import sys; sys.path.insert(0, "src")
from jutsu.environment import Environment

env = Environment()
cases = [
    ("{% for k, v in tree recursive %}<{{ loop.depth }}:{{k}}>"
     "{% if v %}{{ loop(v) }}{% endif %}{% endfor %}",
     {"tree": [(1, [(2, []), (3, [])]), (5, [])]}),
    ("{% for i in [1,2] %}{% set w = i*2 %}{% endfor %}({{w}})", {}),
    ("{% for i in xs %}[{{ loop.index }}/{{ loop.revindex }}/{{ loop.length }}"
     "/{{ loop.revindex0 }}]{{ loop.first }}~{{ loop.last }}"
     "{{ loop.cycle('a','b') }}{{ loop.previtem if not loop.first else ''}}"
     "{{ '!' }}{% endfor %}", {"xs": [9, 8, 7]}),
    ("{% for c in 'abc' %}{{ loop.changed(c % 2) }}{% endfor %}", {}),
]
for src, vars_ in cases:
    print(repr(src[:60]), "->", repr(env.from_string(src).render(**vars_)))
