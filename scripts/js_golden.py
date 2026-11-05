#!/usr/bin/env python3
"""Generate js/test/golden.json -- CPython/jutsu behavior captured as golden
values for the JavaScript value bridge (PLAN_JS.md slice E0, decision D-E1).

Every case is a declarative replayable operation the JS side can execute
without an engine: Undefined message shapes, filter calls through
Environment.call_filter, test calls through Environment.call_test, and the
getitem/getattr lookup rules.  Template-render cases are recorded for the
E2/E3 wasm suites and skipped by E0 (no JS engine yet).

Run:  python scripts/js_golden.py            (from the repo root)
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

import jutsu  # noqa: E402
from jutsu import ChainableUndefined, DebugUndefined  # noqa: E402
from jutsu import Environment, StrictUndefined, Undefined  # noqa: E402
from markupsafe import Markup  # noqa: E402

ROOT = os.path.join(os.path.dirname(__file__), "..")
MISSING = jutsu.runtime.missing


def enc(v):
    """Encode a python value into the golden JSON form."""
    if isinstance(v, Undefined):
        return {"undefined": {"message": v._undefined_message,
                              "str": str(v)}}
    if v is None:
        return {"none": True}
    if isinstance(v, bool):
        return {"bool": v}
    if isinstance(v, Markup):
        return {"markup": str(v)}
    if isinstance(v, int):
        return {"num": [v, "int"]}
    if isinstance(v, float):
        return {"num": [_num_text(v), "float"]}
    if isinstance(v, str):
        return {"str": v}
    if isinstance(v, (list, tuple)):
        return {"list": [enc(x) for x in v]}
    if isinstance(v, dict):
        return {"dict": [[enc(k), enc(x)] for k, x in v.items()]}
    if hasattr(v, "__iter__"):  # lazy generators/views: materialize
        return {"list": [enc(x) for x in v]}
    raise TypeError(f"cannot encode {type(v)}: {v!r}")


def _num_text(f):
    """JSON cannot carry inf/nan; keep them as strings for the float tag."""
    if f != f:
        return "nan"
    if f == float("inf"):
        return "inf"
    if f == float("-inf"):
        return "-inf"
    return f


def run(fn, *args):
    try:
        result = fn(*args)
    except Exception as exc:  # noqa: BLE001 -- capture every shape
        return {"error": [type(exc).__name__, str(exc)]}
    return {"value": enc(result)}


def main():
    env = Environment()
    autoenv = Environment(autoescape=True)
    strictenv = Environment(undefined=StrictUndefined)
    chainenv = Environment(undefined=ChainableUndefined)
    debugenv = Environment(undefined=DebugUndefined)
    out = []

    def add(case):
        assert "id" in case and "op" in case, case
        out.append(case)

    # ------------------------------------------------- Undefined messages
    msg_cases = [
        ("undef-hint", "base", "No first item, sequence was empty.", None, None),
        ("undef-name", "base", None, None, "foo"),
        ("undef-attr", "base", None, {"a": 1}, "b"),
        ("undef-element-int", "base", None, [1, 2], 0),
        ("undef-element-str", "base", None, [1, 2], "x"),
        ("undef-tuple-element", "base", None, (1, 2), 5),
        ("undef-strict-name", "strict", None, None, "foo"),
        ("undef-chain-name", "chainable", None, None, "foo"),
        ("undef-debug-name", "debug", None, None, "foo"),
    ]
    cls_map = {"base": Undefined, "strict": StrictUndefined,
               "chainable": ChainableUndefined, "debug": DebugUndefined}

    def mk(kind, hint, obj, name):
        if obj is None:
            return cls_map[kind](hint=hint, name=name)
        return cls_map[kind](hint=hint, obj=obj, name=name)

    for cid, kind, hint, obj, name in msg_cases:
        u = mk(kind, hint, obj, name)
        try:
            raise u._undefined_exception(u._undefined_message)
        except Exception as exc:  # noqa: BLE001
            expect = {"error": [type(exc).__name__, str(exc)]}
        add({"id": cid, "op": "undefMessage", "kind": kind,
             "hint": hint, "obj": None if obj is None else enc(obj),
             "name": name, **expect})

    # ------------------------------------------------- Undefined operations
    def uop(cid, kind, hint, obj, name, action, params):
        u = mk(kind, hint, obj, name)
        fn = {"str": lambda: str(u), "bool": lambda: bool(u),
             "len": lambda: len(u), "iter": lambda: list(u),
             "add": lambda: u + 42, "call": lambda: u(1),
             "get": lambda: getattr(u, params["attr"] if "attr" in params else "x"),
             "item": lambda: u[params["key"]] if "key" in params else None,
             }[action]
        add({"id": cid, "op": "undefOp", "kind": kind, "hint": hint,
             "obj": None if obj is None else enc(obj), "name": name,
             "action": action, "params": params, **run(fn)})

    uop("undef-str", "base", None, None, "foo", "str", {})
    uop("undef-bool", "base", None, None, "foo", "bool", {})
    uop("undef-len", "base", None, None, "foo", "len", {})
    uop("undef-iter", "base", None, None, "foo", "iter", {})
    uop("undef-add", "base", None, None, "foo", "add", {})
    uop("undef-call", "base", None, None, "foo", "call", {})
    uop("undef-get", "base", None, None, "foo", "get", {"attr": "bar"})
    uop("strict-str", "strict", None, None, "foo", "str", {})
    uop("strict-bool", "strict", None, None, "foo", "bool", {})
    uop("strict-len", "strict", None, None, "foo", "len", {})
    uop("debug-str", "debug", None, None, "foo", "str", {})
    uop("debug-str-hint", "debug", "hmm", None, None, "str", {})
    uop("debug-str-obj", "debug", None, {"k": 1}, "kk", "str", {})
    uop("chain-str", "chainable", None, None, "foo", "str", {})
    uop("chain-get", "chainable", None, None, "foo", "get", {"attr": "bar"})
    uop("chain-item", "chainable", None, None, "foo", "item", {"key": 1})

    # --------------------------------------------------------- filters
    def cf(e, name, value, *fargs):
        return run(e.call_filter, name, value, list(fargs))

    filter_cases = [
        ("upper", env, "upper", "héllo"), ("lower", env, "lower", "HÉLLO"),
        ("cap", env, "capitalize", "hELLO wORLD"),
        ("title", env, "title", "the quick brown fox-jumps over"),
        ("trim", env, "trim", "  hi  "),
        ("trim-chars", env, "trim", "xxhixx", "x"),
        ("length-str", env, "length", "abcd"),
        ("length-list", env, "length", [1, 2, 3]),
        ("length-dict", env, "length", {"a": 1}),
        ("length-int", env, "length", 42),
        ("abs-neg", env, "abs", -3.5),
        ("int-str", env, "int", "42"),
        ("int-float-str", env, "int", "42.23"),
        ("int-hex", env, "int", "0x1a", 0, 16),
        ("int-octal", env, "int", "0o777"),
        ("int-bin", env, "int", "0b101", 0, 2),
        ("int-underscore", env, "int", "1_2"),
        ("int-bad-underscore", env, "int", "1__2"),
        ("int-ws", env, "int", " 7 "),
        ("int-float", env, "int", 42.9),
        ("int-nan", env, "int", float("nan")),
        ("float-str", env, "float", "3.5"),
        ("float-bad", env, "float", "x"),
        ("default-undef-x", env, "default", Undefined(name="u"), "dv"),
        ("default-empty-bool", env, "default", "", "dv", True),
        ("default-empty", env, "default", "", "dv"),
        ("join-list", env, "join", [1, 2, 3], "|"),
        ("join-str", env, "join", "abc", "-"),
        ("join-dict", env, "join", {"a": 1, "b": 2}, "+"),
        ("join-attr", env, "join", [{"n": "x"}, {"n": "y"}], ",", "n"),
        ("replace-plain", env, "replace", "aaa", "a", "b"),
        ("replace-count", env, "replace", "aaaa", "a", "b", 2),
        ("first-list", env, "first", [3, 4]),
        ("first-empty", env, "first", []),
        ("last-list", env, "last", [3, 4]),
        ("last-empty", env, "last", []),
        ("reverse-str", env, "reverse", "abc"),
        ("reverse-list", env, "reverse", [1, 2]),
        ("list-of", env, "list", {"k": 1}),
        ("sort-plain", env, "sort", [3, 1, 2]),
        ("sort-ci-str", env, "sort", ["b", "A", "a"]),
        ("sort-cs-str", env, "sort", ["b", "A", "a"], False, True),
        ("sort-reverse", env, "sort", [1, 3, 2], True),
        ("sort-attr", env, "sort", [{"n": 2}, {"n": 1}], False, False, "n"),
        ("escape-str", env, "escape", "<b>&"),
        ("escape-none", env, "escape", None),
        ("string-filter", env, "string", Undefined(name="u")),
    ]
    for spec in filter_cases:
        cid, e, name, value = spec[0], spec[1], spec[2], spec[3]
        fargs = list(spec[4:])
        r = cf(e, name, value, *fargs)
        add({"id": cid, "op": "filter", "name": name,
             "input": {"value": enc(value), "args": [enc(a) for a in fargs]},
             **r})

    # Undefined-valued filter inputs (message shapes through the filters):
    for cid, name, extra in [("abs-undef", "abs", []), ("int-undef", "int", []),
                             ("float-undef", "float", [])]:
        r = cf(env, name, Undefined(name="u"), *extra)
        add({"id": cid, "op": "filter", "name": name,
             "input": {"value": enc(Undefined(name="u")), "args": []}, **r})

    # autoescape join/replace via EvalContext (standalone call shape):
    def eval_call(e, fn_name, *args):
        from jutsu.nodes import EvalContext
        import jutsu.filters as JF
        ctx = EvalContext(e)  # autoescape derived from the environment
        fn = {"join": JF.sync_do_join, "replace": JF.do_replace}[fn_name]
        return run(fn, ctx, *args)

    for cid, fn_name, args in [
        ("join-auto-plain", "join", [["<a>", "<b>"], "<hr>"]),
        ("join-auto-markup-sep", "join", [["a", "b"], Markup("&x")]),
        ("join-auto-markup-item", "join", [[Markup("<m>"), "<e>"], "<hr>"]),
        ("replace-auto", "replace", ["aaa", "a", "<x>"]),
    ]:
        r = eval_call(autoenv, fn_name, *args)
        add({"id": cid, "op": "evalFilter", "name": fn_name,
             "input": {"args": [enc(a) for a in args]}, **r})

    # context filters decline cleanly without a context (R1):
    for cid, name, value, extra in [("map-noctx", "map", [1], ["string"]),
                                    ("select-noctx", "select", [1], ["odd"])]:
        r = run(env.call_filter, name, value, list(extra))
        add({"id": cid, "op": "filter", "name": name,
             "input": {"value": enc(value), "args": [enc(a) for a in extra]},
             **r})

    # --------------------------------------------------------- tests
    test_cases = [
        ("odd-3", "odd", 3, []), ("odd-neg", "odd", -3, []),
        ("even-4", "even", 4, []), ("divby-9-3", "divisibleby", 9, [3]),
        ("defined-x", "defined", 1, []),
        ("defined-u", "defined", Undefined(name="u"), []),
        ("undefined-u", "undefined", Undefined(name="u"), []),
        ("none-none", "none", None, []), ("none-zero", "none", 0, []),
        ("boolean-t", "boolean", True, []), ("boolean-1", "boolean", 1, []),
        ("false-f", "false", False, []), ("true-t", "true", True, []),
        ("integer-3", "integer", 3, []), ("integer-3.5", "integer", 3.5, []),
        ("float-3.0", "float", 3.0, []), ("float-3", "float", 3, []),
        ("lower-l", "lower", "abc", []), ("lower-mixed", "lower", "aBc", []),
        ("upper-u", "upper", "ABC", []), ("string-s", "string", "x", []),
        ("mapping-d", "mapping", {"a": 1}, []),
        ("number-1", "number", 1, []), ("number-t", "number", True, []),
        ("sequence-l", "sequence", [1], []), ("sequence-i", "sequence", 1, []),
        ("iterable-d", "iterable", {"a": 1}, []),
        ("iterable-i", "iterable", 1, []),
        ("sameas-f", "sameas", False, [False]),
        ("escaped-m", "escaped", Markup("x"), []),
        ("in-list", "in", 1, [[1, 2]]), ("in-str", "in", "a", ["abc"]),
        ("in-dict", "in", "a", [{"a": 1}]),
        ("eq-1", "==", 1, [1]), ("eq-t1", "==", True, [1]),
        ("ne-str", "!=", "a", [1]), ("gt-2", ">", 2, [1]),
        ("ge-eq", ">=", 1, [1]), ("lt-1", "<", 1, [2]),
        ("le-eq", "<=", 1, [1]), ("gt-str-int", ">", "a", [1]),
        ("ge-str-int", ">=", "a", [1]),
    ]
    for cid, name, value, args in test_cases:
        r = run(env.call_test, name, value, list(args))
        add({"id": cid, "op": "test", "name": name,
             "input": {"value": enc(value), "args": [enc(a) for a in args]},
             **r})

    # ------------------------------------------------------- lookups
    lookup_cases = [
        ("gi-dict", "getitem", {"a": 1}, "a"),
        ("gi-dict-miss", "getitem", {"a": 1}, "b"),
        ("gi-list-int", "getitem", [5, 6], 1),
        ("gi-list-str", "getitem", [5, 6], "0"),
        ("gi-list-oob", "getitem", [5, 6], 9),
        # python getitem is `obj[argument]`: negative subscripts wrap on
        # lists; out-of-range negatives still fall through to Undefined.
        ("gi-list-neg", "getitem", [5, 6], -1),
        ("gi-list-neg-oob", "getitem", [5, 6], -9),
        # getattr finding dict.keys / str.upper as bound methods is python
        # only -- JS rules never expose built-in methods (documented
        # divergence; pinned in the JS suite instead of golden).
        ("ga-dict", "getattr", {"a": 1}, "a"),
    ]
    for cid, kind, obj, arg in lookup_cases:
        fn = env.getitem if kind == "getitem" else env.getattr
        try:
            r = fn(obj, arg)
            expect = {"value": enc(r)}
        except Exception as exc:  # noqa: BLE001
            expect = {"error": [type(exc).__name__, str(exc)]}
        add({"id": cid, "op": kind, "obj": enc(obj), "arg": enc(arg),
             **expect})

    # ------------------------------------------------------ templates
    tpl_cases = [
        ("tpl-map", env, "{{ ['a','b']|map('upper')|join(',') }}"),
        ("tpl-select", env, "{{ [1,2,3,4]|select('odd')|join(',') }}"),
        ("tpl-reject", env, "{{ [1,2,3,4]|reject('odd')|join(',') }}"),
        ("tpl-selectattr", env,
         "{{ [{'a':1},{'a':0}]|selectattr('a')|list|length }}"),
        ("tpl-sort-attr", env,
         "{{ [{'n':2},{'n':1}]|sort(attribute='n')"
         "|map(attribute='n')|join('-') }}"),
        ("tpl-default", env, "{{ nothere|default('d!') }}"),
        ("tpl-undef-print", env, "[{{ nope }}]"),
        ("tpl-strict-print", strictenv, "[{{ nope }}]"),
        ("tpl-debug-print", debugenv, "[{{ nope }}]"),
        ("tpl-chain-print", chainenv, "[{{ a.b.c }}]"),
    ]
    for cid, e, src in tpl_cases:
        try:
            expect = {"value": enc(e.from_string(src).render())}
        except Exception as exc:  # noqa: BLE001
            expect = {"error": [type(exc).__name__, str(exc)]}
        add({"id": cid, "op": "template", "src": src, **expect})

    data = {"generated_by": "scripts/js_golden.py", "cases": out}
    path = os.path.join(ROOT, "js", "test", "golden.json")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as fh:
        json.dump(data, fh, indent=1, sort_keys=False)
    print(f"wrote {path}: {len(out)} cases")


if __name__ == "__main__":
    main()
