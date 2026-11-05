// Filters-subset rule suites (PLAN_JS.md slice E0): registry integrity plus
// edge behaviors beyond the golden replays, all pinned directly.

import test from "node:test";
import assert from "node:assert/strict";
import * as V from "../src/index.mjs";
import { FILTERS, ABSENT_FILTERS } from "../src/filters.mjs";

const env = new V.Environment();
const autoenv = new V.Environment({ autoescape: true });
const cf = (name, value, args = []) => env.callFilter(name, value, args);

function expectRaises(fn, message) {
  assert.throws(fn, (e) => {
    if (!(e instanceof V.TemplateError)) throw e;
    if (message !== undefined && e.message !== message) {
      throw new Error(`wrong message: got '${e.message}' want '${message}'`);
    }
    return true;
  });
}

test("registry integrity", () => {
  for (const [name, entry] of Object.entries(FILTERS)) {
    assert.equal(typeof entry.func, "function", name + " has func");
    assert.ok(["", "env", "evalCtx", "ctx"].includes(entry.pass), name);
  }
  // absent filters are really absent (decision D-E4)
  for (const name of ABSENT_FILTERS) {
    assert.equal(FILTERS[name], undefined, `absent: ${name}`);
    assert.equal(env.hasFilter(name), false, `env absent: ${name}`);
  }
  // aliases registered per filters.py FILTERS dict
  for (const [alias, target] of [["d", "default"], ["e", "escape"],
    ["count", "length"]]) {
    assert.ok(FILTERS[alias], alias);
    assert.equal(FILTERS[alias].func, FILTERS[target].func, alias + " same fn");
  }
});

test("int() edges", () => {
  assert.equal(cf("int", "42"), 42);
  assert.equal(cf("int", "-7"), -7);
  assert.equal(cf("int", "+5"), 5);
  assert.equal(cf("int", " 12 "), 12, "whitespace tolerated");
  // filter args map positionally to do_int(value, default, base) exactly
  // like the python call_filter convention.
  assert.equal(cf("int", "0x1A", [0, 16]), 26, "upper-case prefix digits");
  assert.equal(cf("int", "0X1a", [0, 16]), 26);
  assert.equal(cf("int", "0x1a", [7, 10]), 7,
    "prefix with wrong base falls to the default (no raise)");
  assert.equal(cf("int", "0x1a", [10, 0]), 26, "base 0 infers from prefix");
  assert.equal(cf("int", "3.9"), 3, "float string via fallback");
  assert.equal(cf("int", 25.75), 25);
  assert.equal(cf("int", true), 1);
  assert.equal(cf("int", false), 0);
  assert.equal(cf("int", "nope"), 0, "default on garbage");
  assert.equal(cf("int", "nope", [7]), 7, "custom default");
  expectRaises(() => cf("int", new V.Undefined(null, V.missing, "u")),
    "'u' is undefined");
  // documented deviation: CPython int() accepts Nd-script digits via
  // str.isdigit(); the E0 bridge is ASCII-only and falls to the default.
  assert.equal(cf("int", "١٢"), 0, "non-ASCII digits -> default (caveat)");
});

test("float() edges", () => {
  assert.equal(cf("float", "3.5"), 3.5);
  assert.equal(cf("float", true), 1);
  assert.equal(cf("float", "inf"), Infinity);
  assert.ok(Number.isNaN(cf("float", "nan")));
  assert.equal(cf("float", "x"), 0);
  assert.equal(cf("float", "x", [2.5]), 2.5, "custom default");
  expectRaises(() => cf("float", new V.Undefined(null, V.missing, "u")),
    "'u' is undefined");
});

test("string filters edges", () => {
  assert.equal(cf("upper", "a-Ä"), "A-Ä");
  assert.equal(cf("capitalize", ""), "");
  assert.equal(cf("capitalize", "1test"), "1test",
    "leading digit is unchanged; remaining letters lower-case");
  assert.equal(cf("title", "a-b cd"), "A-B Cd");
  assert.equal(cf("title", "x<y>z"), "X<Y>z",
    "'<' splits words, '>' does not (python split pattern)");
  assert.equal(cf("trim", "\t hi \n"), "hi");
  assert.equal(cf("trim", "ababa", ["ab"]), "", "chars strip both ends");
});

