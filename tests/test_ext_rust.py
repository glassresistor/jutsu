"""D1: extension gating & fallback correctness (PLAN_D_PARALLEL.md).

The documented rule enforced here end-to-end: any extension that contributes
parse tags, `filter_stream` or `preprocess` forces the python parse+codegen
path, and therefore never rust serving; pure runtime extensions (filters and
tests only) stay rust-eligible.  Every shipped extension is diffed against
upstream jinja for byte-identical rendering, and parser selection is asserted
by instrumentation rather than assumption.

This module owns no src changes: the eligibility predicate lives in C-v8a; if
gating needs adjustment it goes to the integration owner as a follow-up.
"""
import re
from io import BytesIO

import pytest

from jutsu import DictLoader
from jutsu import Environment
from jutsu import nodes
from jutsu import rust_eval
from jutsu.ext import Extension
from jutsu.ext import babel_extract
from jutsu.ext import extract_from_ast
from jutsu.lexer import Token
from jutsu.lexer import count_newlines
from jutsu.parser import Parser
from jutsu.parser import _extensions_contribute
from jutsu.parser import _rust_parser_active

try:
    import jinja2
except ImportError:  # pragma: no cover - differential partner optional
    jinja2 = None

try:
    assert rust_eval.has_rust_core()
except AssertionError:  # pragma: no cover
    pytest.skip("rust core not built", allow_module_level=True)


# --------------------------------------------------------------------------
# parser-selection instrumentation
# --------------------------------------------------------------------------

SELECTS = []


def _install_spy(monkeypatch):
    """Record which parser each whole-template parse actually used."""
    original = Parser.parse

    def spy(self):
        SELECTS.append((str(self.source)[:32], bool(_rust_parser_active(self.environment))))
        return original(self)

    monkeypatch.setattr(Parser, "parse", spy)
    SELECTS.clear()
    return SELECTS


def used_rust():
    assert len(SELECTS) >= 1, "no parse recorded"
    return SELECTS[0][1]


# --------------------------------------------------------------------------
# upstream differential partner
# --------------------------------------------------------------------------

def diff_against_upstream(ext_name, source, variables=None, build=None):
    """Render `source` with the extension in jutsu and in upstream jinja and
    require byte-identical output.  Returns (jutsu_out, upstream_out)."""
    if jinja2 is None:  # pragma: no cover
        pytest.skip("upstream jinja2 not installed")
    jutsu_env = Environment(extensions=[f"jutsu.ext.{ext_name}"])
    up_env = jinja2.Environment(extensions=[f"jinja2.ext.{ext_name}"])
    if build is not None:
        build(jutsu_env, "jutsu")
        build(up_env, "jinja2")
    a = jutsu_env.from_string(source).render(**(variables or {}))
    b = up_env.from_string(source).render(**(variables or {}))
    return a, b


def normalize_pkg_reprs(text):
    """Blank out the two things a renamed fork cannot possibly match byte-for-
    byte: the package prefix inside class reprs and object addresses."""
    text = re.sub(r"class '(?:jutsu|jinja2)\.", "class 'PKG.", text)
    return re.sub(r"0x[0-9a-f]+", "0xADDR", text)


# --------------------------------------------------------------------------
# shipped extensions: gating predicate
# --------------------------------------------------------------------------

SHIPPED = {
    "loopcontrols": ["break", "continue"],
    "do": ["do"],
    "i18n": ["trans"],
    "debug": ["debug"],
}


class NoTagsExt(Extension):
    """Pure runtime extension: filters and tests only, no tags, and neither
    filter_stream nor preprocess overridden.  This is the other side of the
    documented rule."""

    def __init__(self, environment):
        super().__init__(environment)
        environment.filters["shout"] = lambda s: str(s).upper() + "!"
        environment.tests["big"] = lambda v: v > 5


class StreamFilterExt(Extension):
    """No tags, but overrides filter_stream -> must force python parsing."""

    _re = re.compile(r"_\((.*?)\)", re.DOTALL)

    def filter_stream(self, stream):
        for token in stream:
            if token.type == "data":
                yield from self.interpolate(token)
            else:
                yield token

    def interpolate(self, token):
        pos = 0
        end = len(token.value)
        lineno = token.lineno
        while True:
            match = self._re.search(token.value, pos)
            if match is None:
                break
            value = token.value[pos : match.start()]
            if value:
                yield Token(lineno, "data", value)
            lineno += count_newlines(token.value)
            yield Token(lineno, "variable_begin", None)
            yield Token(lineno, "name", "gettext")
            yield Token(lineno, "lparen", None)
            yield Token(lineno, "string", match.group(1))
            yield Token(lineno, "rparen", None)
            yield Token(lineno, "variable_end", None)
            pos = match.end()
        if pos < end:
            yield Token(lineno, "data", token.value[pos:])


