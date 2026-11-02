"""Driver for the rust evaluator (slice v1, see PLAN_INTERP.md).

Renders templates fully inside rust -- parse to the rust AST and evaluate it
directly against a real `Context` created by the python runtime helpers, so
undefined semantics, filter/test lookup and ``pass_context`` injection all
follow the compiled engine.  Templates using nodes outside the supported
subset return ``None`` so callers can fall back to the python engine; runtime
exceptions propagate as the very same python exception objects the compiled
code would raise.
"""
try:  # optional rust core (built via maturin / cargo)
    from . import _jutsu
except ImportError:  # pragma: no cover - pure python installs
    _jutsu = None

from collections import OrderedDict

from . import runtime as _runtime
from .exceptions import TemplateAssertionError, TemplateSyntaxError
from .rust_parse import wrapped_tokens


def has_rust_core() -> bool:
    return _jutsu is not None and hasattr(_jutsu, "eval_template")


def eligible(environment) -> bool:
    """May the rust evaluator serve templates from ``environment``?

    The predicate mirrors the parser's gate (PLAN_PARSER.md) plus the extra
    conditions a render path must not bypass: core present, the
    ``JUTSU_RUST_EVAL`` flag on (default off during slice C-v8a), no
    extension contributing parse tags / stream filters / preprocessing (the
    same test as :func:`jutsu.parser._extensions_contribute`), not async,
    not sandboxed, and the plain :class:`Environment` class only -- native
    value environments are excluded until D4 defines their output mode.
    """
    import os

    if _jutsu is None or not has_rust_core():
        return False
    if os.environ.get("JUTSU_RUST_EVAL", "0") == "0":
        return False
    if getattr(environment, "is_async", False):
        return False
    if getattr(environment, "sandboxed", False):
        return False
    # native value environments evaluate to python objects, not text, so only
    # the exact plain Environment class (or an overlay of it, which shares the
    # same type) is served; subclasses such as NativeEnvironment are excluded
    # until D4 defines their output mode.
    from .environment import Environment

    if type(environment) is not Environment:
        return False
    # deferred import avoids an environment <-> parser import cycle.
    from .parser import _extensions_contribute

    if _extensions_contribute(environment):
        return False
    return True


def make_context(environment, variables=None, name=None, globals_=None):
    """Build the same `Context` a `Template` would use for this render."""
    if globals_ is None:
        globals_ = environment.make_globals(None)
    return _runtime.new_context(
        environment, name, {}, dict(variables or {}), False, globals_, None
    )


def prepare_sources(environment, templates):
    """Wrap every extra template source into the same token form the rust core
    consumes.  A source that cannot be tokenized cleanly is passed through as
    ``None`` so the evaluator knows to fall back when it reaches for it.
    """
    if templates is None:
        return None
    out: dict[str, object] = {}
    for tname, tsrc in templates.items():
        toks, lexerr = wrapped_tokens(environment, tsrc, tname, None)
        out[tname] = None if lexerr is not None else toks
    return out


def serve_verdict(template, ctx):
    """Decide once whether ``template`` may be served from rust.

    Runs only the compile-time analyses against a real python ``Context``
    (never executing), and returns ``(supported, reason)``.  A supported
    verdict additionally guarantees the template reaches no other template
    (no extends/include/import nodes), which is what lets evaluation render
    start-to-finish without declining part-way -- the never-guess invariant.
    """
    env = template.environment
    src = _template_source(template)
    if src is None:
        return False, "no source retained"
    ast = _cached_compile(env, src, template.name, template.filename)
    if ast is None:  # core predates the split entry points
        return False, "core lacks compile_ast"
    ok, reason = _jutsu.check_supported(ast, ctx, _runtime, template.name)
    return bool(ok), reason


def render_served(template, ctx):
    """Render ``template`` through the rust evaluator using a real python
    ``Context`` built by :meth:`Template.new_context`.

    Returns the rendered text.  A supported template (per :func:`serve_verdict`
    -- scan/layout clean and no cross-template nodes) evaluates start-to-finish
    without declining, so this either returns text or raises the same python
    exception the compiled engine would raise; it never returns ``None`` for a
    verdict-supported template.
    """
    env = template.environment
    src = _template_source(template)
    ast = _cached_compile(env, src, template.name, template.filename)
    res = _jutsu.eval_parsed(ast, ctx, _runtime, template.name, None)
    return _serve_result(res, src)


