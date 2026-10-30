"""Golden-behaviour oracle for macro / call-block semantics (slice C-v3b).

Pure python engine only -- no rust imports.  Every expectation was recorded
by running the forked python implementation; the rust evaluator must match
each of these exactly once macros land in rust.

Surprises recorded while measuring:

* macro defaults are evaluated at CALL time, inside the defining frame chain
  (a later root-level store is visible: ``m(a=x)`` sees a subsequent
  ``{% set x = 9 %}``);
* names owned by the defining scope become closure cells over live slots:
  late stores after the def are visible, but loop-local shadowing of the
  same name in a *different* frame is not;
* call blocks always forward ``caller=`` into the target's kwargs
  (``forward_caller=True``), and the body only renders when the target
  invokes the caller closure;
* an explicit ``caller`` parameter without a default fails at compile time
  with TemplateAssertionError when the body references caller;
* naming a parameter ``varargs``/``kwargs`` makes it an ordinary parameter,
  suppressing the special behaviour unless the body references them.
"""
import pytest

from jutsu.environment import Environment


def _py(env, src, vars_):
    try:
        return ("ok", env.from_string(src).render(**vars_))
    except Exception as exc:  # noqa: BLE001
        return ("exc", type(exc).__name__, str(exc))


GOLDENS = [
    ("arg-matching-positional",
     "{% macro m(a) %}[{{ a }}]{% endmacro %}{{ m(1) }}", {},
     ("ok", "[1]")),
    ("arg-matching-keyword",
     "{% macro m(a) %}[{{ a }}]{% endmacro %}{{ m(a=1) }}", {},
     ("ok", "[1]")),
    ("arg-default-filled",
     "{% macro m(a,b=2) %}[{{ a }}{{ b }}]{% endmacro %}{{ m(1) }}", {},
     ("ok", "[12]")),
    ("missing-param-renders-empty",
     "{% macro m(a) %}[{{ a }}]{% endmacro %}{{ m() }}", {},
     ("ok", "[]")),
    ("too-many-positional",
     "{% macro m(a) %}x{% endmacro %}{{ m(1, 2) }}", {},
     ("exc", "TypeError", "macro 'm' takes not more than 1 argument(s)")),
    ("unexpected-keyword",
     "{% macro m(a) %}x{% endmacro %}{{ m(1, z=2) }}", {},
     ("exc", "TypeError", "macro 'm' takes no keyword argument 'z'")),
    ("varargs-kwargs-specials",
     "{% macro m(a) %}[{{ a }}|{{ varargs }}|{{ kwargs }}]{% endmacro %}"
     "{{ m(1, 2, k=3) }}", {},
     ("ok", "[1|(2,)|{'k': 3}]")),
    ("varargs-empty-kwargs",
     "{% macro m(a) %}[{{ a }}|{{ varargs }}|{{ kwargs }}]{% endmacro %}"
     "{{ m(k=3) }}", {},
     ("ok", "[|()|{'k': 3}]")),
    ("caller-not-passed",
     "{% macro m() %}[{{ caller() }}]{% endmacro %}{{ m() }}", {},
     ("exc", "UndefinedError", "No caller defined")),
    ("defaults-at-call-time",
     "{% macro m(a=x) %}[{{ a }}]{% endmacro %}{% set x = 9 %}{{ m() }}", {},
     ("ok", "[9]")),
    ("default-of-missing-resolves-empty",
     "{% macro m(a=x) %}[{{ a }}]{% endmacro %}{{ m() }}", {},
     ("ok", "[]")),
    ("closure-cell-late-store-visible",
     "{% set x = 3 %}{% macro m() %}[{{ x }}]{% endmacro %}"
     "{% for x in [1] %}{{ m() }}{% endfor %}", {}, ("ok", "[3]")),
    ("context-resolve-not-loop-local",
     "{{ x }}{% macro m() %}[{{ x }}]{% endmacro %}"
     "{% for x in [1] %}{{ m() }}{% endfor %}", {}, ("ok", "[]")),
    ("cell-vs-context-after-loop",
     "{% set x = 3 %}{% macro m() %}[{{ x }}]{% endmacro %}"
     "{% for x in [1] %}{{ m() }}{% endfor %}{{ x }}", {}, ("ok", "[3]3")),
    ("macro-defined-in-loop-sees-target",
     "{% for y in [7] %}{% macro m() %}[{{ y }}]{% endmacro %}"
     "{{ m() }}{% endfor %}", {}, ("ok", "[7]")),
    ("forward-reference-ok",
     "{% macro a() %}[{{ b() }}]{% endmacro %}{% macro b() %}b{% endmacro %}"
     "{{ a() }}", {}, ("ok", "[b]")),
    ("macro-recursion",
     "{% macro m(n) %}{% if n > 0 %}{{ m(n-1) }}{{ n }}{% endif %}"
     "{% endmacro %}[{{ m(3) }}]", {}, ("ok", "[123]")),
    ("stored-macro-called-later",
     "{% macro m(a) %}[{{ a }}]{% endmacro %}{% set q = m %}{{ q(5) }}", {},
     ("ok", "[5]")),
    ("macro-repr",
     "{% macro m() %}x{% endmacro %}|{{ m }}|", {}, ("ok", "|<Macro 'm'>|")),
    ("caller-default-none",
     "{% macro m(caller=none) %}[{{ caller }}]{% endmacro %}{{ m() }}", {},
     ("ok", "[None]")),
    ("call-block-explicit-caller",
     "{% macro m(caller=none) %}[{{ caller }}]{% endmacro %}"
     "{% call m() %}CB{% endcall %}", {}, ("ok", "[<Macro anonymous>]")),
    ("caller-invoked-twice",
     "{% macro m(caller=none) %}[{{ caller() }}|{{ caller() }}]{% endmacro %}"
     "{% call m() %}C{% endcall %}", {}, ("ok", "[C|C]")),
    ("nested-macro-sees-outer-param",
     "{% macro outer(p) %}{% macro inner() %}i{{ p }}{% endmacro %}"
     "{{ inner() }}{% endmacro %}{{ outer(3) }}", {}, ("ok", "i3")),
    ("varargs-as-plain-param",
     "{% macro m(varargs) %}[{{ varargs }}]{% endmacro %}{{ m('v') }}", {},
     ("ok", "[v]")),
    ("kwargs-as-plain-param",
     "{% macro m(kwargs) %}[{{ kwargs }}]{% endmacro %}{{ m(k='v') }}", {},
     ("exc", "TypeError", "macro 'm' takes no keyword argument 'k'")),
    ("caller-keyword-into-catch_kwargs",
     "{% macro m(a) %}[{{ a }}|{{ kwargs }}]{% endmacro %}"
     "{{ m(1, caller=2) }}", {}, ("ok", "[1|{'caller': 2}]")),
    ("explicit-caller-no-default",
     "{% macro m(caller) %}[{{ caller() }}]{% endmacro %}{{ m() }}", {},
     ("exc", "TemplateAssertionError",
      'When defining macros or call blocks the special "caller" argument '
      "must be omitted or be given a default.")),
    ("signature-required",
     "{% macro m %}x{% endmacro %}", {},
     ("exc", "TemplateSyntaxError",
      "expected token '(', got 'end of statement block'")),
]


