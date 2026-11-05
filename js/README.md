# @jutsu/core (JavaScript wrapper, Phase E)

A JavaScript package for the jutsu template engine. Slice E0 is the
scaffold without the engine: the value bridge, the filters subset, the tests
registry and an `Environment` shell that declines compile/render until a
wasm backend is attached (E2). See PLAN_JS.md for the plan; STATUS.md tracks
progress.

## Layout

    src/values.mjs      Undefined family, Markup/escape, str()/truthiness,
                        len/iter/eq/compare/contains bridge rules
    src/filters.mjs     ported filter subset + registry (filters.py mirror)
    src/tests.mjs       tests registry (tests.py mirror)
    src/environment.mjs Environment shell: registration, undefined factory,
                        autoescape, getitem/getattr, callFilter/callTest,
                        attachBackend/compile/render decline points
    src/index.mjs       public surface
    test/               node:test suites + golden.json replay harness

## Running the tests

    node --test js/test/          # from the repo root (no npm deps)
    python scripts/js_golden.py   # regenerate js/test/golden.json

The golden file is optional: without it the golden suites skip, and the
direct rule suites still run. With it present every CPython-captured case is
replayed against the JS bridge.

## Parity caveats (E0 scope)

These are deliberate deviations or approximations; each is pinned by a test
or documented here as required by decision D-E4:

1. **Python filters and extensions are NOT available in JS.** The ported
   subset is the documented Phase-E list (`upper/lower/join/length/default/
   escape` plus companions). Anything else is *absent* from the registry,
   never approximated -- `ABSENT_FILTERS`/`ABSENT_TESTS` enumerate the names
   that would exist under full parity.
2. **Type mapping.** `None <-> null` (JS `undefined` is not None); dicts are
   `Map` *or* plain object literals; tuples have no representation (tuple
   golden cases skip). Numeric dict keys need a `Map`: plain-object keys are
   strings, so `{1: 'a'}` becomes `new Map([[1,'a']])`.
3. **int/float indistinguishable.** JS cannot tell `1` from `1.0`, so the
   `integer`/`float` tests approximate via `Number.isInteger`; those two
   golden cases skip.
4. **Unicode character classes are ASCII-fast-pathed** where CPython uses
   full cased-letter tables (`str.islower`, title-casing internals) and
   whitespace code points in `trim`. BMP behavior matches; exotic code point
   sets are a known gap until the golden suites widen.
5. **No built-in attribute leakage.** Python's getattr finds `dict.keys` or
   `str.upper` as bound methods; the JS bridge resolves attributes only on
   plain objects and user class instances, never on Array/Map/Set/string/
   number built-ins (`Map.size`, `array.length` etc. must not surface into
   templates). This is a documented divergence from CPython, pinned in the
   lookup rule tests rather than golden.
6. **Comparison failure messages** carry CPython's operand-order quirks:
   `sorted()` names operands as `'int' and 'str'` (incoming-vs-earlier),
   while direct `>` probes name `'str' and 'int'`; both shapes are pinned.

## Decline, not guess (R1)

A JS `Environment` without an attached backend throws `EngineUnavailable`
on `compile()`/`render()` -- it does not ship a secret second interpreter.
Filters/tests/bridge rules are reachable standalone through
`callFilter`/`callTest` and the lookup primitives; template compilation is
E2's job over the wasm core.
