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
    ("{{ 10 ** 2 if x else 'no' }}", {"x": True}, "100"),
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
    "{{ f(*args) }}",
    "{{ f(**kw) }}",
    "{{ f(a, *b, **c) }}",
    "{% macro m(a) %}x{% endmacro %}",
    "{% filter upper %}hi{% endfilter %}",
    "{% block b %}B{% endblock %}",
    "{% extends 'x' %}",
    "{% include 'x' %}",
    "{% call m() %}x{% endcall %}",
]


def test_unsupported():
    env = Environment()
    for src in UNSUPPORTED:
        assert rust_eval.evaluate(env, src) is None, src


def test_missing_filter_falls_back():
    """A missing filter/test must not be decided by the rust evaluator: the
    python engine knows whether it is a compile-time or runtime error.
    """
    env = Environment()
    assert rust_eval.evaluate(env, "{{ 'a'|nosuch }}") is None
    assert rust_eval.evaluate(env, "{{ 1 if x else 'a'|nosuch }}") is None
    assert rust_eval.evaluate(env, "{{ 1 is novelt }}") is None


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


# ---------------------------------------------------------------------------
# slice v1: expression evaluation

EXPR_CASES = [
    ("{{ 1 + 2 * 3 }}", {}, "7"),
    ("{{ 'a' ~ 1 ~ true }}", {}, "a1True"),
    ("{{ [1, 2, 3][1] }}", {}, "2"),
    ("{{ {'a': 1}['a'] }}", {}, "1"),
    ("{{ (1,) }}", {}, "(1,)"),
    ("{{ 5 > 3 > 1 }}", {}, "True"),
    ("{{ 5 > 7 > 1 }}", {}, "False"),
    ("{{ 1 if false else 'x' }}", {}, "x"),
    ("{{ nosuchvar }}", {}, ""),
    ("{{ 'abc'|upper }}", {}, "ABC"),
    ("{{ 'a'|replace('a', 'b') }}", {}, "b"),
    ("{{ 3 is odd }}", {}, "True"),
    ("{{ 3 is not odd }}", {}, "False"),
    ("{{ 'abcdef'[1:4] }}", {}, "bcd"),
    ("{{ [1, 2][1:] }}", {}, "[2]"),
    ("{{ 2 ** -1 }}", {}, "0.5"),
    ("{{ 7 % 3 }}", {}, "1"),
    ("{{ 7 // 2 }}", {}, "3"),
    ("{{ 'a' in ['a', 'b'] }}", {}, "True"),
    ("{{ '' or 'x' }}", {}, "x"),
    ("{{ true and false }}", {}, "False"),
    ("{{ -3 if 1 else 4 }}", {}, "-3"),
    ("text {{ a.b }} and {{ d['k'] }} more",
     {"a": {"b": 3}, "d": {"k": "v"}}, "text 3 and v more"),
]


def test_expr_cases():
    env = Environment()
    for src, vars_, want in EXPR_CASES:
        got = rust_eval.evaluate(env, src, variables=vars_)
        assert got is not None, f"v1 case unsupported: {src!r}"
        assert got == want, f"{src!r}: {got!r} != {want!r}"


def test_expr_cases_match_python():
    env = Environment()
    for src, vars_, _ in EXPR_CASES:
        got = rust_eval.evaluate(env, src, variables=vars_)
        expected = env.from_string(src).render(**vars_)
        assert got == expected, f"{src!r}: {got!r} != {expected!r}"


class Obj:
    """A plain object so attribute access is exercised."""

    bar = 7

    def __init__(self):
        self.baz = "q"

    def __eq__(self, other):
        return isinstance(other, Obj) and self.bar == other.bar

    def __hash__(self):
        return hash(self.bar)

    def __call__(self, x=1):
        return f"called:{x}"


VARS = {
    "a": {"b": 2, "c": "s"},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "ns": {"attr": "v"},
    "loop": {"first": True},
    "foo": Obj(),
    "bar": lambda z=3: z * 2,
}


