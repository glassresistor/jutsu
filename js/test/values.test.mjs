// Value-bridge rule suites (PLAN_JS.md slice E0): every named JS rule that
// replaces a CPython behavior, pinned with direct tests. Golden replays
// cover the python-captured shapes; these pin the bridge internals.

import test from "node:test";
import assert from "node:assert/strict";
import * as V from "../src/index.mjs";

test("truthy rules", () => {
  assert.equal(V.truthy(null), false);
  assert.equal(V.truthy(false), false);
  assert.equal(V.truthy(0), false);
  assert.equal(V.truthy(0n), false);
  assert.equal(V.truthy(""), false);
  assert.equal(V.truthy([]), false);
  assert.equal(V.truthy(new Map()), false);
  assert.equal(V.truthy(new Set()), false);
  assert.equal(V.truthy({}), false);
  assert.equal(V.truthy({ a: 1 }), true);
  assert.equal(V.truthy([0]), true);
  assert.equal(V.truthy("0"), true, "non-empty string is truthy in python");
  assert.equal(V.truthy(NaN), true, "CPython bool(nan) is True");
  assert.equal(V.truthy(new V.Undefined(null, V.missing, "u")), false);
});

test("pyStr rules", () => {
  assert.equal(V.pyStr(null), "None");
  assert.equal(V.pyStr(true), "True");
  assert.equal(V.pyStr(false), "False");
  assert.equal(V.pyStr(3), "3");
  assert.equal(V.pyStr(4.5), "4.5");
  assert.equal(V.pyStr("hi"), "hi");
  assert.equal(V.pyStr([1, "x"]), "[1, 'x']");
  assert.equal(V.pyStr([[1, 2], 3]), "[[1, 2], 3]");
  assert.equal(V.pyStr(new Map([["a", 1]])), "{'a': 1}");
  assert.equal(V.pyStr({ a: "x" }), "{'a': 'x'}");
  assert.equal(V.pyStr(new Set([1, 2])), "{1, 2}");
  assert.equal(V.pyStr(new V.Undefined(null, V.missing, "u")), "");
  const m = new V.Markup("<i&gt;&amp;lt");
  assert.equal(V.pyStr(m), "<i&gt;&amp;lt"); // Markup prints its raw text
});

test("objectTypeRepr / pyTypeName shapes", () => {
  assert.equal(V.objectTypeRepr(null), "None");
  assert.equal(V.objectTypeRepr(1), "int object");
  assert.equal(V.objectTypeRepr(1.5), "float object");
  assert.equal(V.objectTypeRepr(true), "bool object");
  assert.equal(V.objectTypeRepr("x"), "str object");
  assert.equal(V.objectTypeRepr([1]), "list object");
  assert.equal(V.objectTypeRepr(new Map()), "dict object");
  assert.equal(V.objectTypeRepr(new Set()), "set object");
  assert.equal(V.objectTypeRepr({}), "dict object");
  class Thing {}
  assert.equal(V.objectTypeRepr(new Thing()), "Thing object");
  // error-message type names (distinct from the repr form)
  assert.equal(V.pyTypeName(null), "NoneType");
  assert.equal(V.pyTypeName(3.5), "float");
  assert.equal(V.pyTypeName("x"), "str");
});

test("Undefined message forms", () => {
  const u = new V.Undefined(null, V.missing, "foo");
  assert.equal(u._undefinedMessage, "'foo' is undefined");
  const h = new V.Undefined("some hint");
  assert.equal(h._undefinedMessage, "some hint");
  const obj = { a: 1 };
  const ua = new V.Undefined(null, obj, "b");
  assert.equal(ua._undefinedMessage, "'dict object' has no attribute 'b'");
  const arr = [1];
  const ue = new V.Undefined(null, arr, 0);
  assert.equal(ue._undefinedMessage, "list object has no element 0");
});

test("Undefined behaviors by class", () => {
  const base = new V.Undefined(null, V.missing, "u");
  assert.equal(V.pyStr(base), "");
  assert.equal(V.pyLen(base), 0);
  assert.deepEqual(V.pyIter(base), []);
  assert.throws(() => base.add(), V.UndefinedError);
  assert.throws(() => base._getattr("x"), V.UndefinedError);

  const strict = new V.StrictUndefined(null, V.missing, "u");
  assert.throws(() => V.pyStr(strict), V.UndefinedError);
  assert.throws(() => V.truthy(strict), V.UndefinedError);
  assert.throws(() => V.pyLen(strict), V.UndefinedError);

  const debug = new V.DebugUndefined(null, V.missing, "u");
  assert.equal(V.pyStr(debug), "{{ u }}");
  const debugHint = new V.DebugUndefined("hmm");
  assert.equal(V.pyStr(debugHint), "{{ undefined value printed: hmm }}");

  const chain = new V.ChainableUndefined(null, V.missing, "u");
  // chained lookups return self and still print empty
  assert.ok(chain._getattr("a") === chain);
  assert.ok(chain._getitem(0) === chain);
  assert.equal(V.pyStr(chain), "");
  assert.throws(() => chain.add(), V.UndefinedError);
});

