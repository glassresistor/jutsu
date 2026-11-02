"""A synthetic third-party extension package for jutsu.

Built and tested end-to-end by D2 (PLAN_D_PARALLEL.md) to prove the extension
API story: an out-of-tree package that subclasses `jutsu.ext.Extension`, ships
a custom tag plus a filter, gets discovered through the entry-point-free
`extensions=[...]` import-string mechanism, and renders correctly while the
rust cores are active.  Nothing in this package may import upstream `jinja2`
-- doing so would hand templates to a second, independent engine.
"""
from .shout import ShoutExtension
from . import upstream_only  # noqa: F401 (hazard probe module, D2)

__all__ = ["ShoutExtension", "upstream_only"]
__version__ = "0.1"
