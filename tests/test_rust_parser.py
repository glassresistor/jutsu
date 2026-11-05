"""Differential tests: the rust parser core must produce byte-identical ASTs
(and error messages) compared to the original python parser.

Compares `Node.dump()` output of both trees, plus exception args for failures.
"""
import os
import random

import pytest

from jutsu import nodes
from jutsu import rust_parse
from jutsu.environment import Environment
from jutsu.exceptions import TemplateSyntaxError
from jutsu.parser import Parser

try:
    from jutsu import _jutsu as _  # noqa: F401
    if not rust_parse.has_rust_core():
        raise ImportError
except ImportError:  # pragma: no cover
    pytest.skip("rust core not built", allow_module_level=True)


def py_parse(env, source):
    try:
        return ("ok", Parser(env, source).parse().dump())
    except TemplateSyntaxError as exc:
        return ("err", tuple(exc.args))


def rs_parse(env, source):
    try:
        return ("ok", rust_parse.parse_template(env, source).dump())
    except TemplateSyntaxError as exc:
        return ("err", tuple(exc.args))


def cmp_tree(a, b, path="root"):
    """Lockstep comparison of two node trees including line numbers and
    attributes (``dump()`` omits both, so this is the extra validation layer)."""
    if type(a) is not type(b):
        raise AssertionError(
            f"type mismatch at {path}: {type(a).__name__} vs {type(b).__name__}"
        )
    if getattr(a, "lineno", None) != getattr(b, "lineno", None):
        raise AssertionError(
            f"lineno mismatch at {path}: {type(a).__name__} "
            f"{getattr(a, 'lineno', None)!r} vs {getattr(b, 'lineno', None)!r}"
        )
    for name in a.fields:
        va = getattr(a, name)
        vb = getattr(b, name)
        if isinstance(va, nodes.Node):
            cmp_tree(va, vb, f"{path}.{name}")
        elif isinstance(va, (list, tuple)) and any(
            isinstance(x, nodes.Node) for x in va
        ):
            if len(va) != len(vb):
                raise AssertionError(f"length mismatch at {path}.{name}")
            for i, (x, y) in enumerate(zip(va, vb)):
                cmp_tree(x, y, f"{path}.{name}[{i}]")
        elif va != vb:
            raise AssertionError(
                f"value mismatch at {path}.{name}: {va!r} vs {vb!r}"
            )


def check(source, env=None):
    env = env or Environment()
    a = py_parse(env, source)
    b = rs_parse(env, source)
    if a != b:
        raise AssertionError(f"parser mismatch for {source!r}\n  python={a!r}\n   rust={b!r}")
    if a[0] == "ok":
        # trees agree on dumps; now compare them field by field, linenos included
        tree_a = Parser(env, source).parse()
        tree_b = rust_parse.parse_template(env, source)
        cmp_tree(tree_a, tree_b)


CASES = [
    "",
    "foo",
    "{{ foo }}",
    "{{ foo.bar }}",
    "{{ foo[0] }}",
    "{{ foo[1:2] }}",
    "{{ foo[::2] }}",
    "{{ a.b.c().d[1] if x else y }}",
    "{{ ('a' ~ 'b')|upper }}",
    "{% for i in [] %}{% else %}{% endfor %}",
    "{% if 1 %}{% elif 2 %}{% elif 3 %}{% endif %}",
    "{% set x = 1 %}",
    "{% set x %}{% endset %}",
    "{% block name scoped required %} {% endblock %}",
    "{% extends 'base.html' %}",
    "{% include ['a', 'b'] ignore missing without context %}",
    "{% import 'x.html' as y with context %}",
    "{% from 'x.html' import a, b as c without context %}",
    "{% macro m(a=1, *b) %}{% endmacro %}",
    "{% call(m=1) t() %}{% endcall %}",
    "{% filter upper | lower %}x{% endfilter %}",
    "{% with a=1, b=2 %}{% endwith %}",
    "{% autoescape false %}{% endautoescape %}",
    "{{ x is not defined }}",
    "{{ x is in [1, 2] }}",
    "{{ x is mapping or y is string }}",
    "{{ a + b|f(1) ~ c }}",
    "{{ 99999999999999999999999999999999999999 }}",
    "{{ 0x10 }}",
    "{{ 0o7 }}",
    "{{ 0b11 }}",
    "{{ 1_000 }}",
    "{{ 1e400 }}",
    "{{ -0.0 }}",
    "{{ x|replace('a', 'b')|upper }}",
    "{{ x is divisibleby 3 }}",
    "{{ x is not divisibleby(3) }}",
    "{{ foo['a', 'b'] }}",
    "{{ x[1:2:3] }}",
    "{{ a if b else c if d else e }}",
    "{{ (('a')) }}",
    "{{ {'%': 'pct'} }}",
    "{{ ns.x }}",
    "{% set ns.x = 1 %}",
    "{% for a in b if c %}{% else %}{% endfor %}",
    "{% for a in b recursive %}{% endfor %}",
    "{% if a %}{% elif b %}{% else %}{% endif %}",
    "{% macro m(*a, **k) %}{% endmacro %}",
    "{% call m() with a %}{% endcall %}",
    "{{ x | f(a, b if c else d) }}",
    "{{ (1 + 2) * 3 ** 4 // 5 % 6 }}",
    "{{ 'a' if true else 'b' if false else 'c' }}",
    "{% raw %}{{ nope }}{% endraw %}",
    "{#comment#}",
    "{{ {'a': 1, 'b': 2}[key] }}",
    "{{ [1, 2, 3][0] }}",
    "{{ (1,) }}",
    "{{ () }}",
    "{{ a[0][1].x }}",
    "{% for x in y %}{{ loop.index }}{% endfor %}",
    "text {{ var }} more",
    "{{ 'a' 'b' }}",   # implicit string concat of adjacent literals
    "{{ 1 if true }}",
    "{{ not x }}",
    "{{ -x ** 2 }}",
    "{{ (not a) and b }}",
    "{% for a in b %}{{ loop.cycle('odd','even') }}{% endfor %}",
]

