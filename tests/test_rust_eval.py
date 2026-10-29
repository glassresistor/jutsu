"""Differential tests for the rust evaluator slice v0 (PLAN_INTERP.md).

Every supported template must render byte-identical to the python engine;
every unsupported template must be reported as such.
"""
import pytest

from jutsu import rust_eval
from jutsu.environment import Environment

try:
    assert rust_eval.has_rust_core()
except AssertionError:  # pragma: no cover
    pytest.skip("rust core not built", allow_module_level=True)


CASES = [
    ("hello", {}, "hello"),
    ("a {{ b }} c", {"b": 42}, "a 42 c"),
    ("{{ 'x' }}{{ 1 }}{{ none }}{{ true }}", {}, "x1NoneTrue"),
    ("{{ x }}", {"x": "y"}, "y"),
    ("{{ u }}", {}, ""),                      # default Undefined renders empty
    ("t ext {{ n }}!", {"n": None}, "t ext None!"),
    ("{{ g }}", {}, "7"),                     # via globals below
    ("line1\n{{ v }}\nline2", {"v": 'dÃ¶'}, "line1\ndÃ¶\nline2"),
    ("{% print 'a', v %}", {"v": 3}, "a3"),
    ("static {{ v }} {{ v }} dyn", {"v": "m"}, "static m m dyn"),
    ("{{ 10 ** 2 if x else 'no' }}", {"x": True}, None),   # unsupported: ops
]

GLOBALS = {"g": 7}


def mk_env():
    env = Environment()
    env.globals.update(GLOBALS)
    return env


def test_eval_slice():
    for src, vars_, want in CASES:
        env = mk_env()
        got = rust_eval.evaluate(env, src, variables=vars_)
        assert (got is not None) == (want is not None), f"support mismatch {src!r}"
        if got is not None:
            assert got == want, f"{src!r}: {got!r} != {want!r}"


def test_matches_python():
    for src, vars_, _want in CASES:
        env = mk_env()
        got = rust_eval.evaluate(env, src, variables=vars_)
        expected = env.from_string(src).render(**vars_)
        if got is None:
            continue
        assert got == expected, f"{src!r}: {got!r} != {expected!r}"


def test_expected_values():
    for src, vars_, want in CASES:
        env = mk_env()
        got = rust_eval.evaluate(env, src, variables=vars_)
        if want is None or got is None:
            continue
        assert got == want, f"{src!r}: {got!r} != {want!r}"


UNSUPPORTED = [
    "{% if 1 %}x{% endif %}",
    "{{ a.b }}",
    "{% for i in [] %}{% endfor %}",
    "{{ x|upper }}",
    "{% set q = 1 %}",
    "{{ a + b }}",
]


def test_unsupported():
    env = Environment()
    for src in UNSUPPORTED:
        assert rust_eval.evaluate(env, src) is None, src


def test_structured_slice(n=3000, seed=91):
    """Cross-validate the v0 subset on structured generated templates: every
    template the python engine renders without error must render identically
    through rust whenever it is supported."""
    import random as _random

    from gen_templates import Gen

    rng = _random.Random(seed)
    g = Gen(rng)
    rendered = unsupported = 0
    for _ in range(n):
        src = g.template()
        env = Environment()
        try:
            expected = env.from_string(src).render()
        except Exception:
            continue  # runtime errors/unsupported constructs: out of v0 scope
        got = rust_eval.evaluate(env, src)
        if got is None:
            unsupported += 1
            continue
        rendered += 1
        assert got == expected, f"eval mismatch {src!r}: {got!r} != {expected!r}"
    # sanity: the generator actually produced supported templates
    assert rendered > 50, f"only {rendered} supported renders"
