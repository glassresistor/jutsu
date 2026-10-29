"""Differential tests: the rust core lexer must produce exactly the same token
streams (and errors) as the original pure-python jinja lexer.

The python implementation is kept available as ``Lexer._tokeniter_python``
specifically so that these comparisons can run.
"""
import random

import pytest

from jutsu.environment import Environment
from jutsu.exceptions import TemplateSyntaxError
from jutsu.lexer import Lexer

try:
    from jutsu import _jutsu  # noqa: F401
except ImportError:  # pragma: no cover - rust core not built
    pytest.skip("rust core not built", allow_module_level=True)

CONFIGS = [
    {},
    {"trim_blocks": True},
    {"lstrip_blocks": True},
    {"trim_blocks": True, "lstrip_blocks": True},
    {
        "block_start_string": "<%",
        "block_end_string": "%>",
        "variable_start_string": "${",
        "variable_end_string": "}",
        "comment_start_string": "<%#",
        "comment_end_string": "%>",
    },
    {
        "line_statement_prefix": "%%",
        "line_comment_prefix": "##",
    },
    {
        "block_start_string": "{[",
        "block_end_string": "]}",
        "variable_start_string": "[[",
        "variable_end_string": "]]",
        "comment_start_string": "(*",
        "comment_end_string": "*)",
    },
]

# pool of pieces used to build random template sources
PIECES = [
    "{", "}", "%", "{{", "}}", "{%", "%}", "{#", "#}", "raw", "endraw",
    " ", "\n", "\t", "\r\n", "-", "+", "a", "b", "1", "0", "12", "1_2",
    "'", '"', "\\", ".", "*", "(", ")", "[", "]", ":", "|", ",", "=",
    "<-", "<%", "%>", "${", "}", "%%", "##", "([", "])", "(*", "*)",
    "foo", " ", "\n", "\u00e9", "\u00b7", "x", "y", "0x1f", "1.5", "1e5",
]


def _run_python(lexer, source, name=None, filename=None, state=None):
    return _drain(lambda: lexer._tokeniter_python(source, name, filename, state))


def _run_rust(lexer, source, name=None, filename=None, state=None):
    return _drain(lambda: lexer.tokeniter(source, name, filename, state))


def _drain(gen_fn):
    """Collect the tokens yielded before (and including) any error, plus the
    exception args if one was raised.  Mirrors generator semantics where partial
    output is produced before an error surfaces.
    """
    items = []
    try:
        for item in gen_fn():
            items.append(tuple(item))
    except TemplateSyntaxError as exc:
        return (items, tuple(exc.args))
    return (items, None)


@pytest.mark.parametrize("cfg", CONFIGS)
def test_differential_random(cfg):
    rng = random.Random(1234 + len(cfg.items()) * 7)
    env = Environment(**cfg)
    lexer = Lexer(env)
    mismatches = 0
    n_cases = 3000
    for _ in range(n_cases):
        parts = [rng.choice(PIECES) for _ in range(rng.randint(1, 14))]
        source = "".join(parts)
        py = _run_python(lexer, source)
        rs = _run_rust(lexer, source)
        if py[0] != rs[0] or py[1] != rs[1]:
            mismatches += 1
            print(f"\nMISMATCH for {source!r} cfg={cfg}\n python={py}\n    rust={rs}")
            if mismatches > 5:
                pytest.fail("too many mismatches (see stdout)")
    assert mismatches == 0, f"{mismatches} mismatches for cfg={cfg}"


@pytest.mark.parametrize("cfg", CONFIGS)
def test_differential_state(cfg):
    """The same, but lexing directly in `variable`/`block` states."""
    rng = random.Random(99 + len(cfg.items()))
    env = Environment(**cfg)
    lexer = Lexer(env)
    mismatches = 0
    for _ in range(800):
        parts = [rng.choice(PIECES) for _ in range(rng.randint(1, 8))]
        source = "".join(parts)
        for state in ("variable", "block"):
            py = _run_python(lexer, source, None, None, state)
            rs = _run_rust(lexer, source, None, None, state)
            if py[0] != rs[0] or py[1] != rs[1]:
                mismatches += 1
                print(f"\nMISMATCH state={state} {source!r}\n python={py}\n    rust={rs}")
    assert mismatches == 0