test("replace edges", () => {
  assert.equal(cf("replace", "aaa", ["a", "bb"]), "bbbbbb");
  assert.equal(cf("replace", "aaaa", ["a", "b", 2]), "bbaa");
  // empty needle with count is the documented unsupported corner
  expectRaises(() => cf("replace", "ab", ["", "x", 1]));
  // autoescape branch: replacement gets escaped into markup output
  assert.equal(autoenv.callFilter("replace", "aaa", ["a", "<x>"]), "<x><x><x>");
});

test("sequence filters edges", () => {
  assert.equal(cf("first", new Set([1, 2])), 1);
  expectRaises(() => cf("last", new Set([1, 2])),
    "'set' object is not reversible");
  // python reversed(dict) yields a reverse KEY iterator
  assert.deepEqual(cf("reverse", new Map([["a", 1], ["b", 2]])), ["b", "a"]);
  assert.deepEqual(cf("reverse", { a: 1 }), ["a"]);
  expectRaises(() => cf("reverse", null), "argument must be iterable");
  assert.deepEqual(cf("list", new Set([1, 2])), [1, 2]);
});

test("length edges + messages", () => {
  assert.equal(cf("count", [1]), 1);
  expectRaises(() => cf("length", null),
    "object of type 'NoneType' has no len()");
  expectRaises(() => cf("length", true),
    "object of type 'bool' has no len()");
});

test("abs edges + messages", () => {
  assert.equal(cf("abs", -7), 7);
  assert.equal(cf("abs", true), 1);
  expectRaises(() => cf("abs", "x"),
    "bad operand type for abs(): 'str'");
});

test("default edges", () => {
  assert.equal(cf("default", "", ["d"]), "");
  assert.equal(cf("default", "", ["d", true]), "d");
  assert.equal(cf("default", 0, ["d", true]), "d");
  assert.equal(cf("default", [], ["d", true]), "d");
  assert.equal(cf("default", new V.Undefined(null, V.missing, "u"), ["d"]), "d");
});

test("sort edges", () => {
  // stable: equal keys keep incoming order
  assert.deepEqual(cf("sort", [[1, "b1"], [0, "x"], [0, "y"]]),
    [[0, "x"], [0, "y"], [1, "b1"]]);
  // dotted attribute paths with digit parts as indexes
  assert.deepEqual(cf("sort", [{ a: [{ b: 2 }] }, { a: [{ b: 1 }] }],
    [false, false, "a.0.b"]),
    [{ a: [{ b: 1 }] }, { a: [{ b: 2 }] }]);
  // multi-key paths
  assert.deepEqual(cf("sort", [{ p: 1, q: 2 }, { p: 1, q: 1 }],
    [false, false, "p,q"]),
    [{ p: 1, q: 1 }, { p: 1, q: 2 }]);
  // incomparable values abort the sort; CPython's list.sort compares the
  // incoming element against earlier ones so operands name in that order
  // (golden-pinned as sort-mixed).
  expectRaises(() => cf("sort", ["a", 1]),
    "'<' not supported between instances of 'int' and 'str'");
});

test("join edges", () => {
  // dict join concatenates KEYS (python iteration rule)
  assert.equal(cf("join", { a: 1, b: 2 }, [", "]), "a, b");
  assert.equal(cf("join", new Map([["k1", "v"]]), ["-"]), "k1");
  // Undefined iterates empty (joins to ""); None raises like CPython
  assert.equal(cf("join", new V.Undefined(null, V.missing, "u"), []), "");
  expectRaises(() => cf("join", null, []),
    "'NoneType' object is not iterable");
  // attribute form through getitem preference
  assert.equal(cf("join", [{ n: 1 }, { n: 2 }], ["-", "n"]), "1-2");
});

test("map/select need context (R1 declines)", () => {
  expectRaises(() => cf("map", [1], ["string"]),
    "Attempted to invoke a context filter without context.");
  // with a context object carrying environment, they work
  const ctx = { environment: env };
  assert.deepEqual(env.callFilter("map", ["a", "b"], ["upper"], ctx),
    ["A", "B"]);
});
