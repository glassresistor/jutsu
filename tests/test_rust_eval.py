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
    "{% block b %}B{% endblock %}",
    "{% extends 'x' %}",
    "{% include 'x' %}",
]


def test_macro_call_block_supported():
    """Macros and call blocks landed in the evaluator with slice C-v3b."""
    env = Environment()
    src = "{% macro m(a) %}[{{ a }}]{% endmacro %}{{ m(1) }}"
    assert rust_eval.evaluate(env, src) == "[1]"

    # calling an undefined target inside a call block raises the same error
    env2 = Environment()
    exc_src = "{% call m() %}x{% endcall %}"
    try:
        py_exc = ("exc", None)
        env2.from_string(exc_src).render()
    except Exception as e:  # noqa: BLE001
        py_exc = ("exc", type(e).__name__, str(e))
    try:
        rust_eval.evaluate(env2, exc_src)
        raise AssertionError("expected an exception")
    except Exception as e2:  # noqa: BLE001
        assert ("exc", type(e2).__name__, str(e2)) == py_exc


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
    assert rust_eval.evaluate(env, src) == env.from_string(src).render() == "y"


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


# ---------------------------------------------------------------------------
# slice C-v3 part 1: filter blocks

V3_CASES = [
    ("{% filter upper %}hi {{'x'}}{% endfilter %}", {}, "HI X"),
    ("{% filter join('') %}a{{ 'b' }}c{% endfilter %}", {}, "abc"),
    ("{% filter upper|replace('I','Y') %}hi{% endfilter %}", {}, "HY"),
    ("{% filter upper|lower %}hI{% endfilter %}", {}, "hi"),
    ("{% for i in [1,2] %}{% filter join('') %}a{{i}}{% endfilter %}"
     " {% endfor %}", {}, None),
    ("{% set q = 1 %}{% filter join('') %}{% set q = 'z' %}{{q}}"
     "{% endfilter %}|{{ q }}|", {}, None),
]


def test_v3_cases():
    env = Environment()
    for src, vars_, want in V3_CASES:
        p = _py_pair(env, src, vars_)
        r = _rust_pair(env, src, vars_)
        assert r[0] != "fallback", f"v3 case unsupported: {src!r}"
        assert r == p, f"{src!r}: {r} != {p}"
        if want is not None:
            assert r[1] == want, f"{src!r}: {r[1]!r} != {want!r}"


def test_test_filter_frame_resolves_from_context():
    """The ``if=`` test of a for loop resolves names from the context, not
    from stores made by the body: the generated code runs it in its own
    filter function (separate frame)."""
    env = Environment()
    src = ("{% for a in [1,2] if t %}{% set t = 'x' %}[{{t}}]{% endfor %}"
           "|{{ t }}|")
    vars_ = {"t": True}
    p = _py_pair(env, src, vars_)
    r = _rust_pair(env, src, vars_)
    assert r[0] != "fallback", f"unsupported: {src!r}"
    assert r == p, f"{r} != {p}"


def test_unpack_of_undefined_matches():
    """Regression (fuzz seed 6): unpacking an undefined value must raise the
    same python ValueError inside a test-filtered loop."""
    env = Environment()
    src = ("{{ y.loop }}{% for a in {86 : x.y} if c%}{% set ((a, b), c) = "
           "x.b-%}-{% endfor %}%\n\n")
    vars_ = {"a": {"b": 2}, "x": "hello", "y": 42, "c": {1: "one"}}
    p = _py_pair(env, src, vars_)
    r = _rust_pair(env, src, vars_)
    assert r[0] != "fallback", f"unsupported: {src!r}"
    assert r == p, f"{r} != {p}"


def test_autoescape_falls_back():
    """Autoescape needs the dedicated markup slice (compile-time constant
    folding is not markup aware in the python code generator), so every
    autoescape environment must fall back to the python engine for now.
    """
    env = Environment(autoescape=True)
    for src in ["{{ '<b>' }}", "{{ 'a' ~ '<b>' }}", "static {{ x }} dyn"]:
        assert rust_eval.evaluate(env, src, variables={"x": "<y>"}) is None, src


