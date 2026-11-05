// Tests registry for the JavaScript wrapper of jutsu (PLAN_JS.md slice E0).
// Mirrors src/jutsu/tests.py for the ported subset; unported semantics are
// absent, not approximated (decision D-E4). Type-category tests follow the
// documented JS<->python type mapping in values.mjs.

import {
  truthy, pyStr, pyLen, pyIter, pyCompare, UNORDERED, pyContains, pyEq,
  Undefined, StrictUndefined, TemplateError,
} from "./values.mjs";

function strValue(v) {
  if (v instanceof Undefined) return v.toString(); // strict raises
  return pyStr(v);
}

/** python str.islower(): all cased characters lowercase and at least one
 *  cased character present. ASCII fast path; Unicode cased sets are a
 *  documented caveat until the golden suites widen. */
function isLowerLike(s) {
  let cased = false;
  for (const ch of s) {
    if (/[a-zA-Z]/.test(ch)) {
      cased = true;
      if (ch !== ch.toLowerCase()) return false;
    } else if (/\p{Lu}|\p{Ll}/u.test(ch)) {
      // non-ASCII letter: compare case mappings (BMP approximation)
      cased = true;
      if (ch !== ch.toLowerCase()) return false;
    }
  }
  return cased;
}
function isUpperLike(s) {
  let cased = false;
  for (const ch of s) {
    if (/[a-zA-Z]/.test(ch)) {
      cased = true;
      if (ch !== ch.toUpperCase()) return false;
    } else if (/\p{Lu}|\p{Ll}/u.test(ch)) {
      cased = true;
      if (ch !== ch.toUpperCase()) return false;
    }
  }
  return cased;
}

function numMod(v, m) {
  // python % semantics: result carries the divisor's sign family
  // (-3 % 2 == 1). Undefined fails through (% is a failing dunder slot).
  if (v instanceof Undefined) v._fail();
  if (typeof v === "boolean") return (v ? 1 : 0) % m;
  if (typeof v !== "number" && typeof v !== "bigint")
    throw new TemplateError(
      "'%' not supported between instances of a non-number and a number");
  const r = ((Number(v) % m) + Math.abs(m)) % Math.abs(m);
  return r;
}

export const TESTS = {
  // value tests
  "odd": { func: (v) => numMod(v, 2) === 1 },
  "even": { func: (v) => numMod(v, 2) === 0 },
  "divisibleby": { func: (v, num) => numMod(v, num) === 0 },
  "defined": { func: (v) => !(v instanceof Undefined) },
  "undefined": { func: (v) => v instanceof Undefined },
  "filter": { func: (env, v) => env.hasFilter(v), pass: "env" },
  "test": { func: (env, v) => env.hasTest(v), pass: "env" },
  "none": { func: (v) => v === null },
  "boolean": { func: (v) => typeof v === "boolean" },
  "false": { func: (v) => v === false },
  "true": { func: (v) => v === true },
  // integer/float approximate via Number.isInteger -- JS cannot tell 1 from
  // 1.0; python's bool exclusion is honored (bools are not integers here).
  "integer": { func: (v) => typeof v === "bigint" ||
    (typeof v === "number" && Number.isInteger(v)) },
  "float": { func: (v) => typeof v === "number" && !Number.isInteger(v) },
  "lower": { func: (v) => isLowerLike(strValue(v)) },
  "upper": { func: (v) => isUpperLike(strValue(v)) },
  "string": { func: (v) => typeof v === "string" },
  // mapping/sequence/iterable follow the type mapping: dict-like are Maps and
  // plain objects; sequence-ish means sized AND item-accessible (python's
  // len + __getitem__ probe -- dicts qualify, numbers do not); iterable-ish
  // means pyIter succeeds (dicts iterate keys, Undefined iterates empty).
  "mapping": { func: (v) => v instanceof Map || isPlain(v) },
  "number": { func: (v) => typeof v === "number" || typeof v === "bigint" ||
    typeof v === "boolean" },
  "sequence": { func: (v) => {
    if (v instanceof Undefined) return true; // has __len__ and __getitem__
    try { pyLen(v); } catch { return false; }
    return sequenceAccessible(v);
  } },
  "iterable": { func: (v) => {
    try { pyIter(v); return true; } catch { return false; }
  } },
  // sameas is identity (`is`): exact object identity in JS. Caveat recorded:
  // CPython caches small integers so `1 is 1` holds there but is a value
  // comparison question in JS only for objects/booleans/null.
  "sameas": { func: (v, other) => v === other },
  "escaped": { func: (v) => v !== null && v !== undefined &&
    typeof v.__html__ === "function" },
  "in": { func: (v, seq) => pyContains(seq, v) },
  // comparison operators (operator.eq ... in TESTS): == is always defined,
  // ordering raises on incomparable pairs via pyCompare.
  "==": { func: (a, b) => pyEq(a, b) },
  "eq": { func: (a, b) => pyEq(a, b) },
  "equalto": { func: (a, b) => pyEq(a, b) },
  "!=": { func: (a, b) => !pyEq(a, b) },
  "ne": { func: (a, b) => !pyEq(a, b) },
  // Ordering tests name the actual operator in their failure text (golden
  // pinned as gt-str-int / ge-str-int).
  ">": { func: (a, b) => ordGt(pyCompare(a, b, GT)) },
  "gt": { func: (a, b) => ordGt(pyCompare(a, b, GT)) },
  "greaterthan": { func: (a, b) => ordGt(pyCompare(a, b, GT)) },
  ">=": { func: (a, b) => ordGe(pyCompare(a, b, GTE)) },
  "ge": { func: (a, b) => ordGe(pyCompare(a, b, GTE)) },
  "<": { func: (a, b) => ordLt(pyCompare(a, b, LT)) },
  "lt": { func: (a, b) => ordLt(pyCompare(a, b, LT)) },
  "lessthan": { func: (a, b) => ordLt(pyCompare(a, b, LT)) },
  "<=": { func: (a, b) => ordLe(pyCompare(a, b, LTE)) },
  "le": { func: (a, b) => ordLe(pyCompare(a, b, LTE)) },
};

const GT = ">", GTE = ">=", LT = "<", LTE = "<=";

function ordGt(c) { return c === UNORDERED ? false : c > 0; }
function ordGe(c) { return c === UNORDERED ? false : c >= 0; }
function ordLt(c) { return c === UNORDERED ? false : c < 0; }
function ordLe(c) { return c === UNORDERED ? false : c <= 0; }

function isPlain(v) {
  if (typeof v !== "object" || v === null) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

function sequenceAccessible(v) {
  // python probes `value.__getitem__` existence only; the JS analogue is
  // indexable-by-position (array/string) or dict-like (Map/plain object),
  // everything else sized-but-not-indexable still counts True because the
  // probe never calls it. Numbers failed pyLen above already.
  return true;
}

export const ABSENT_TESTS = ["callable"];