def _py(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:  # noqa: B902 -- differential by design
        return ("exc", type(exc).__name__, str(exc))


def _rust(env, src, vars_):
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:  # noqa: B902
        return ("exc", type(exc).__name__, str(exc))


def test_structured_expr(n=6000, seed=121):
    """Cross-validate expression evaluation on generated templates: every
    template must agree with the python engine, including exception type and
    message; only constructs outside the subset may fall back.
    """
    import random as _random

    from gen_templates import Gen

    rng = _random.Random(seed)
    g = Gen(rng)
    ok = exc = fb = 0
    for _ in range(n):
        src = "{{ " + g.expr(0) + " }}"
        env = Environment()
        p = _py(env, src, VARS)
        r = _rust(env, src, VARS)
        if r[0] == "fallback":
            fb += 1
            continue
        assert r == p, f"mismatch {src!r}: {r} != {p}"
        if p[0] == "ok":
            ok += 1
        else:
            exc += 1
    assert ok > 500, f"only {ok} successful renders"
    assert exc > 200, f"only {exc} runtime errors compared"


# ---------------------------------------------------------------------------
# slice C-v2: statements, scoping, loop contexts

from jutsu.exceptions import TemplateAssertionError  # noqa: E402
from jutsu.runtime import Namespace  # noqa: E402


class VObj:
    bar = 7

    def __init__(self):
        self.baz = "q"

    def __eq__(self, other):
        return isinstance(other, VObj) and self.bar == other.bar

    def __hash__(self):
        return hash(self.bar)

    def __call__(self, x=1):
        return f"called:{x}"


V2_VARS = {
    "a": {"b": 2, "c": "s", "attr": 1},
    "b": [1, 2, 3],
    "x": "hello",
    "y": 42,
    "ns": Namespace(),
    "loop": {"first": True},
    "foo": VObj(),
    "bar": lambda z=3: z * 2,
    "c": {1: "one", 2: "two"},
    "d": {"k": "v"},
}

OB, CB, VO, VC = "{%", "%}", "{{", "}}"

V2_CASES = [
    ("{% set x = 1 %}{{x}}", {}, "1"),
    ("{% for i in [1,2] %}{{i}}{% endfor %}", {}, "12"),
    ("{% if 1 %}yes{% else %}no{% endif %}", {}, "yes"),
    ("{% if false %}a{% elif true %}b{% else %}c{% endif %}", {}, "b"),
    ("{% if false %}a{% elif false %}b{% else %}c{% endif %}", {}, "c"),
    ("{% for a in [] %}A{% else %}E{% endfor %}", {}, "E"),
    ("{% for a in [1] %}A{% else %}E{% endfor %}", {}, "A"),
    ("{% with a=1 %}{{a}}{% endwith %}|{{ a }}", {}, "1|"),
    ("{% set y = 2 %}{% with y=y+1, z=3 %}{{y}}{{z}}{% endwith %}:{{y}}", {}, "33:2"),
    ("{% set x | upper %}hi{% endset %}[{{x}}]", {}, "[HI]"),
    ("{% set ns = namespace(v=0) %}{% for i in [1,2] %}"
     "{% set ns.v = ns.v + i %}{% endfor %}{{ ns.v }}", {}, "3"),
    ("{% for a, b in [(1,2)] %}{{a}}{{b}}{% endfor %}", {}, "12"),
    ("{% set (a,(b,c)) = (1,(2,3)) %}{{a}}{{b}}{{c}}", {}, "123"),
]

SCOPE_CASES = [
    # alias-copy at loop entry; writeback only for root-level assignments
    ("{% set x = 'X' %}{% for i in [1,2] %}{{ x}}{% set x = x ~ 'a' %}{{ x}}"
     "{% endfor %}|{{ x }}|", {}, "XXaXXa|X|"),
    ("{% for i in [1,2] %}{{ y}}{% endfor %}{% set y = 7 %}"
     "{% for i in [1,2] %}{{ y}}{% endfor %}", {}, "77"),
    ("{% if 1 %}{% set z = 3 %}{{ z}}{% endif %}|{{ z }}|", {}, "3|3|"),
    ("{% for i in [1,2] %}{{ loop.index}}{% if i==1 %}{% set q = 9 %}{{ q}}"
     "{% endif%}{% endfor %}({{ q }})", {}, "192()"),
    # loop-local stores never reach the context: undefined after the loop
    ("{% for i in [1,2] %}{% set w = i*2 %}{% endfor %}({{w}})", {}, "()"),
]

LOOP_FEATURE_CASES = [
    ("{% for i in xs %}[{{ loop.index }}/{{ loop.revindex }}/{{ loop.length }}"
     "/{{ loop.revindex0 }}]{{ loop.first }}~{{ loop.last }}"
     "{{ loop.cycle('a','b') }}{{ loop.previtem if not loop.first else ''}}"
     "{{ '!' }}{% endfor %}", {"xs": [9, 8, 7]}, None),
    ("{% for i in xs %}{% if loop.last %}|L{% endif %}{% endfor %}", {"xs": [1, 2]}, "|L"),
    ("{% for c in 'abc' %}{{ loop.changed(c % 2) }}{% endfor %}", {}, None),
]

RECURSIVE_CASES = [
    (
        "{% for k, v in tree recursive %}<{{ loop.depth }}:{{k}}>"
        "{% if v %}{{ loop(v) }}{% endif %}{% endfor %}",
        {"tree": [(1, [(2, []), (3, [])]), (5, [])]},
        "<1:1><2:2><2:3><1:5>",
    ),
]


def _py_pair(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:  # noqa: B902 -- differential by design
        return ("exc", type(exc).__name__, str(exc))


def _rust_pair(env, src, vars_):
    try:
        rv = rust_eval.evaluate(env, src, variables=vars_)
        if rv is None:
            return ("fallback",)
        return ("ok", rv)
    except Exception as exc:  # noqa: B902
        return ("exc", type(exc).__name__, str(exc))


def test_v2_cases():
    for src, vars_, want in V2_CASES:
        env = Environment()
        got = rust_eval.evaluate(env, src, variables=vars_)
        assert got is not None, f"v2 case unsupported: {src!r}"
        assert got == want, f"{src!r}: {got!r} != {want!r}"


def test_v2_scope_cases():
    for src, vars_, want in SCOPE_CASES:
        env = Environment()
        got = rust_eval.evaluate(env, src, variables=vars_)
        assert got is not None, f"v2 scope case unsupported: {src!r}"
        assert got == want, f"{src!r}: {got!r} != {want!r}"


def test_v2_matches_python():
    all_cases = V2_CASES + SCOPE_CASES + LOOP_FEATURE_CASES + RECURSIVE_CASES
    for src, vars_, _ in all_cases:
        env = Environment()
        p = _py_pair(env, src, vars_)
        r = _rust_pair(env, src, vars_)
        if r[0] == "fallback":
            continue
        assert r == p, f"{src!r}: {r} != {p}"


def test_loop_target_assertion_parity():
    env = Environment()
    cases = [
        "{% for loop in [1] %}x{% endfor %}",
        "{% for a in [1] %}{% set loop = 1 %}{% endfor %}",
        "{% if true %}{% for loop in [1] %}x{% endfor %}{% endif %}",
    ]
    for src in cases:
        try:
            env.from_string(src).render()
        except TemplateAssertionError as exc:
            want = str(exc)
        else:
            raise AssertionError("python did not fail")
        try:
            rust_eval.evaluate(env, src)
        except TemplateAssertionError as exc2:
            assert str(exc2) == want
        else:  # pragma: no cover
            raise AssertionError(f"rust did not fail for {src!r}")


def test_v2_unpack_error_parity():
    env = Environment()
    for src in [
        "{% set (a,(b,c)) = (1,2,3)%}",
        "{% for a,b in 5 %}x{% endfor %}",
        "{% for a,b in [1] %}x{% endfor %}",
    ]:
        p = _py_pair(env, src, {})
        r = _rust_pair(env, src, {})
        assert r == p, f"{src!r}: {r} != {p}"


def test_recursive_expected():
    env = Environment()
    for src, vars_, want in RECURSIVE_CASES:
        got = rust_eval.evaluate(env, src, variables=vars_)
        assert got is not None, src
        assert got == want, f"{src!r}: {got!r} != {want!r}"


def test_loop_target_store_in_macro_ok():
    env = Environment()
    # a macro parameter named `loop` is fine (only for-targets are checked)
    src = "{% macro m(loop) %}x{% endmacro %}y"
    assert rust_eval.evaluate(env, src) is None
    assert env.from_string(src).render() == "y"


def test_structured_statements(n=4000, seed=201):
    import random as _random

    from gen_templates import Gen

    rng = _random.Random(seed)
    g = Gen(rng)
    ok = exc = fb = 0
    for _ in range(n):
        src = g.template()
        env = Environment()
        p = _py_pair(env, src, V2_VARS)
        r = _rust_pair(env, src, V2_VARS)
        if r[0] == "fallback":
            fb += 1
            continue
        assert r == p, f"mismatch {src!r}: {r} != {p}"
        if p[0] == "ok":
            ok += 1
        else:
            exc += 1
    assert ok > 300, f"only {ok} successful renders"
    assert exc > 100, f"only {exc} runtime errors compared"
    print(f"structured statements: ok={ok} exc={exc} fb={fb}")


def test_autoescape_falls_back():
    """Autoescape needs the dedicated markup slice (compile-time constant
    folding is not markup aware in the python code generator), so every
    autoescape environment must fall back to the python engine for now.
    """
    env = Environment(autoescape=True)
    for src in ["{{ '<b>' }}", "{{ 'a' ~ '<b>' }}", "static {{ x }} dyn"]:
        assert rust_eval.evaluate(env, src, variables={"x": "<y>"}) is None, src
