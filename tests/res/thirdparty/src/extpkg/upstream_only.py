"""A deliberately-conflicting extension: it subclasses *upstream* jinja2's
Extension base and registers a tag with the same name as the jutsu-side
synthetic extension.

D2 uses this to measure what actually happens when third-party code keeps
importing real `jinja2` inside a jutsu process (PLAN_D_PARALLEL.md): two
independent engines side by side.  The class can be instantiated and used by
a genuine jinja2 environment, but it is *not* a jutsu extension and must not
be accepted as one.
"""
import jinja2
from jinja2.ext import Extension


class UpstreamOnlyExtension(Extension):
    tags = {"shout"}

    def __init__(self, environment):
        super().__init__(environment)
        environment.filters["hush"] = lambda s: "u:" + str(s).lower()

    def parse(self, parser):
        from jinja2 import nodes as unodes

        lineno = next(parser.stream).lineno
        expr = parser.parse_expression()
        return unodes.Output(
            [self.call_method("_emit", [expr], lineno=lineno)], lineno=lineno
        )

    def _emit(self, value):
        return "UPPER:" + str(value).upper()


def jinja2_environment():
    """A real upstream jinja2 environment carrying this extension."""
    env = jinja2.Environment(extensions=[UpstreamOnlyExtension])
    return env
