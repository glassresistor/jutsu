"""The `shout` extension: a custom tag plus two filters, written against the
extension API exactly as a third party would write it (PLAN_D_PARALLEL.md D2).

It deliberately touches the parts of the API real extensions use:

* `tags` + `parse()` returning nodes built through `Extension.call_method`
  and `Extension.attr`, so it exercises the node-factory helpers;
* a filter registered in `__init__` via `environment.filters[...]`;
* `preprocess` is *not* overridden, so the class stays parse-tag-contributing
  but not source-preprocessing.
"""
from jutsu.ext import Extension
from jutsu import nodes


class ShoutExtension(Extension):
    """`{% shout NAME %}` renders `NAME` uppercased with exclamation marks."""

    tags = {"shout"}
    priority = 10

    def __init__(self, environment):
        super().__init__(environment)
        environment.filters["hush"] = lambda s: str(s).lower().strip()

    def parse(self, parser):
        lineno = next(parser.stream).lineno
        expr = parser.parse_expression()
        return nodes.Output(
            [self.call_method("_emit", [expr], lineno=lineno)], lineno=lineno
        )

    def _emit(self, value):
        return "!!!" + str(value).upper() + "!!!"

    @property
    def ext_marker(self):
        return f"{self.identifier}:ok"