ERR_CASES = [
    "{% foo %}",
    "{% foo %}{% endfoo %}",
    "{{ x is is 1 }}",
    "{{ .x }}",
    "{{ {'a': } }}",
    "{{ [1, }}",
    "{{ (1 }}",
    "{{ {'a' }",
    "{{ 1 if }}",
    "{{ 1 if 2 else }}",
    "{{ x is not }}",
    "{{ x. }}",
    "{{ x.[0] }}",
    "{{ x| }}",
    "{{ x|f( }}",
    "{{ a b }}",
    "{{ 'a' ' }}",
    "{{ '\\xNN' }}",
    "{% if 1 %}{% elif 2 %}",
    "{% block %}x",
    "{% macro m() %}",
    "{% for a in b %}",
    "{% set x = %}",
    "{% set %}1{% endset %}",
    "{% extends 'a' %}{% block b %}{% endblock %}{% block c %}{% endblock %}",
    "{% filter %}x{% endfilter %}",
    "{% call m( %}",
    "{% import 'a' as %}",
    "{% from 'a' import b as %}",
    "{% include 'a' ignore %}",
    "{% with a= %}",
    "{% autoescape true or %}",
    "{% if %}",
    "{% endif %}",
    "{{ }}",
    "{{ 1 + }}",
    "{% for %}{% endfor %}",
    "{% set %}",
    "{% block %}",
    "{% macro %}",
    "{% call %}",
    "{% filter %}",
    "{{ x is %}",
    "{% include %}",
    "{% extends %}",
    "{% import %}",
    "{% from %}",
    "{% with %}",
    "{% autoescape %}",
    "foo {{ bar",
    "{# unterminated",
]


def test_fixed_cases():
    bad = 0
    for src in CASES + ERR_CASES:
        try:
            check(src)
        except AssertionError:
            bad += 1
            print("FAIL", repr(src))
    assert bad == 0, f"{bad} failures"


def test_custom_delimiters():
    env = Environment(
        block_start_string="<%",
        block_end_string="%>",
        variable_start_string="${",
        variable_end_string="}",
        comment_start_string="<%#",
        comment_end_string="%>",
    )
    bad = 0
    for src in CASES:
        try:
            check(src, env)
        except AssertionError:
            bad += 1
            print("FAIL", repr(src))
    assert bad == 0


FRAGMENTS = [
    "{%", "%}", "{{", "}}", "{#", "#}", "raw", "endraw", "for", "in", "endfor",
    "if", "elif", "else", "endif", "block", "endblock", "scoped", "required",
    "set", "endset", "with", "endwith", "macro", "endmacro", "call", "endcall",
    "filter", "endfilter", "extends", "include", "import", "from", "as", "without",
    "context", "ignore", "missing", "recursive", "not", "and", "or", "is", "true",
    "false", "none", "print", "autoescape", "endautoescape",
    "a", "b", "x", "y", "1", "2.5", "'s'", '"s"', ",", ".", ":", "|", "=", "+", "-",
    "*", "**", "/", "//", "%", "~", "[", "]", "(", ")", "{", "}", "<", ">", "==",
    "!=", ">=", "<=", "\n", "\t", "\\", "'", "#", " ", "\r\n",
]


def test_fuzz(n=6000, seed=1234):
    rng = random.Random(seed)
    mismatches = 0
    for _ in range(n):
        k = rng.randint(1, 14)
        src = "".join(rng.choice(FRAGMENTS) for _ in range(k))
        try:
            check(src)
        except AssertionError as exc:
            mismatches += 1
            print(f"\nFUZZ MISMATCH {src!r}\n {exc}")
            if mismatches > 8:
                pytest.fail("too many fuzz mismatches")
    assert mismatches == 0


def test_structured_fuzz(n=4000, seed=17):
    """Structured generator (tests/gen_templates.py): grammatically plausible
    templates hitting deep grammar corners; compared with the full tree and
    lineno walk."""
    import random as _random

    from gen_templates import Gen

    rng = _random.Random(seed)
    g = Gen(rng)
    mismatches = 0
    for _ in range(n):
        src = g.template()
        try:
            check(src)
        except AssertionError as exc:
            mismatches += 1
            print(f"\nSTRUCT MISMATCH {src!r}\n {exc}")
            if mismatches > 8:
                pytest.fail("too many structured fuzz mismatches")
    assert mismatches == 0


def test_loader_fixture_files():
    d = os.path.join(os.path.dirname(__file__), "res", "templates")
    env = Environment()
    bad = 0
    for name in sorted(os.listdir(d)):
        p = os.path.join(d, name)
        if not os.path.isfile(p):
            continue
        with open(p, encoding="utf-8", errors="replace") as fh:
            src = fh.read()
        try:
            check(src, env)
        except AssertionError:
            bad += 1
            print("FAIL file", name)
    assert bad == 0
