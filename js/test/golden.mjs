// Golden-value harness (PLAN_JS.md slice E0, decision D-E1): reads
// js/test/golden.json produced by scripts/js_golden.py from CPython/jutsu.
// Suites skip cleanly when the file is absent (node-only environments).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import * as V from "../src/index.mjs";

const GOLDEN_PATH = fileURLToPath(new URL("./golden.json", import.meta.url));

export function loadGolden() {
  try {
    return JSON.parse(readFileSync(GOLDEN_PATH, "utf-8"));
  } catch {
    return null; // python not run here -- skip golden comparisons
  }
}

/** Decode a golden-encoded input value into the JS bridge representation. */
export function decode(e) {
  if (e === null || typeof e !== "object") return e;
  if ("none" in e) return null;
  if ("bool" in e) return e.bool;
  if ("num" in e) {
    const [v] = e.num;
    if (typeof v === "string") {
      if (v === "nan") return NaN;
      if (v === "inf") return Infinity;
      if (v === "-inf") return -Infinity;
    }
    return v;
  }
  if ("str" in e) return e.str;
  if ("markup" in e) return new V.Markup(e.markup);
  if ("undefined" in e) return new V.Undefined(e.undefined.message);
  if ("list" in e) return e.list.map(decode);
  if ("dict" in e)
    return new Map(e.dict.map(([k, v]) => [decode(k), decode(v)]));
  throw new Error(`cannot decode golden value: ${JSON.stringify(e).slice(0, 60)}`);
}

/** Encode a JS bridge result back into the golden JSON form. */
export function encode(v) {
  if (v instanceof V.Undefined) {
    let str;
    try { str = V.pyStr(v); } catch { str = null; }
    return { undefined: { message: v._undefinedMessage, str } };
  }
  if (v === null || v === undefined) return { none: true };
  if (typeof v === "boolean") return { bool: v };
  if (typeof v === "bigint") return { num: [Number(v), "int"] };
  if (typeof v === "number") {
    if (Number.isInteger(v)) return { num: [v, "int"] };
    return { num: [numText(v), "float"] };
  }
  if (typeof v === "string") return { str: v };
  if (v instanceof V.Markup) return { markup: v.toString() };
  if (Array.isArray(v)) return { list: v.map(encode) };
  if (v instanceof Map) {
    return { dict: [...v].map(([k, val]) => [encode(k), encode(val)]) };
  }
  if (Object.getPrototypeOf(v) === Object.prototype ||
      Object.getPrototypeOf(v) === null) {
    return {
      dict: Object.entries(v).map(
        ([k, val]) => [encode(k), encode(val)]),
    };
  }
  throw new Error(`cannot encode JS result: ${typeof v}`);
}

function numText(v) {
  if (Number.isNaN(v)) return "nan";
  if (v === Infinity) return "inf";
  if (v === -Infinity) return "-inf";
  return v;
}

/** Deep equality over plain JSON structures (NaN-aware). */
export function jsonEq(a, b) {
  if (Number.isNaN(a) && Number.isNaN(b)) return true;
  if (a === b) return true;
  if (typeof a !== typeof b || a === null || b === null) return false;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((x, i) => jsonEq(x, b[i]));
  }
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (typeof a === "object") {
    const ka = Object.keys(a), kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => jsonEq(a[k], b[k]));
  }
  return false;
}

/** Assert a replayed bridge call against one golden case expectation.
 * Returns true when the outcome matches; records the mismatch otherwise.
 * Suites assert on the returned boolean so failures name the case. */
export function expectOutcome(t, c, thunk) {
  if ("value" in c) {
    let got;
    try {
      got = encode(thunk());
    } catch (e) {
      return fail(c, `expected value ${JSON.stringify(c.value)} but threw '${e.message}'`);
    }
    if (!jsonEq(got, c.value)) {
      return fail(c, `got ${JSON.stringify(got)} != golden ${JSON.stringify(c.value)}`);
    }
    return true;
  }
  const [pyType, message] = c.error;
  let thrown = null;
  try { thunk(); } catch (e) { thrown = e; }
  if (!thrown) {
    return fail(c, `expected error ${JSON.stringify(c.error)} but no throw`);
  }
  if (!(thrown instanceof V.TemplateError)) {
    return fail(c, `threw non-template error: ${thrown}`);
  }
  if (pyType === "UndefinedError" && !(thrown instanceof V.UndefinedError)) {
    return fail(c, `expected UndefinedError, got ${thrown.constructor.name}`);
  }
  if (thrown.message !== message) {
    return fail(c, `got message '${thrown.message}' != golden '${message}'`);
  }
  return true;
}

function fail(c, why) {
  const err = new Error(`golden ${c.id}: ${why}`);
  throw err;
}