def test_async_falls_back():
    """An async-enabled environment awaits coroutines and consumes async
    iterables even in the synchronous `render` path; the interpreter does not
    model that, so every async environment must fall back -- including plain
    templates where a synchronous filter happens to work either way.
    """
    async def agen():
        for i in [1, 2]:
            yield i

    async def af(v):
        return v.upper()

    env = Environment(enable_async=True)
    env.filters["af"] = af
    cases = [
        ("{{ 'a'|upper }}", {}),
        ("{{ 'a'|af }}", {}),
        ("{% for i in g %}{{ i }}{% endfor %}", {"g": agen()}),
        ("{% macro m() %}{{ 'z'|af }}{% endmacro %}{{ m() }}", {}),
    ]
    for src, vars_ in cases:
        assert rust_eval.evaluate(env, src, variables=vars_) is None, src


class Obj:
    def __init__(self):
        self.bar = 7

    def __call__(self, x=1):
        return f"called:{x}"


def test_branch_store_resolves_from_context():
    """Regression (known gap from fuzz seed 2): names stored only inside an
    if branch must become RESOLVE loads at the merge point -- the code
    generator's ``Symbols.copy``/``branch_update`` -- never undefined slots:
    a store that may not have executed cannot shadow the context value.
    """
    from jutsu.runtime import Namespace
    env = Environment()
    vars_ = {
        "x": "hello",
        "b": [1, 2, 3],
        "ns": Namespace(),
        "foo": Obj(),
        "xs": [1, -2],
    }
    cases = [
        # store in a branch that never runs -> load sees the context value
        "{% if x %}{% set q = 1 %}{% else %}{% set b = 9 %}{% endif %}|{{ b }}",
        # store in the branch that runs -> load sees the stored value
        "{%- if x %} {% set b = 9 %} {%- else %}{% set b = 8 %}{% endif %}|{{ b }}",
        # load in one branch, store in another
        "{% if x %}{{ b }}{% else %}{% set b = 9 %}{% endif %}",
        # elif chain: stores evolve on the chain copy
        "{% if false %}{% set b = 1 %}{% elseif x %}{% set b = 8 %}"
        "{% else %}{% set z = 7 %}{% endif %}|{{ b }}|{{ z }}".replace("elseif", "elif"),
        # nested if inside a branch, load after the inner merge
        "{% if x %}{% if y %}{% set b = 1 %}{% endif %}{{ b }}"
        "{% else %}{% endif %}",
        # set-block store inside a dead branch
        "{% if x %}{% else %}{% set b %}q{% endset %}{% endif %}|{{ b }}",
        # two consecutive ifs storing the same name (merge must not demote)
        "{% if x %}{% set b = 1 %}{% endif %}{% if false %}{% set b = 2 %}"
        "{% endif %}|{{ b }}",
        # branch store inside a loop body scope
        "{% for i in xs %}{% if i > 0 %}{% set b = i %}{% endif %}{{ b }}{% endfor %}",
    ]
    for src in cases:
        p = _py_pair(env, src, vars_)
        r = _rust_pair(env, src, vars_)
        assert r[0] != "fallback", f"unsupported: {src!r}"
        assert r == p, f"{src!r}: {r} != {p}"


def test_branch_merge_full_fuzz_case():
    """The exact fuzz seed-2 template that exposed the branch_update gap:
    a set-block target inside a dead else branch plus a later root store of
    the same names; python raises TypeError on the namespace subscript, the
    rust evaluator used to report an undefined ``b`` instead.
    """
    from jutsu.runtime import Namespace
    env = Environment()
    vars_ = {
        "a": {"b": 2, "c": "s", "attr": 1},
        "b": [1, 2, 3],
        "x": "hello",
        "ns": Namespace(),
        "foo": Obj(),
    }
    src = (
        "{% if x is defined%}{% else %}{% set (a, b) %}{% endset %}"
        "{% for a in x.y%}{% endfor %}{% endif %}| |"
        "{% set ((a, b), c) = ns[[true, True, None]:{0.2349 : -1.2891, "
        "3 : true}:b.foo].bar.bar >= foo(x=1) != \"quote\" 'its'%}"
    )
    p = _py_pair(env, src, vars_)
    r = _rust_pair(env, src, vars_)
    assert r[0] != "fallback", f"unsupported: {src!r}"
    assert r == p, f"{src!r}: {r} != {p}"


