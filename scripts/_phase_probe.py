import sys; sys.path.insert(0, "src")
from jutsu import rust_eval
from jutsu.environment import Environment
env = Environment()
EXPR = "{{ a ~ b }}"
STMT = ("{% set x = 'X' %}{% for i in [1,2,3] %}"
        "{% if i % 2 %}odd {{ loop.index }}{% else %}even {{ loop.index }}{% endif %}"
        "{% endfor %}|{{ x }}|{% with y=3 %}{{ y }}{% endwith %}")
for _ in range(3): rust_eval.evaluate(env, EXPR)
print("--- expr ---")
for _ in range(4): rust_eval.evaluate(env, EXPR)
for _ in range(3): rust_eval.evaluate(env, STMT)
print("--- stmt ---")
for _ in range(4): rust_eval.evaluate(env, STMT)