@pytest.mark.parametrize("tag,src,vars_,want", GOLDENS)
def test_macro_semantics_oracle(tag, src, vars_, want):
    env = Environment()
    got = _py(env, src, vars_)
    assert tuple(got) == tuple(want), f"golden drift [{tag}] {src!r}: {got}"


def test_call_block_forwards_caller_to_target():
    """Call blocks always pass ``caller=`` into the target (forward_caller);
    a target that cannot accept it raises, and the caller object is a
    runtime.Macro."""
    env = Environment()

    def foo(a, b):
        return f"{a}-{b}"

    src = "<{% call(x) foo(x, 1) %}B{% endcall %}>"
    got = _py(env, src, {"foo": foo})
    assert got[0] == "exc" and got[1] == "TypeError", got
    assert str(got[2]).endswith(
        "foo() got an unexpected keyword argument 'caller'"), got

    def bar(a, b, **kw):
        return f"{a}-{b}-{type(kw.get('caller')).__name__}"

    env2 = Environment()
    got2 = _py(env2, src, {"foo": bar})
    assert got2 == ("ok", "<-1-Macro>"), got2


def test_call_block_body_only_renders_via_caller():
    env = Environment()

    def noop(a, b, **kw):
        return f"{a}-{b}"

    src = "{% call(x) noop(x, 1) %}BODY{% endcall %}"
    assert _py(env, src, {"noop": noop}) == ("ok", "-1")

    def twice(a, b, **kw):
        return str(kw["caller"]()) * 2

    env2 = Environment()
    got = _py(env2, "{% call(x) twice(x, 1) %}C{{ x }}{% endcall %}",
              {"twice": twice})
    assert got == ("ok", "CC"), got  # caller() gets no args -> x missing
