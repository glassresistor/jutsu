"""Differential tests: the rust parser core must produce byte-identical ASTs
(and error messages) compared to the original python parser.

Compares `Node.dump()` output of both trees, plus exception args for failures.
"""
import os
import random

import pytest

from jutsu import _jutsu
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


def check(source, env=None):
    env = env or Environment()
    a = py_parse(env, source)
    b = rs_parse(env, source)
    if a != b:
        raise AssertionError(f"parser mismatch for {source!r}\n  python={a!r}\n   rust={b!r}")


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
        a = py_parse(Environment(), src)
        b = rs_parse(Environment(), src)
        if a != b:
            mismatches += 1
            print(f"\nFUZZ MISMATCH {src!r}\n python={a!r}\n    rust={b!r}")
            if mismatches > 8:
                pytest.fail("too many fuzz mismatches")
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
