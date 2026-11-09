"""D2: third-party extension compatibility & API story (PLAN_D_PARALLEL.md).

A synthetic third-party extension package lives under
`tests/res/thirdparty/` and is installed into the test environment through
`sys.path` at runtime, then exercised in an environment that also has the rust
cores active.  The companion module `extpkg.upstream_only` measures what
happens when third-party code keeps importing real `jinja2` -- two independent
engines side by side -- which is the hazard documented in
docs/EXT_COMPAT.md.

Owned files: this module, docs/EXT_COMPAT.md and the fixtures.  Zero edits to
existing src files.
"""
import sys
from pathlib import Path

import pytest

from jutsu import Environment
from jutsu import rust_eval
from jutsu.ext import Extension
from jutsu.parser import _extensions_contribute
from jutsu.parser import _rust_parser_active

HERE = Path(__file__).resolve().parent
FIXTURE_SRC = HERE / "res" / "thirdparty" / "src"


def install_package_on_path():
    """Put the synthetic package on `sys.path` and return a fresh importer.

    Returns (extpkg_module, cleanup).  The import is invalidated first so a
    previous test's module cache cannot leak in.
    """
    if str(FIXTURE_SRC) not in sys.path:
        sys.path.insert(0, str(FIXTURE_SRC))
    for stale in [m for m in list(sys.modules) if m.startswith("extpkg")]:
        del sys.modules[stale]
    import extpkg  # noqa: SAE000 (synthetic package installed by this test)

    return extpkg


def cleanup_package_path():
    for stale in [m for m in list(sys.modules) if m.startswith("extpkg")]:
        del sys.modules[stale]
    try:
        sys.path.remove(str(FIXTURE_SRC))
    except ValueError:
        pass


@pytest.fixture
def extpkg():
    module = install_package_on_path()
    yield module
    cleanup_package_path()


# --------------------------------------------------------------------------
# end-to-end: the synthetic package as a real out-of-tree extension
# --------------------------------------------------------------------------

def test_synthetic_package_is_importable_from_sys_path(extpkg):
    assert extpkg.__version__ == "0.1"
    assert issubclass(extpkg.ShoutExtension, Extension)
    assert extpkg.ShoutExtension.tags == {"shout"}


def test_synthetic_extension_renders_end_to_end(extpkg):
    env = Environment(extensions=[extpkg.ShoutExtension])
    tmpl = env.from_string("{% shout 'hello there' %}|{{ '  CaSeD  '|hush }}")
    assert tmpl.render() == "!!!HELLO THERE!!!|cased"


def test_synthetic_extension_forces_python_parse(extpkg, monkeypatch):
    """A third-party tag extension must force the python parse+codegen path."""
    calls = []
    from jutsu.parser import Parser

    original = Parser.parse

    def spy(self):
        calls.append(_rust_parser_active(self.environment))
        return original(self)

    monkeypatch.setattr(Parser, "parse", spy)
    env = Environment(extensions=[extpkg.ShoutExtension])
    env.from_string("{% shout 'x' %}")
    assert calls and not any(calls), calls
    assert _extensions_contribute(env)


@pytest.mark.skipif(not rust_eval.has_rust_core(), reason="requires rust core")
def test_synthetic_extension_ineligible_for_rust_serving(extpkg, monkeypatch):
    """With the rust evaluator active the third-party extension environment is
    still ineligible: extensions contributing tags never get served by rust."""
    monkeypatch.setenv("JUTSU_RUST_EVAL", "1")
    env = Environment(extensions=[extpkg.ShoutExtension])
    assert not rust_eval.eligible(env)
    out = env.from_string("{% shout 'go' %}").render()
    assert out == "!!!GO!!!"


@pytest.mark.skipif(not rust_eval.has_rust_core(), reason="requires rust core")
def test_synthetic_extension_coexists_with_rust_cores(extpkg, monkeypatch):
    """An environment carrying the third-party extension plus plain templates:
    the plain ones may be rust-served while the extension one is python-served,
    and both must render correctly."""
    monkeypatch.setenv("JUTSU_RUST_EVAL", "1")
    plain = Environment()
    assert rust_eval.eligible(plain)
    assert plain.from_string("{{ 1 + 2 }}").render() == "3"

    mixed = Environment(extensions=[extpkg.ShoutExtension])
    assert not rust_eval.eligible(mixed)
    assert mixed.from_string("{% shout 'x' %}").render() == "!!!X!!!"