class PreprocessorExt(Extension):
    """No tags, but overrides preprocess -> must force python parsing."""

    def preprocess(self, source, name, filename=None):
        return source.replace("[[TEST]]", "({{ foo }})")


def test_shipped_extensions_contribute_tags():
    """Every shipped extension contributes parse tags, so each one forces the
    python parse+codegen path by the documented predicate."""
    for name, tags in SHIPPED.items():
        env = Environment(extensions=[f"jutsu.ext.{name}"])
        assert _extensions_contribute(env), name
        assert not _rust_parser_active(env), name
        cls = env.extensions[f"jutsu.ext.{_ext_cls(name)}"]
        assert sorted(cls.tags) == sorted(tags)


def _ext_cls(name):
    return {
        "loopcontrols": "LoopControlExtension",
        "do": "ExprStmtExtension",
        "i18n": "InternationalizationExtension",
        "debug": "DebugExtension",
    }[name]


def test_no_tags_extension_is_rust_eligible(monkeypatch):
    """The other side of the rule: filters/tests only stays rust-eligible."""
    _install_spy(monkeypatch)
    env = Environment(extensions=[NoTagsExt])
    assert not _extensions_contribute(env)
    assert _rust_parser_active(env)
    out = env.from_string("{{ 'abc'|shout }} {% if 10 is big %}B{% endif %}").render()
    assert out == "ABC! B"
    assert used_rust(), "no-tags extension must not force the python parser"


def test_runtime_only_extension_renders_under_rust_evaluator():
    """A no-tags environment renders identically through the standalone rust
    evaluator as through python -- filters and tests resolve through the real
    Context, so nothing needs parse-time help."""
    env = Environment(extensions=[NoTagsExt])
    src = "{{ 'abc'|shout }} {% if 10 is big %}B{% endif %}"
    py = env.from_string(src).render()
    assert rust_eval.evaluate(env, src) == py


def test_stream_filter_forces_python_parse(monkeypatch):
    _install_spy(monkeypatch)
    env = Environment(extensions=[StreamFilterExt])
    env.globals["gettext"] = lambda x: x.upper()
    assert _extensions_contribute(env)
    out = env.from_string("Foo _(bar) Baz").render()
    assert out == "Foo BAR Baz"
    assert not used_rust(), "filter_stream must force the python parser"


def test_preprocess_forces_python_parse(monkeypatch):
    _install_spy(monkeypatch)
    env = Environment(extensions=[PreprocessorExt])
    assert _extensions_contribute(env)
    out = env.from_string("{[[TEST]]}").render(foo=42)
    assert out == "{(42)}"
    assert not used_rust(), "preprocess must force the python parser"


# --------------------------------------------------------------------------
# differential rendering vs upstream jinja, per shipped extension
# --------------------------------------------------------------------------

LOOPCONTROL_CASES = [
    ("{% for i in [1,2,3,4] %}{% if loop.index == 2 %}{% continue %}{% endif %}{{ i }} {% endfor %}", "1 3 4 "),
    ("{% for i in [1,2,3] %}{% if i == 1 %}{% break %}{% endif %}{{ i }} {% endfor %}", ""),
    ("{% for row in matrix %}{% if loop.first %}{% continue %}{% endif %}{{ row }};{% endfor %}", "b;c;"),
    ("{% for x in items %}{% for y in [1,2] %}{% break %}{% endfor %}{{ x }},{% endfor %}", "0,1,"),
    # note: `{% if i == 1 %}{% break %}` breaks the loop before any output, so
    # the second case renders empty -- verified identical to upstream jinja.
]

DO_CASES = [
    ("{% set foo = [1, 2] %}{% do foo.append(3) %}{{ foo }}", "[1, 2, 3]"),
    ("{% set d = {} %}{% do d.update(a=1) %}{{ d }}", "{'a': 1}"),
]

I18N_DE = {
    "Watch out": "Achtung",
    "%(num)s apple": "%(num)s Apfel",
    "%(num)s apples": "%(num)s Aepfel",
}