test("Undefined equality is same-class only", () => {
  const a = new V.Undefined(null, V.missing, "a");
  const b = new V.Undefined(null, V.missing, "b");
  assert.equal(V.pyEq(a, b), true, "same class compares equal");
  const s = new V.StrictUndefined(null, V.missing, "s");
  assert.equal(V.pyEq(a, s), false);
  assert.throws(() => V.pyEq(s, a), V.UndefinedError, "strict raises on compare");
});

test("pyEq rules", () => {
  assert.equal(V.pyEq(true, 1), true, "bools are numbers for ==");
  assert.equal(V.pyEq(false, 0), true);
  assert.equal(V.pyEq(1, 2), false);
  assert.equal(V.pyEq("a", 1), false, "cross-type is unequal, not an error");
  assert.equal(V.pyEq(null, null), true);
  assert.equal(V.pyEq(null, 0), false);
  assert.equal(V.pyEq([1, [2]], [1, [2]]), true);
  assert.equal(V.pyEq([1], "x"), false);
  assert.equal(V.pyEq(new Map([["a", 1]]), { a: 1 }), true, "dict-like cross compare");
  assert.equal(V.pyEq({ a: NaN }, { a: NaN }), false,
    "CPython: nan != nan even nested");
  const mk = new V.Markup("abc");
  assert.equal(V.pyEq(mk, "abc"), true, "Markup equals its text twin");
});

test("pyCompare rules", () => {
  assert.equal(V.pyCompare(1, 2), -1);
  assert.equal(V.pyCompare(true, 1), 0, "True compares as int 1");
  assert.equal(V.pyCompare("abc", "abd"), -1);
  assert.equal(V.pyCompare([1, 2], [1, 3]), -1, "list lexicographic");
  assert.equal(V.pyCompare([1, 2], [1]), 1, "prefix is smaller");
  const u = new V.Undefined(null, V.missing, "u");
  assert.throws(() => V.pyCompare(u, 1), V.UndefinedError);
  assert.throws(() => V.pyCompare("a", 1), (e) => {
    return e.message ===
      "'<' not supported between instances of 'str' and 'int'";
  });
  // NaN pairs are unordered, not raising
  const n = V.pyCompare(NaN, 1);
  assert.equal(n, V.UNORDERED);
});

test("pyContains rules", () => {
  assert.equal(V.pyContains([1, 2], 2), true);
  assert.equal(V.pyContains("abc", "bc"), true);
  assert.throws(() => V.pyContains("abc", 1), (e) =>
    e.message === "'in <string>' requires string as left operand, not int");
  assert.equal(V.pyContains(new Map([["k", 9]]), "k"), true);
  assert.equal(V.pyContains({ k: 9 }, "k"), true);
  assert.equal(V.pyContains({ k: 9 }, "j"), false);
  assert.equal(V.pyContains(new Set([1]), 1), true);
});

test("pyLen / pyIter rules", () => {
  assert.equal(V.pyLen("abcd"), 4);
  assert.equal(V.pyLen(new Map([["a", 1]])), 1);
  assert.throws(() => V.pyLen(5), (e) =>
    e.message === "object of type 'int' has no len()");
  // dicts iterate keys
  assert.deepEqual(V.pyIter({ a: 1, b: 2 }), ["a", "b"]);
  assert.deepEqual(V.pyIter(new Map([["z", 1]])), ["z"]);
  assert.throws(() => V.pyIter(null),
    (e) => e.message === "'NoneType' object is not iterable");
});

test("escape / Markup protocol", () => {
  assert.equal(V.escapeAny("<a&>").toString(), "&lt;a&amp;&gt;");
  assert.equal(V.escapeSilent(null).toString(), "");
  assert.equal(V.escapeAny(null).toString(), "None");
  const trusted = { __html__() { return "RAW"; } };
  assert.equal(V.escapeAny(trusted).toString(), "RAW");
  const m = new V.Markup("-");
  assert.ok(m instanceof String);
  assert.ok(new V.Markup("x") instanceof V.Markup);
  // join escapes non-html elements, trusts __html__ ones
  const joined = new V.Markup(", ").join(["<a>", trusted]);
  assert.equal(joined.toString(), "&lt;a&gt;, RAW");
  assert.ok(joined instanceof V.Markup);
  assert.equal(V.softStr(new V.Markup("t")).toString(), "t");
});

test("missing sentinel is distinct from null", () => {
  assert.notEqual(V.missing, null);
  assert.notEqual(V.missing, undefined);
});
