"""Driver for the rust evaluator (slice v0, see PLAN_INTERP.md).

Renders templates fully inside rust -- parse to the rust AST and evaluate it
directly against a merged variables dict, without ever rebuilding python node
objects.  Templates using nodes outside the supported subset return ``None``
so callers can fall back to the python engine.
"""
try:  # optional rust core (built via maturin / cargo)
    from . import _jutsu
except ImportError:  # pragma: no cover - pure python installs
    _jutsu = None

from .exceptions import TemplateAssertionError, TemplateSyntaxError
from .rust_parse import wrapped_tokens


def has_rust_core() -> bool:
    return _jutsu is not None and hasattr(_jutsu, "eval_template")


def evaluate(environment, source, name=None, filename=None, variables=None):
    """Render ``source`` with the rust evaluator.

    Returns the rendered text, or ``None`` when the template uses nodes this
    slice cannot evaluate.  Parse/lexer errors are raised as the usual
    jinja exceptions.
    """
    toks, lexerr = wrapped_tokens(environment, source, name, filename)
    ctx = dict(environment.globals)
    if variables:
        ctx.update(variables)
    res = _jutsu.eval_template(toks, lexerr, ctx)
    kind = res[0]
    if kind == "__okrender__":
        return res[1]
    if kind == "__unsupported__":
        return None
    if kind == "__efail__":
        raise RuntimeError(res[1])
    if kind == "__lexerr__":
        raise TemplateSyntaxError(*res[1])
    if kind == "__perr__":
        message, lineno, cls = res[1], res[2], res[3]
        exc = TemplateAssertionError if cls == "assert" else TemplateSyntaxError
        raise exc(message, lineno, name, filename)
    raise RuntimeError(f"bad eval result {res!r}")