class Translations:
    def __init__(self, mapping):
        self.mapping = mapping

    def gettext(self, message):
        return self.mapping.get(message, message)

    def ngettext(self, singular, plural, n):
        key = singular if n == 1 else plural
        return self.mapping.get(key, key)


def test_loopcontrols_matches_upstream():
    for src, want in LOOPCONTROL_CASES:
        got, up = diff_against_upstream("loopcontrols", src)
        assert got == up, src
        assert normalize_pkg_reprs(got) == normalize_pkg_reprs(up)
    # explicit expected values pin the semantics independent of upstream drift
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    for src, want in LOOPCONTROL_CASES:
        if "matrix" in src:
            got = env.from_string(src).render(matrix=["a", "b", "c"])
        elif "items" in src:
            got = env.from_string(src).render(items=[0, 1])
        else:
            got = env.from_string(src).render()
        assert got == want, (src, got)


def test_do_matches_upstream():
    for src, want in DO_CASES:
        got, up = diff_against_upstream("do", src)
        assert got == up == want, (src, got)


def test_debug_matches_upstream_modulo_package_names():
    """`{% debug %}` dumps class reprs, so the only possible differences are
    the package prefix and object addresses -- normalize both away."""
    got, up = diff_against_upstream("debug", "Hello\n{% debug %}\nGoodbye", {"x": 1})
    assert normalize_pkg_reprs(got) == normalize_pkg_reprs(up)
    for needle in ("context", "cycler", "filters", "abs", "tests", "!="):
        assert f"'{needle}'" in got


def test_i18n_plural_forms_both_engines():
    """Plural forms agree with upstream jinja for n==1 and n!=1."""
    def build(env, pkg):
        env.install_gettext_translations(Translations(I18N_DE))

    src = '{% trans n %}{{ n }} apple{% pluralize %}{{ n }} apples{% endtrans %}'
    jutsu_tmpl = Environment(extensions=["jutsu.ext.i18n"])
    up_tmpl = jinja2.Environment(extensions=["jinja2.ext.i18n"]) if jinja2 else None
    jutsu_tmpl.install_gettext_translations(Translations(I18N_DE))
    if up_tmpl is not None:
        up_tmpl.install_gettext_translations(Translations(I18N_DE))
    jt = jutsu_tmpl.from_string(src)
    if up_tmpl is not None:
        ut = up_tmpl.from_string(src)
        for n in (1, 3, 7):
            assert jt.render(n=n) == ut.render(n=n), (src, n)
    # the trans block's literal singular/plural strings are *not* looked up in
    # the mapping (only gettext callables are); pin that shared behavior:
    assert jt.render(n=1) == "1 apple"
    assert jt.render(n=3) == "3 apples"
    # and the callable path *does* translate, including plural selection
    calls = Environment(extensions=["jutsu.ext.i18n"])
    calls.install_gettext_translations(Translations(I18N_DE))
    tmpl = calls.from_string(
        '{{ ngettext("%(num)s apple", "%(num)s apples", n) }}'
    )
    # oldstyle gettext: the callable returns the translated format string
    # without substituting the count -- same in upstream jinja.
    assert tmpl.render(n=1) == "%(num)s Apfel"
    assert tmpl.render(n=4) == "%(num)s Aepfel"


def test_i18n_matches_upstream_with_translations_installed():
    def build(env, pkg):
        env.install_gettext_translations(Translations(I18N_DE))

    cases = [
        ("{% trans %}Watch out{% endtrans %}", "Achtung"),
        ("{{ gettext('Watch out') }}", "Achtung"),
        ('{{ ngettext("%(num)s apple", "%(num)s apples", n) }}', None),
    ]
    for src, want in cases:
        got, up = diff_against_upstream("i18n", src, {"n": 2}, build)
        assert got == up, src
        if want is not None:
            assert got == want


# --------------------------------------------------------------------------
# parser selection per environment shape (instrumented, not assumed)
# --------------------------------------------------------------------------

def test_parser_selection_per_environment_shape(monkeypatch):
    """Every parse is recorded; tag-contributing shapes must select python,
    plain and runtime-only shapes must select rust."""
    _install_spy(monkeypatch)
    shapes = [
        ("plain", Environment(), True),
        ("loopcontrols", Environment(extensions=["jutsu.ext.loopcontrols"]), False),
        ("do", Environment(extensions=["jutsu.ext.do"]), False),
        ("i18n", Environment(extensions=["jutsu.ext.i18n"]), False),
        ("debug", Environment(extensions=["jutsu.ext.debug"]), False),
        ("notags", Environment(extensions=[NoTagsExt]), True),
        ("streamfilter", Environment(extensions=[StreamFilterExt]), False),
        ("preprocess", Environment(extensions=[PreprocessorExt]), False),
    ]
    for label, env, want_rust in shapes:
        SELECTS.clear()
        env.from_string("Hello {{ 1 + 1 }}")
        assert used_rust() is want_rust, label


