# Third-party extension compatibility (D2)

Phase D package D2 settles how extensions written against the jinja extension
API keep working in jutsu after the package rename (`jinja2.*` -> `jutsu.*`).
The old TODO said "add a compat shim if needed"; this document records the
decision instead of leaving it open.

## Measured state of the world

* The extension API surface is **byte-identical** between jutsu and upstream
  jinja 3.1.6.  `dir(jutsu.ext.Extension)` and `dir(jinja2.ext.Extension)` both
  yield exactly:
  `attr, bind, call_method, filter_stream, parse, preprocess, priority, tags`.
* The module member lists match too (`jutsu.ext` exports the same names as
  `jinja2.ext`, including the alias attributes `i18n`, `do`, `loopcontrols`,
  `debug` used by the `extensions=["...ext.i18n"]` import-string form).
* Method signatures differ only in annotation spelling (`Optional[str]` vs
  `str | None`) -- never in parameters or behavior.
* jutsu is installed as a *separate distribution* from Jinja2; both can be
  present in one interpreter (verified: this repo's test environment has
  Jinja2 3.1.6 and jutsu side by side).

## Decision 1 -- extensions must subclass `jutsu.ext.Extension`

A third-party extension that wants to run inside jutsu subclasses
`jutsu.ext.Extension` and registers itself the same way the shipped ones do:
either as a class object or as an import string (`extensions=[
"mypkg.ext.MyExtension"]`).  Discovery is unchanged from upstream.

The synthetic package under `tests/res/thirdparty/` proves this end-to-end:
`extpkg.ShoutExtension` subclasses `jutsu.ext.Extension`, ships a custom tag
(`{% shout %}`) plus a filter (`hush`), gets picked up from `sys.path`, and
renders correctly -- including in an environment where the rust cores are
active (there it is simply python-served, because it contributes parse tags).

## Decision 2 -- code importing real `jinja2` gets upstream's machinery

Third-party code that still imports real `jinja2` keeps working through
upstream's own engine.  The two packages are independent: classes are
distinct, environments are distinct, and nothing bridges them.

**The hazard**, measured rather than assumed (see
`tests/test_ext_thirdparty.py::test_upstream_extension_ducks_into_jutsu_*`):

* Registration is **duck-typed**.  jutsu's `Environment.call_on_extensions`
  machinery identifies extensions by the `identifier` class attribute and
  invokes `parse()` through the same protocol upstream uses, so a class
  subclassing `jinja2.ext.Extension` *is accepted* into a jutsu environment
  and even renders.  There is no import error to warn you.
* What comes back is a **mixed AST**.  The extension's own node classes come
  from `jinja2.nodes`, while every node the jutsu parser builds (`Const`,
  `Output`, ...) comes from `jutsu.nodes` -- the two are distinct class
  objects with identical names.  Verified concretely: parsing
  `{% shout 'hey' %}` in a jutsu environment through an upstream-based
  extension yields a `jinja2.nodes.Call` whose argument is a
  `jutsu.nodes.Const`.
* It works anyway today because the two node sets are structurally identical
  (same fields, same `as_const`/`compile` protocols), and codegen only ever
  sees the mixed tree through duck-typed attribute access.  That makes this a
  latent compatibility dependency rather than a supported feature: nothing
  asserts it, so a future divergence in either package's node internals could
  break such an extension with no clear error.
* The reverse also holds: jutsu-based extensions work inside a real
  `jinja2.Environment` for the same structural reason.

So the hazard is not "it breaks loudly" but "it works silently on borrowed
structurals", which is worse for long-term maintenance.  The guidance stands:
third-party code should subclass `jutsu.ext.Extension` when running under
jutsu, and importing real `jinja2` inside a jutsu process remains possible
but unsupported.

`extpkg.upstream_only` demonstrates both sides: its extension works in a real
`jinja2.Environment`, is *not* a subclass of `jutsu.ext.Extension`, yet still
renders under jutsu through the mixed AST described above.

## Decision 3 -- no `jutsu.ext.api` re-export module

Question asked by the plan: "whether a `jutsu.ext.api` re-export module is
needed: measure the actual upstream extension API surface and record yes/no
with rationale."

**Recorded: no.**  Rationale:

* Upstream jinja has no `jinja2.ext.api` either -- `jinja2.ext` is a single
  flat module, and that is exactly what `jutsu.ext` is (verified by walking
  both packages with `pkgutil`).  Adding an extra level would invent a
  difference between the fork and its source.
* The measured surface is eight public members, all already importable from
  `jutsu.ext`, plus the node module they build (`jutsu.nodes`) and the lexer
  types (`jutsu.lexer.Token` / `TokenStream`).  A re-export would add no
  capability, only an alias.
* Extensions written against upstream need a *rename*, not a reshaped API.
  The rename is mechanical (`import jinja2.ext` -> `import jutsu.ext`) and
  nothing about it is optional -- which is why the hazard in Decision 2 is
  documented rather than papered over.

## Decision 4 -- no `jinja2`-named alias package (deferred)

Whether jutsu ever ships a `jinja2`-named alias package is **not** decided
here.  PLAN_D_PARALLEL.md defers it to the Phase-F drop-in experiment (the
`jinja2/__init__.py` shim that re-exports jutsu and runs jinja's own test
suite).  Deciding it now would pre-empt evidence that experiment is meant to
produce.

## Validation

* `tests/test_ext_thirdparty.py` -- synthetic package installed via `sys.path`,
  exercised end-to-end, plus the two-engine hazard assertions.  Green with the
  default engines and green with `JUTSU_RUST_EVAL=1`.
* Forked `tests/test_ext.py` remains green unchanged under both settings.
* Zero edits to existing src files (D2 owns new files only).
