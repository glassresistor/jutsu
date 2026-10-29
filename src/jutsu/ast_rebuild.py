"""Rebuild `jutsu.nodes` trees from the serialized AST produced by the rust
parser core.

Encoding (rust -> python), all flat tuples::

    ("__node__", ClassName, lineno, arg1, arg2, ...)
    ("__list__", item1, item2, ...)
    ("__tup__", item1, item2, ...)          # plain python tuple entries
    ("__val__", obj)                        # literal passed through untouched
    ("__joinstr__", part1, part2, ...)      # ''.join(parts), built in python
    ("__perr__", message, lineno[, kind])   # parse error

Literal token values are carried as opaque pyobjects so arbitrary-precision
integers and python float formatting survive the round trip exactly.
"""
import typing as t

from . import nodes


class LexReraise(Exception):
    """Carries raw python exception args to re-raise verbatim (lazy lexer error)."""

    def __init__(self, args: tuple) -> None:
        super().__init__("lexer error")
        self.args_tuple = args


class ASTError(Exception):
    """A parse error reported by the rust core (converted to a syntax error by
    callers).  ``kind`` selects the exception class."""

    def __init__(self, message: str, lineno: int | None, kind: str = "syntax") -> None:
        super().__init__(message)
        self.message = message
        self.lineno = lineno
        self.kind = kind


def _registry() -> dict[str, type[nodes.Node]]:
    reg: dict[str, type[nodes.Node]] = {}
    for name in dir(nodes):
        cls = getattr(nodes, name)
        if (
            isinstance(cls, type)
            and issubclass(cls, nodes.Node)
            and not getattr(cls, "abstract", False)
            and name != "InternalName"
        ):
            reg[name] = cls
    return reg


_REGISTRY: dict[str, type[nodes.Node]] = _registry()


def rebuild(data: t.Any) -> t.Any:
    if not isinstance(data, tuple) or not data:
        raise ValueError(f"invalid serialized AST entry: {data!r}")
    kind = data[0]
    if kind == "__val__":
        return data[1]
    if kind == "__list__":
        return [rebuild(x) for x in data[1:]]
    if kind == "__tup__":
        return tuple(rebuild(x) for x in data[1:])
    if kind == "__joinstr__":
        return "".join(rebuild(p) for p in data[1:])
    if kind == "__lexerr__":
        raise LexReraise(tuple(data[1]))
    if kind == "__perr__":
        message = data[1]
        lineno = data[2]
        kind_cls = data[3] if len(data) > 3 else "syntax"
        raise ASTError(message, lineno, kind_cls)
    if kind != "__node__":
        raise ValueError(f"invalid serialized AST entry: {data!r}")

    name, lineno = data[1], data[2]
    cls = _REGISTRY.get(name)
    if cls is None:
        raise ValueError(f"unknown node class {name!r}")
    built = [rebuild(a) for a in data[3:]]
    return cls(*built, lineno=lineno)
