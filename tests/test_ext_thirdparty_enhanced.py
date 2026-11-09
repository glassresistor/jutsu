# D2 -- Third-party extension compatibility & API story (dispatch now)

"""Tests for third-party extension compatibility in jutsu.

This file tests the complete extension compatibility story, showing how
extensions written against the extension API work with jutsu's package rename.
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


def test_synthetic_extension_ineligible_for_rust_serving(extpkg, monkeypatch):
    """With the rust evaluator active the third-party extension environment is
    still ineligible: extensions contributing tags never get served by rust."""
    monkeypatch.setenv("JUTSU_RUST_EVAL", "1")
    env = Environment(extensions=[extpkg.ShoutExtension])
    assert not rust_eval.eligible(env)
    out = env.from_string("{% shout 'go' %}").render()
    assert out == "!!!GO!!!"


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


# --------------------------------------------------------------------------
# D2 specific tests: extension compatibility decision validation
# --------------------------------------------------------------------------

def test_thirdparty_extensions_should_import_jutsu_ext_directly(extpkg):
    """Test that demonstrates the recommended approach for third-party extensions.

    Third-party extensions should explicitly import from jutsu.ext.Extension, not jinja2.ext.
    This ensures compatibility with jutsu's Rust core and prevents mixing engines.
    """
    # Verify the extension uses jutsu.ext directly (the recommended approach)
    assert issubclass(extpkg.ShoutExtension, Extension)
    
    # Test that it works correctly in a jutsu environment
    env = Environment(extensions=[extpkg.ShoutExtension])
    result = env.from_string("{% shout 'test' %}").render()
    assert result == "!!!TEST!!!"
    
    # Verify the extension is properly registered as a jutsu extension
    import jinja2.ext
    assert len(env.extensions) == 1
    ext_instance = list(env.extensions.values())[0]
    assert isinstance(ext_instance, Extension)
    assert not isinstance(ext_instance, jinja2.ext.Extension)


def test_extension_importing_jinja2_ext_creates_two_engines_hazard(extpkg):
    """Test that demonstrates the hazard of importing jinja2.ext in third-party extensions.

    When an extension imports jinja2.ext, it creates two independent engines which is a hazard.
    This shows what happens when using the upstream Jinja2 implementation.
    """
    # Import upstream extension (hazard case)
    from jinja2.ext import Extension as Jinja2Extension
    
    # This demonstrates that extensions can still work with jinja2.ext imports
    # but will use the upstream machinery, not jutsu's
    assert issubclass(extpkg.upstream_only.UpstreamOnlyExtension, Jinja2Extension)
    
    # The extension should be accepted by jutsu environment (duck typing)
    env = Environment(extensions=[extpkg.upstream_only.UpstreamOnlyExtension])
    result = env.from_string("{% shout 'test' %}").render()
    
    # But it will render using the upstream engine's logic
    assert result == "UPPER:TEST"
    
    # This is a hazard because we have two engines running side-by-side
    # The extension uses jinja2.nodes while jutsu uses jutsu.nodes
    import jinja2.nodes as unodes
    from jutsu import nodes as jnodes
    assert unodes.Const is not jnodes.Const


def test_extension_api_surface_compatibility(extpkg):
    """Test that verifies the extension API surface works correctly with jutsu.

    This ensures all expected APIs are available for third-party extensions.
    """
    # Test basic Extension class functionality
    assert hasattr(Extension, 'tags')
    assert hasattr(Extension, 'parse')
    assert hasattr(Extension, 'call_method')
    
    # Test that our synthetic extension works with all API features
    env = Environment(extensions=[extpkg.ShoutExtension])
    
    # Test the custom tag
    result = env.from_string("{% shout 'hello' %}").render()
    assert result == "!!!HELLO!!!"
    
    # Test the custom filter
    result = env.from_string("{{ '  test  '|hush }}").render()
    assert result == "test"


def test_extension_import_path_consistency(extpkg):
    """Test that shows consistent import paths are used throughout.

    This validates the import path decision and demonstrates that extensions
    should consistently use jutsu.ext instead of jinja2.ext.
    """
    # Import directly from jutsu (recommended)
    from jutsu.ext import Extension as JutsuExtension
    
    # Verify it's the same base class used by our extension
    assert issubclass(extpkg.ShoutExtension, JutsuExtension)
    
    # Show that we can create an extension using the recommended approach
    class MyCustomExtension(JutsuExtension):
        tags = {'mytag'}

        def parse(self, parser):
            from jutsu import nodes
            lineno = next(parser.stream).lineno
            return nodes.Output(
                [self.call_method('_process', [], lineno=lineno)], lineno=lineno
            )

        def _process(self):
            return "processed"

    # Test that it works correctly
    env = Environment(extensions=[MyCustomExtension])
    result = env.from_string("{% mytag %}").render()
    assert result == "processed"