def test_extension_registered_by_import_string(extpkg):
    """Extensions may be named by import string exactly like the shipped ones,
    proving the package rename did not break discovery."""
    env = Environment(extensions=["extpkg.shout.ShoutExtension"])
    assert tmpl_out(env) == "!!!WHOA!!!"


def tmpl_out(env):
    return env.from_string("{% shout 'whoa' %}").render()


def test_extension_class_object_and_import_string_agree(extpkg):
    by_class = Environment(extensions=[extpkg.ShoutExtension])
    by_name = Environment(extensions=["extpkg.shout.ShoutExtension"])
    assert tmpl_out(by_class) == tmpl_out(by_name)


# --------------------------------------------------------------------------
# the two-engines hazard: upstream jinja2 imported by third-party code
# --------------------------------------------------------------------------

def test_upstream_only_extension_is_not_a_jutsu_extension(extpkg):
    import jinja2.ext

    cls = extpkg.upstream_only.UpstreamOnlyExtension
    assert issubclass(cls, jinja2.ext.Extension)
    assert not issubclass(cls, Extension), (
        "a class subclassing upstream jinja2 is a distinct base from the jutsu "
        "one -- the two engines are independent classes"
    )


def test_upstream_extension_ducks_into_jutsu_registration(extpkg):
    """Measured hazard: registration is duck-typed, so an upstream-based class
    is accepted into a jutsu environment with no import error."""
    env = Environment(extensions=[extpkg.upstream_only.UpstreamOnlyExtension])
    keys = list(env.extensions)
    assert keys == ["extpkg.upstream_only.UpstreamOnlyExtension"]


def test_upstream_extension_ducks_into_jutsu_yields_mixed_ast(extpkg):
    """The mixed-AST hazard: the extension's own node comes from jinja2.nodes
    while the parser-built nodes come from jutsu.nodes -- distinct class
    objects sharing names.  Documented in docs/EXT_COMPAT.md."""
    import jinja2.nodes as unodes
    from jutsu import nodes as jnodes

    assert unodes.Const is not jnodes.Const
    env = Environment(extensions=[extpkg.upstream_only.UpstreamOnlyExtension])
    tree = env.parse("{% shout 'hey' %}")
    call = tree.body[0].nodes[0]
    assert type(call).__module__ == "jinja2.nodes", type(call)
    assert all(type(a).__module__ == "jutsu.nodes" for a in call.args), call.args


def test_upstream_extension_ducks_and_renders_under_jutsu(extpkg):
    """...and it renders, because the two node sets are structurally identical.
    Recorded as an unsupported latent dependency, not a feature."""
    env = Environment(extensions=[extpkg.upstream_only.UpstreamOnlyExtension])
    out = env.from_string("{% shout 'hey' %}|{{ 'AB'|hush }}").render()
    assert out == "UPPER:HEY|u:ab", out


def test_upstream_extension_still_works_under_jinja2(extpkg):
    """Real jinja2 machinery keeps working for code that imports it."""
    env = extpkg.upstream_only.jinja2_environment()
    out = env.from_string("{% shout 'hey' %}|{{ 'AB'|hush }}").render()
    assert out == "UPPER:HEY|u:ab", out


def test_jutsu_extension_works_under_real_jinja2(extpkg):
    """The reverse direction of the same structural-compatibility effect: a
    jutsu-based extension renders inside an upstream jinja2 environment."""
    import jinja2
    from jutsu import nodes as jnodes

    class JutsuBasedInUpstream(Extension):
        tags = {"jup"}

        def parse(self, parser):
            lineno = next(parser.stream).lineno
            expr = parser.parse_expression()
            return jnodes.Output(
                [self.call_method("_e", [expr], lineno=lineno)], lineno=lineno
            )

        def _e(self, value):
            return "J:" + str(value).upper()

    env = jinja2.Environment(extensions=[JutsuBasedInUpstream])
    assert env.from_string("{% jup 'x' %}").render() == "J:X"


def test_jutsu_and_upstream_are_independent_namespaces():
    """`jutsu.ext.Extension` and `jinja2.ext.Extension` are distinct classes; a
    jutsu environment never accepts the latter."""
    import jinja2.ext

    assert jinja2.ext.Extension is not Extension
    env = Environment()
    assert not isinstance(env.from_string("{{ 1 }}").environment, jinja2.Environment)