def _serve_result(res, source):
    kind = res[0]
    if kind == "__okrender__":
        return res[1]
    if kind == "__unsupported__":
        # Cannot happen for a verdict-supported template; surface as an error
        # so a silent wrong-engine switch never hides a real divergence.
        raise RuntimeError(f"served template declined: {res[1]!r}")
    if kind == "__lexerr__":
        exc = TemplateSyntaxError(*res[1])
        _translate(exc, source)
        raise exc
    if kind == "__perr__":
        message, lineno, cls = res[1], res[2], res[3]
        exc = TemplateAssertionError if cls == "assert" else TemplateSyntaxError
        exc = exc(message, lineno, None, None)
        _translate(exc, source)
        raise exc
    raise RuntimeError(f"bad eval result {res!r}")


def _template_source(template):
    """Recover the original source string for a compiled ``Template``.

    Templates capture their source at compile time (slice C-v8a); returns
    ``None`` when it was not retained (bytecache-loaded templates), which
    makes callers treat the template as ineligible to serve from rust.
    """
    src = getattr(template, "source", None)
    if isinstance(src, str):
        return src
    return None


def _lexer_key(environment):
    """The lexer-cache key from `jutsu.lexer.get_lexer`: everything that can
    shape tokenization of a source (delimiters and strip flags)."""
    return (
        environment.block_start_string,
        environment.block_end_string,
        environment.variable_start_string,
        environment.variable_end_string,
        environment.comment_start_string,
        environment.comment_end_string,
        environment.line_statement_prefix,
        environment.line_comment_prefix,
        environment.trim_blocks,
        environment.lstrip_blocks,
        environment.newline_sequence,
        environment.keep_trailing_newline,
    )


_MAX_CACHED_TEMPLATES = 1024

_AST_CACHE: "OrderedDict[tuple, object]" = OrderedDict()


def _cached_compile(environment, source, name, filename):
    """Parse ``source`` into a cached rust AST handle (performance pass).

    Returns ``None`` when the loaded core predates the split entry points so
    callers can keep using the cold tokenize+parse+evaluate function.  Parse
    failures are cached as well -- they are deterministic for a key, and the
    stored lexer-error arguments carry the *name* and *filename* baked in at
    wrap time, which is why those belong to the cache key too.
    """
    if _jutsu is None or not hasattr(_jutsu, "compile_ast"):
        return None
    key = _lexer_key(environment) + (source, name, filename)
    ast = _AST_CACHE.get(key)
    if ast is not None:
        _AST_CACHE.move_to_end(key)
        return ast
    toks, lexerr = wrapped_tokens(environment, source, name, filename)
    ast = _jutsu.compile_ast(toks, lexerr)
    _AST_CACHE[key] = ast
    while len(_AST_CACHE) > _MAX_CACHED_TEMPLATES:
        _AST_CACHE.popitem(last=False)
    return ast


def evaluate(environment, source, name=None, filename=None, variables=None,
            templates=None, with_reason=False):
    """Render ``source`` with the rust evaluator.

    Returns the rendered text, or ``None`` when the template uses nodes this
    slice cannot evaluate.  Parse/lexer errors are raised as the usual jinja
    exceptions and runtime errors propagate untouched.  ``templates`` maps
    extra template names to their sources so that `{% extends %}` can be
    followed inside rust without going through python's loaders.

    Repeated renders of one source skip lexing and parsing entirely via the
    cached handle from :func:`_cached_compile`; older cores fall back to the
    single-shot entry point transparently.
    """
    ast = _cached_compile(environment, source, name, filename)
    ctx = make_context(environment, variables, name)
    srcs = prepare_sources(environment, templates)
    if ast is not None:
        res = _jutsu.eval_parsed(ast, ctx, _runtime, name, srcs)
    else:
        toks, lexerr = wrapped_tokens(environment, source, name, filename)
        res = _jutsu.eval_template(toks, lexerr, ctx, _runtime, name, srcs)
    kind = res[0]
    if kind == "__okrender__":
        return res[1]
    if kind == "__unsupported__":
        if with_reason:
            return None, res[1]
        return None
    if kind == "__lexerr__":
        exc = TemplateSyntaxError(*res[1])
        _translate(exc, source)
        raise exc
    if kind == "__perr__":
        message, lineno, cls = res[1], res[2], res[3]
        exc = TemplateAssertionError if cls == "assert" else TemplateSyntaxError
        exc = exc(message, lineno, name, filename)
        _translate(exc, source)
        raise exc
    raise RuntimeError(f"bad eval result {res!r}")


def _translate(exc, source):
    """Mark a parse error as already rewritten the way `Environment.compile`
    does via :func:`jutsu.debug.rewrite_traceback_stack`, so that `str()` of
    the exception carries just the message.
    """
    exc.translated = True
    exc.source = source