# ---------------------------------------------------------------------------
# slice C-v3b: macros, call blocks and caller closures


class MacroObj:
    def __init__(self):
        self.bar = 7

    def __call__(self, x=1):
        return f"called:{x}"


def _kw(*args, **kw):
    return "pos:" + repr(list(args)) + "|kw:" + repr(sorted(kw.items()))


def _invoke_caller(*args, **kw):
    c = kw.get("caller")
    if c is not None:
        return "invoked:" + str(c())
    return "plain"


def macro_vars():
    return {"x": 0, "xs": [1, 2], "o": MacroObj(), "f": _kw, "tf": _invoke_caller}


# macros / call blocks: closures over the defining frame chain, recursion,
# the caller/kwargs/varargs specials and every error path along the way.
MACRO_SRC = [
    # closure over root locals that are assigned after the definition
    "{% macro m() %}<{{ x }}>{% endmacro %}{% set x = 1 %}{{ m() }}",
    "{% macro m() %}<{{ x }}>{% endmacro %}{{ x }}{% set x = 2 %}{{ m() }}{{ m() }}",
    # recursion, mutual recursion and nesting
    "{% macro r(i) %}{% if i > 0 %}{{ i }}{{ r(i - 1) }}{% endif %}{% endmacro %}"
    "{{ r(3) }}",
    "{% macro even(i) %}{% if i == 0 %}yes{% elif i < 0 %}no{% else %}"
    "{{ odd(i - 1) }}{% endif %}{% endmacro %}{% macro odd(i) %}"
    "{{ even(i - 1) }}{% endmacro %}{{ even(4) }}",
    "{% macro outer(x) %}{% macro inner(y) %}({{ x }}{{ y }}){% endmacro %}"
    "{{ inner(1) }}{% endmacro %}{{ outer(5) }}",
    # defining scope gone by the time the captured macro runs
    "{% macro outer() %}{% macro inner() %}[{{ x }}]{% endmacro %}"
    "{% set g = inner %}{% endmacro %}{{ outer() }}|({{ g() }})",
    # macros inside for / with / filter scopes
    "{% for i in xs %}{% macro m() %}i={{ i }} {% endmacro %}{{ m() }}"
    "{% endfor %}",
    "{% for i in xs %}{% macro m2() %}<{{ loop.index }}>{% endmacro %}"
    "{{ m2() }}{% endfor %}",
    "{% macro m() %}<{{ loop.index }}>{% endmacro %}{% for i in xs %}"
    "{{ m() }}{% endfor %}",
    "{% with q = 1 %}{% macro m() %}[{{ q }}{{ x }}]{% endmacro %}{{ m() }}"
    "{% endwith %}",
    "{% filter upper %}{% macro m() %}hi{% endmacro %}a{{ m() }}{% endfilter %}",
    # caller / kwargs / varargs specials
    "{% macro m() %}A{{ caller() }}B{% endmacro %}{% call m() %}CB{% endcall %}",
    "{% macro m(x, caller=None) %}[{{ caller() }}]{% endmacro %}"
    "{% call m() %}CB{% endcall %}",
    "{% macro m(a) %}|{{ varargs }}|{{ kwargs.items()|list }}|{% endmacro %}"
    "{{ m(1, 2, 3, k=4) }}",
    "{% macro m(varargs) %}<{{ varargs }}>{% endmacro %}{{ m('v') }}",
    # defaults evaluated inside the macro frame / missing parameter undefined
    "{% set z = 9 %}{% macro m(a=z) %}[{{ a }}]{% endmacro %}{{ m() }}",
    "{% macro m(a=z) %}[{{ a }}]{% endmacro %}{% set z = 3 %}{{ m() }}",
    "{% macro m(a) %}({{ a }}){% endmacro %}{{ m() }}",
    # the Macro object itself
    "{% macro m(a, b=2) %}x{% endmacro %}{{ m.name }}|{{ m.arguments }}|{{ m }}",
    "{% macro _priv() %}x{% endmacro %}{% set p = _priv() %}{{ p }}",
    # call blocks with their own signature
    "{% call (a) f(a) %}body:{{ a }}{% endcall %}",
    "{% call (a, b=3) f(a, 1) %}{{ a }}{{ b }}{% endcall %}",
    "{% call (a, b) f(a, 1) %}{{ caller(5, b=6) }}{% endcall %}",
    "{% call tf() %}cb-body{% endcall %}",
    "{% call f(k=1) %}body{% endcall %}",
    # two values for the special caller argument (python's own guard)
    "{% macro m() %}m{% endmacro %}{% call m() %}{% call m() %}x{% endcall %}"
    "{% endcall %}",
    # name resolution / shadowing
    "{% macro m(a) %}{{ a }}{% endmacro %}{% set m = 5 %}{{ m(1) }}",
    "{% set n = 5 %}{% call n() %}x{% endcall %}",
    # store into a root cell from inside a macro
    "{% macro m(v) %}{% set x = v %}{{ x }}{% endmacro %}{{ m(7) }}|{{ x }}",
    # argument errors of the python-side Macro wrapper
    "{% macro m(a) %}x{% endmacro %}{{ m(1, 2) }}",
    "{% macro m(a) %}x{% endmacro %}{{ m(k=9) }}",
    # dict-literal workaround path: duplicates are legal and last wins
    "{{ f(class=1, a=1, a=2) }}",
    "{% call f(caller='c', class=1) %}cb{% endcall %}",
]