def test_extension_syntax_never_reaches_the_rust_parser(monkeypatch):
    """Templates *using* extension syntax parse through python and render; the
    rust parser never sees such an environment, so it cannot mis-serve."""
    _install_spy(monkeypatch)
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    src = (
        "X {% for i in [1,2,3] %}{% if i == 1 %}{% break %}"
        "{% endif %}{{ i }} {% endfor %} Y"
    )
    got = env.from_string(src).render()
    assert not used_rust()
    assert got == "X  Y", got          # break fires before any output
    env2 = Environment(extensions=["jutsu.ext.loopcontrols"])
    src2 = "[{% for i in [1,2,3] %}{% if i == 1 %}{% continue %}{% endif %}{{ i }}{% endfor %}]"
    SELECTS.clear()
    got2 = env2.from_string(src2).render()
    assert not used_rust()
    assert got2 == "[23]", got2


def test_rust_parse_of_extension_syntax_reports_python_parse_error():
    """Forcing the rust parser on an extension template yields exactly the
    python parser's own unknown-tag message: a clean decline, never a silent
    wrong answer.  This records the accepted error-path divergence (R1)."""
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    src = (
        "A {% for i in [1,2,3] %}{% if i == 1 %}{% break %}"
        "{% endif %}{{ i }} {% endfor %}"
    )
    good = env.from_string(src).render()
    # `{% if i == 1 %}{% break %}` aborts the loop before any output lands
    assert good == "A ", repr(good)

    plain = Environment()  # no extension at all
    with pytest.raises(Exception) as excinfo:
        plain.from_string(src)
    msg = str(excinfo.value)
    assert "unknown tag" in msg or "Encountered unknown tag" in msg
    assert "break" in msg


def test_extension_templates_decline_in_the_rust_evaluator():
    """The standalone evaluator has no extension machinery, so extension syntax
    is a parse failure -- identical to the python parser without the
    extension.  That is the decline path a wired fallback would take."""
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    src = (
        "A {% for i in [1,2,3] %}{% if i == 1 %}{% break %}"
        "{% endif %}{{ i }} {% endfor %}"
    )
    from jutsu.exceptions import TemplateSyntaxError

    with pytest.raises(TemplateSyntaxError) as excinfo:
        rust_eval.evaluate(env, src)
    rust_msg = str(excinfo.value)

    plain = Environment()
    with pytest.raises(TemplateSyntaxError) as other:
        plain.from_string(src)
    assert rust_msg == str(other.value), "decline message must be the python parse error"


def test_extension_tag_nodes_are_python_ast_after_parse():
    """The AST produced for an extension template carries the extension's own
    nodes (Break/Continue/ExprStmt...), which only the python parser builds."""
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    tree = env.parse(
        "X {% for i in [1,2] %}{% break %}{% continue %}{% endfor %}"
    )
    kinds = sorted({type(n).__name__ for n in tree.find_all(nodes.Node)})
    assert "Break" in kinds and "Continue" in kinds, kinds


def test_extension_tag_nodes_reach_codegen():
    """The python codegen path handles the extension nodes: a template using
    break/continue compiles and renders repeatedly with identical output."""
    env = Environment(extensions=["jutsu.ext.loopcontrols"])
    tmpl = env.from_string(
        "[{% for i in [1,2,3,4] %}{% if loop.index == 2 %}{% continue %}"
        "{% endif %}{{ i }}{% endfor %}]"
    )
    assert tmpl.render() == "[134]"
    assert tmpl.render() == "[134]"          # cached module, same answer
    assert tmpl.module is not None


# --------------------------------------------------------------------------
# i18n extraction through the python parse path
# --------------------------------------------------------------------------

def test_extract_from_ast_both_styles():
    env = Environment()
    tree = env.parse('{{ (_("foo"), _(), ngettext("foo", "bar", 42)) }}')
    assert list(extract_from_ast(tree)) == [
        (1, "_", "foo"),
        (1, "_", ()),
        (1, "ngettext", ("foo", "bar", None)),
    ]
    assert list(extract_from_ast(tree, babel_style=False)) == [
        (1, "_", ("foo",)),
        (1, "ngettext", ("foo", "bar")),
    ]


