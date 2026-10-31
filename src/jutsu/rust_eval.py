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

from . import runtime as _runtime
from .exceptions import TemplateAssertionError, TemplateSyntaxError
from .rust_parse import wrapped_tokens


def has_rust_core() -> bool:
    return _jutsu is not None and hasattr(_jutsu, "eval_template")


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


def evaluate(environment, source, name=None, filename=None, variables=None,
            templates=None, with_reason=False):
    """Render ``source`` with the rust evaluator.

    Returns the rendered text, or ``None`` when the template uses nodes this
    slice cannot evaluate.  Parse/lexer errors are raised as the usual jinja
    exceptions and runtime errors propagate untouched.  ``templates`` maps
    extra template names to their sources so that `{% extends %}` can be
    followed inside rust without going through python's loaders.
    """
    toks, lexerr = wrapped_tokens(environment, source, name, filename)
    ctx = make_context(environment, variables, name)
    srcs = prepare_sources(environment, templates)
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