def test_macro_cases():
    for src in MACRO_SRC:
        vars_ = macro_vars()
        env = Environment()
        p = _py(env, src, vars_)
        env2 = Environment()
        r = _rust(env2, src, vars_)
        assert r[0] != "fallback", f"unsupported: {src!r}"
        assert r == p, f"{src!r}: {r} != {p}"


# python's code generator emits these into generated source, where CPython
# fails with a SyntaxError carrying generated line numbers; the interpreter
# cannot reproduce that message, so it must report unsupported instead.
CODEGEN_SYNTAX = [
    "{{ f(a=1, a=2) }}",                                   # duplicate keyword
    "{% if false %}{{ f(a=1, a=2) }}{% endif %}",          # ... even when dead
    "{{ x | upper(b=1, b=2) }}",                           # filters too
    "{% if x is undefined(a=1, a=2) %}y{% endif %}",       # and tests
    "{% macro m() %}{{ f(a=1, a=2) }}{% endmacro %}",
    "{% call f(b=1, b=2) %}x{% endcall %}",
    "{% call f(caller='c') %}cb{% endcall %}",             # forwarded caller
    "{% macro m(a, a) %}x{% endmacro %}",                  # duplicate parameter
]


def test_codegen_syntax_falls_back():
    for src in CODEGEN_SYNTAX:
        vars_ = macro_vars()
        env = Environment()
        p = _py(env, src, vars_)
        assert p[0] == "exc" and p[1] == "SyntaxError", f"{src!r}: {p}"
        env2 = Environment()
        assert rust_eval.evaluate(env2, src, variables=vars_) is None, src


def test_macro_expected_semantics():
    """Lock the closure/caller semantics that make macros tricky."""
    cases = [
        # a macro sees the current value of root locals, not the define-time one
        ("{% macro m() %}<{{ x }}>{% endmacro %}{% set x = 1 %}{{ m() }}<{{ x }}>",
         "<1><1>"),
        # implicit caller keeps the call block body's output
        ("{% macro m() %}[{{ caller() }}]{% endmacro %}"
         "{% call m() %}cb{% endcall %}", "[cb]"),
        # implicit varargs/kwargs shapes
        ("{% macro m() %}{{ varargs }}|{{ kwargs.pop('k') }}|{{ kwargs }}{% endmacro %}"
         "{{ m(1, k=2) }}", "(1,)|2|{}"),
        # recursion through the macro name
        ("{% macro r(i) %}{% if i %}{{ i }}{{ r(i - 1) }}{% endif %}{% endmacro %}"
         "{{ r(4) }}", "4321"),
    ]
    env = Environment()
    for src, want in cases:
        got = rust_eval.evaluate(Environment(), src, variables=macro_vars())
        assert got is not None, src
        assert got == want, f"{src!r}: {got!r} != {want!r}"
        assert env.from_string(src).render(**macro_vars()) == want