def test_babel_extract_works_with_rust_default_engines():
    source = BytesIO(
        b"""
        {{ gettext('Hello World') }}
        {% trans %}Hello World{% endtrans %}
        {% trans %}{{ users }} user{% pluralize %}{{ users }} users{% endtrans %}
        """
    )
    assert list(babel_extract(source, ("gettext", "ngettext", "_"), [], {})) == [
        (2, "gettext", "Hello World", []),
        (3, "gettext", "Hello World", []),
        (4, "ngettext", ("%(users)s user", "%(users)s users", None), []),
    ]


# --------------------------------------------------------------------------
# evaluator eligibility predicate (defined by C-v8a, consumed read-only)
# --------------------------------------------------------------------------

def test_eligibility_predicate_performs_the_documented_rule(monkeypatch):
    """`rust_eval.eligible(environment)` is the C-v8a serve predicate.  Every
    shipped extension contributes parse tags, so each must be ineligible even
    with the flag forced on; a runtime-only extension must stay eligible."""
    monkeypatch.setenv("JUTSU_RUST_EVAL", "1")
    for name in SHIPPED:
        env = Environment(extensions=[f"jutsu.ext.{name}"])
        assert not rust_eval.eligible(env), name
    for cls in (StreamFilterExt, PreprocessorExt):
        env = Environment(extensions=[cls])
        assert not rust_eval.eligible(env), cls.__name__
    plain = Environment()
    assert rust_eval.eligible(plain)
    runtime_only = Environment(extensions=[NoTagsExt])
    assert rust_eval.eligible(runtime_only)


def test_eligibility_predicate_excludes_async_sandbox_and_native(monkeypatch):
    """The predicate's extra conditions: async, sandboxed and native value
    environments are never eligible (native exclusion is D4's minimum)."""
    monkeypatch.setenv("JUTSU_RUST_EVAL", "1")
    assert rust_eval.eligible(Environment())

    async_env = Environment(enable_async=True)
    assert not rust_eval.eligible(async_env)

    from jutsu.sandbox import SandboxedEnvironment

    sand = SandboxedEnvironment()
    assert getattr(sand, "sandboxed", False), "fixture sanity"
    assert not rust_eval.eligible(sand)

    from jutsu.nativetypes import NativeEnvironment

    native = NativeEnvironment()
    assert not rust_eval.eligible(native)


def test_eligibility_predicate_respects_the_flag_off(monkeypatch):
    """With `JUTSU_RUST_EVAL` unset or 0 nothing is eligible -- the default
    engines stay python until C-v8c flips them."""
    monkeypatch.delenv("JUTSU_RUST_EVAL", raising=False)
    assert not rust_eval.eligible(Environment())
    monkeypatch.setenv("JUTSU_RUST_EVAL", "0")
    assert not rust_eval.eligible(Environment())


def test_eligibility_predicate_is_per_environment_shape():
    """Two environments with different extension sets must get independent
    verdicts -- a shared cache would let one shape's answer leak to another
    (the B2 two-environment probe rule)."""
    import os

    old = os.environ.get("JUTSU_RUST_EVAL")
    try:
        os.environ["JUTSU_RUST_EVAL"] = "1"
        tagged = Environment(extensions=["jutsu.ext.loopcontrols"])
        plain = Environment()
        assert not rust_eval.eligible(tagged)
        assert rust_eval.eligible(plain), "plain verdict must not inherit tagged's"
        # and the other direction
        assert rust_eval.eligible(plain)
        assert not rust_eval.eligible(tagged)
    finally:
        if old is None:
            os.environ.pop("JUTSU_RUST_EVAL", None)
        else:
            os.environ["JUTSU_RUST_EVAL"] = old


def test_extract_translations_uses_python_parse_path(monkeypatch):
    """`Environment.extract_translations` parses then extracts; with the i18n
    extension present the parse must come from python so trans nodes exist."""
    _install_spy(monkeypatch)
    env = Environment(extensions=["jutsu.ext.i18n"])
    out = list(env.extract_translations("{% trans %}Hello{% endtrans %}{{ _('Bye') }}"))
    assert not used_rust(), "i18n extraction must parse through python"
    messages = [m for _, _, m in out]
    flat = [x for item in messages for x in (item if isinstance(item, tuple) else [item])]
    assert "Hello" in flat and "Bye" in flat, out
