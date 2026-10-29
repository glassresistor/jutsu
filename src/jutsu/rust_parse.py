"""Driver for the rust parser core.

Feeds wrapped token streams into ``_jutsu.parse`` and rebuilds `jutsu.nodes`
trees from the serialized AST (see :mod:`jutsu.ast_rebuild`).  Used by the
differential tests; later phases may make it authoritative.

The python ``TokenStream`` is lazy: a lexer error surfaces only when the parser
advances into it.  To reproduce that, ``wrapped_tokens`` truncates the stream at
the failure point and hands the exception's args to the rust core, which then
raises them exactly where python would have.
"""
import typing as t

from . import _jutsu
from .ast_rebuild import ASTError, LexReraise, rebuild
from .exceptions import TemplateAssertionError, TemplateSyntaxError
from .lexer import TokenStream, get_lexer


def has_rust_core() -> bool:
    return hasattr(_jutsu, "parse")


def wrapped_tokens(
    environment,
    source: str,
    name: str | None = None,
    filename: str | None = None,
    state: str | None = None,
):
    """Produce the post-``wrap`` token tuples plus any pending lexer error.

    The wrapped generator is consumed directly (not through ``TokenStream``:
    its iterator advances *before* yielding, which would drop the last token
    successfully fetched before the error).  Every partial token is a valid,
    consumable token; the error only surfaces on the fetch *past* them.
    """
    lexer = get_lexer(environment)
    gen = lexer.wrap(lexer.tokeniter(source, name, filename, state))
    out: list[tuple[int, str, t.Any]] = []
    try:
        for tok in gen:
            out.append((tok.lineno, tok.type, tok.value))
    except TemplateSyntaxError as exc:
        # full constructor args so the error can be re-raised verbatim later
        return out, (exc.args[0], exc.lineno, exc.name, exc.filename)
    return out, None


def _raise(err: ASTError, name: str | None, filename: str | None):
    cls = TemplateAssertionError if err.kind == "assert" else TemplateSyntaxError
    return cls(err.message, err.lineno, name, filename)


def parse_template(environment, source, name=None, filename=None):
    """Parse a whole template into a rebuilt ``Template`` node."""
    toks, lexerr = wrapped_tokens(environment, source, name, filename)
    result = _jutsu.parse(toks, lexerr)
    try:
        return rebuild(result)
    except ASTError as exc:
        raise _raise(exc, name, filename) from None
    except LexReraise as exc:
        raise TemplateSyntaxError(*exc.args_tuple) from None
