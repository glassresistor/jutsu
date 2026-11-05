// Filters subset for the JavaScript wrapper of jutsu (PLAN_JS.md slice E0).
//
// Ported filters mirror src/jutsu/filters.py case-by-case; anything not
// ported is ABSENT from the registry, never approximated (decision D-E4).
// Aliases (`d`, `e`, `count`) are registered per the FILTERS dict there.
//
// Calling convention replacing Python's @pass_* decorators: registry entries
// carry `pass: "env" | "evalCtx" | "ctx"` when they need a first argument
// injected. Environment.callFilter performs the insertion the same way
// Environment._filter_test_common does in Python (PLAN_JS.md D-E1).

import {
  truthy, pyStr, pyLen, pyIter, pyCompare, UNORDERED,
  Undefined, StrictUndefined, Markup, escapeAny, softStr,
  TemplateError, FilterNotFound, UndefinedError,
} from "./values.mjs";

// ---------------------------------------------------------------- int rules

/** Python `int(value, base)` for strings. Named JS rules:
 *  - surrounding whitespace tolerated (CPython int(' 12 ') == 12);
 *  - underscores allowed *between* digits only, never leading/trailing or
 *    doubled (`'1_2'` -> 12, `'1__2'` -> invalid);
 *  - base prefixes ('0x'/'0o'/'0b') accepted for base 0 and the matching
 *    explicit base; '0x1a' with base 10 is invalid (CPython agrees);
 *  - returns null on failure so callers can fall back like do_int does.
 */
function pyIntStr(s, base) {
  if (base !== 0 && (base < 2 || base > 36))
    throw new TemplateError("int() base must be >= 2 and <= 36, or 0");
  let t = s.trim();
  let sign = 1;
  if (t[0] === "+" || t[0] === "-") { if (t[0] === "-") sign = -1; t = t.slice(1); }
  let radix = base === 0 ? 10 : base;
  if (t.length > 1 && t[0] === "0" && /[xob]/i.test(t[1])) {
    const wanted = { x: 16, o: 8, b: 2 };
    const matched = wanted[t[1].toLowerCase()];
    if (base !== 0 && base !== matched) return null;
    radix = base === 0 ? matched : base;
    t = t.slice(2);
  }
  const digits = t;
  if (digits === "") return null;
  if (/^_|_$/.test(digits) || /__/.test(digits)) return null;
  const clean = digits.replaceAll("_", "");
  if (clean === "") return null;
  // digit-by-digit validation: parseInt is too lenient with mixed case/base
  let value = 0n, big = BigInt(radix);
  for (const c of clean) {
    let d;
    if (c >= "0" && c <= "9") d = c.charCodeAt(0) - 48;
    else if (c >= "a" && c <= "z") d = c.charCodeAt(0) - 97 + 10;
    else if (c >= "A" && c <= "Z") d = c.charCodeAt(0) - 65 + 10;
    else return null;
    if (d >= radix) return null;
    value = value * big + BigInt(d);
  }
  // NB: sign is a Number; multiply via unary minus on the BigInt instead
  // (Number * BigInt throws and would silently hit do_int's fallback).
  const signed = sign === -1 ? -value : value;
  const num = Number(signed);
  return Number.isSafeInteger(num) ? num : signed;
}

/** Python int(value) for non-strings: truncate toward zero; bools are 1/0;
 *  Undefined fails through -- do_int's except clauses never catch it.
 *  NaN/inf raise so that do_int's nested float fallback also fails and the
 *  caller returns its default (CPython int(nan) -> ValueError). */
function pyIntNum(v) {
  if (v instanceof Undefined) v._fail(); // UndefinedError, not caught
  if (typeof v === "boolean") return v ? 1 : 0;
  if (typeof v === "bigint") return Number(v);
  if (typeof v === "number") {
    if (!Number.isFinite(v))
      throw new TemplateError("cannot convert float NaN to integer");
    return Math.trunc(v);
  }
  if (v === null)
    throw new TemplateError(
      "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'");
  throw new TemplateError(
    `int() argument must be a string, a bytes-like object or a real number, not '${typeof v}'`);
}

/** Python float(str): whitespace tolerant, underscores between digits only,
 *  inf/nan accepted. Returns null on failure (JS-side rule). */
function pyFloatStr(s) {
  const t = s.trim();
  if (t === "") return null;
  if (/^(nan|inf|[+-]inf)$/.test(t.toLowerCase())) {
    if (/^nan$/.test(t.toLowerCase())) return NaN;
    return /^[-]/.test(t) ? -Infinity : Infinity;
  }
  if (/_/.test(t)) {
    // underscores: between digits/digits after the decimal point only
    const core = /^[+-]?(\d+)(\.\d*)?([eE][+-]?\d+)?$/.test(t.replaceAll("_", ""));
    if (!core || /^_|_$|[+-]_|\._|_\./.test(t)) return null;
    if (/__/.test(t)) return null;
  } else {
    if (!/^[+-]?(?:\d+(?:\.\d*|\.\d+)|\d+|\.\d+)(?:[eE][+-]?\d+)?$/.test(t))
      return null;
  }
  const n = Number(t.replaceAll("_", ""));
  return n;
}

function pyFloatAny(v) {
  if (v instanceof Undefined) v._fail(); // do_float does not catch this
  if (typeof v === "boolean") return v ? 1.0 : 0.0;
  if (typeof v === "bigint") return Number(v);
  if (typeof v === "number") return v;
  if (typeof v === "string") {
    const n = pyFloatStr(v);
    if (n === null) throw new TemplateError("could not convert string to float");
    return n;
  }
  throw new TemplateError("float() argument must be a string or a number, not 'NoneType'");
}

// ------------------------------------------------------------- str() bridge

/** The soft_str()/str() conversion filters run before touching strings. */
function toStrForFilter(v) {
  if (v instanceof Undefined) return pyStr(v); // base "", debug '{{ }}', strict raises
  if (typeof v === "string") return v;
  if (v instanceof Markup) return v.toString();
  return softStr(v).toString();
}

// --------------------------------------------------------- simple filters

function do_upper(v) { return toStrForFilter(v).toUpperCase(); }
function do_lower(v) { return toStrForFilter(v).toLowerCase(); }

function do_capitalize(v) {
  const s = toStrForFilter(v);
  if (s.length === 0) return s;
  // python capitalize() upper-cases char 1 and LOWER-CASES the rest.
  return s[0].toUpperCase() + s.slice(1).toLowerCase();
}

// python: re.split(r"([-\s({\[<]+)", ...) -- captured delimiters stay in the
// token stream; JS split with a capture group behaves identically.
const TITLE_SPLIT_RE = /([-\s({[\[<]+)/g;

function do_title(v) {
  const s = toStrForFilter(v);
  const items = s.split(TITLE_SPLIT_RE).filter((x) => x !== "" && x !== undefined);
  return items.map((item) => item[0].toUpperCase() + item.slice(1).toLowerCase())
    .join("");
}

function trimWs(s) {
  // python str.strip(): whitespace per str.isspace(); JS \s covers the ASCII
  // and BMP common cases -- exotic space code points are a documented caveat.
  return s.replace(/^\s+/, "").replace(/\s+$/, "");
}

function do_trim(v, chars = null) {
  const s = toStrForFilter(v);
  if (chars === null || chars === undefined) return trimWs(s);
  const set = new Set([...String(chars)]);
  let a = 0, b = s.length;
  while (a < b && set.has(s[a])) a++;
  while (b > a && set.has(s[b - 1])) b--;
  return s.slice(a, b);
}

function hasHtml(v) { return !!(v !== null && v !== undefined && typeof v.__html__ === "function"); }

function replaceLiteral(s, old, rep, count) {
  if (old === "") {
    // python str.replace("", x, n) inserts x at every gap; E0 keeps the
    // full-insert behavior when count is -1 and declines otherwise.
    if (count === -1) return rep + [...s].join(rep) + rep;
    throw new TemplateError("replace with empty needle and count is not supported");
  }
  const out = [];
  let i = 0, n = 0;
  for (;;) {
    const j = s.indexOf(old, i);
    if (j < 0 || (count >= 0 && n >= count)) break;
    out.push(s.slice(i, j)); out.push(rep);
    i = j + old.length; n++;
  }
  out.push(s.slice(i));
  return out.join("");
}

function do_replace(evalCtx, v, old, repl, count = null) {
  if (count === undefined || count === null) count = -1;
  if (!evalCtx.autoescape) {
    return replaceLiteral(pyStr(v), pyStr(old), pyStr(repl), count);
  }
  // autoescape branch (filters.py do_replace): escape/markup rules
  const oldHtml = hasHtml(old), newHtml = hasHtml(repl), sHtml = hasHtml(v);
  let s, o, r;
  if ((oldHtml || (newHtml && !sHtml))) {
    s = escapeAny(v).toString();
  } else {
    s = toStrForFilter(v);
  }
  o = oldHtml ? String(old) : pyStr(old);
  r = newHtml ? String(repl) : pyStr(repl);
  return replaceLiteral(String(s), String(o), String(r), count);
}

// --------------------------------------------------------- sequence filters

function do_length(v) { return pyLen(v); }

function do_abs(v) {
  if (v instanceof Undefined)
    throw new TemplateError("bad operand type for abs(): 'Undefined'");
  if (typeof v === "boolean") return v ? 1 : 0;
  if (typeof v === "bigint") return v < 0n ? -v : v;
  if (typeof v === "number") return Math.abs(v);
  throw new TemplateError(
    `bad operand type for abs(): '${typeof v === "string" ? "str" : typeof v}'`);
}

function do_first(env, seq) {
  const arr = pyIter(seq);
  if (arr.length === 0)
    return env.undefined("No first item, sequence was empty.");
  return arr[0];
}

function do_last(env, seq) {
  // python reversed() only accepts reversible collections; Set is not --
  // the TypeError propagates out of do_last (filters.py has no guard).
  if (seq instanceof Set)
    throw new TemplateError("'set' object is not reversible");
  const arr = pyIter(seq);
  if (arr.length === 0)
    return env.undefined("No last item, sequence was empty.");
  return arr[arr.length - 1];
}

function do_reverse(v) {
  if (typeof v === "string") return [...v].reverse().join("");
  if (Array.isArray(v)) return v.slice().reverse();
  // python reversed(dict/Map) yields a reverse KEY iterator (values are
  // not part of the result), so dict-like reverses to a keys array.
  if (v instanceof Map) return [...v.keys()].reverse();
  if (v instanceof Set) {
    // python: reversed(set) raises, then the list() fallback reverses a copy
    const arr = [...v]; arr.reverse(); return arr;
  }
  if (isPlainObj(v)) return Object.keys(v).reverse(); // dict analogue

  if (typeof v === "object" && v !== null && typeof v[Symbol.iterator] === "function") {
    const arr = [...v]; arr.reverse(); return arr;
  }
  throw new TemplateError("argument must be iterable");
}

function do_list(v) { return pyIter(v); }

// ---------------------------------------------------------------- default

function do_default(value, defaultValue = "", boolean = false) {
  if (value instanceof Undefined || (boolean && !truthy(value)))
    return defaultValue;
  return value;
}

// ------------------------------------------------------------- int / float

function do_int(value, defaultValue = 0, base = 10) {
  try {
    if (typeof value === "string") {
      const n = pyIntStr(value, base);
      if (n === null) throw new TemplateError("invalid literal for int()");
      return n;
    }
    return pyIntNum(value);
  } catch (e) {
    if (e instanceof UndefinedError) throw e;
    // do_int quirk: "42.23"|int -> 42 via the nested float() fallback;
    // int(float(nan)) itself raises (ValueError/OverflowError) so NaN/inf
    // end in the default, CPython-faithful.
    try {
      const f = pyFloatAny(value);
      if (!Number.isFinite(f)) throw new TemplateError("cannot convert");
      return Math.trunc(f);
    } catch {
      return defaultValue;
    }
  }
}

function do_float(value, defaultValue = 0.0) {
  try {
    return pyFloatAny(value);
  } catch (e) {
    if (e instanceof UndefinedError) throw e;
    return defaultValue;
  }
}

// ------------------------------------------------------------------- join

function attrParts(attr) {
  // filters.py _prepare_attribute_parts: dotted paths with digit parts as ints
  if (attr === null || attr === undefined) return [];
  if (typeof attr === "number" || typeof attr === "bigint") return [attr];
  if (typeof attr !== "string") return [attr];
  return attr.split(".").map((x) => /^\d+$/.test(x) ? parseInt(x, 10) : x);
}

/** make_attrgetter equivalent; lookup goes through env.getitem so dict vs
 *  attribute preference stays in one place (environment.mjs). */
function makeAttrgetter(env, attribute, postprocess = null, deflt = null) {
  const parts = attrParts(attribute);
  return function (item) {
    for (const part of parts) {
      item = env.getitem(item, part);
      if (deflt !== null && deflt !== undefined && item instanceof Undefined)
        item = deflt;
    }
    if (postprocess !== null) item = postprocess(item);
    return item;
  };
}

/** make_multi_attrgetter equivalent: comma separated paths, always returns
 *  a list key (python compares these lexicographically). */
function makeMultiAttrgetter(env, attribute, postprocess) {
  let splitList;
  if (typeof attribute === "string") splitList = attribute.split(",");
  else splitList = [attribute];
  const partsList = splitList.map(attrParts);
  return function (item) {
    const out = [];
    for (const parts of partsList) {
      let itemI = item;
      for (const part of parts) itemI = env.getitem(itemI, part);
      if (postprocess !== null) itemI = postprocess(itemI);
      out.push(itemI);
    }
    return out;
  };
}

function ignoreCase(v) {
  // filters.py ignore_case: isinstance(value, str).lower() -- Markup is a
  // str subclass so it lower-cases to plain text as well.
  if (typeof v === "string") return v.toLowerCase();
  if (v instanceof String || v instanceof Markup) return v.toString().toLowerCase();
  return v;
}

function do_join(evalCtx, value, d = "", attribute = null) {
  let items;
  if (attribute !== null && attribute !== undefined) {
    const getter = makeAttrgetter(evalCtx.environment, attribute);
    items = pyIter(value).map(getter);
  } else {
    items = pyIter(value);
  }
  if (!evalCtx.autoescape) {
    // str(d).join(map(str, value))
    return items.map(pyStr).join(pyStr(d));
  }
  if (!hasHtml(d)) {
    let doEscape = false;
    items = items.map((item) => {
      if (hasHtml(item)) { doEscape = true; return item; }
      return pyStr(item);
    });
    const sep = doEscape ? escapeAny(d) : pyStr(d);
    if (doEscape) {
      // Markup.join escapes every non-markup element before joining
      return new Markup(sep.toString()).join(items.map((x) => hasHtml(x) ? x : String(x)));
    }
    return items.map(String).join(sep);
  }
  // delimiter is markup: soft_str(d) keeps the Markup object itself, and
  // Markup.join escapes every element lacking __html__ before joining --
  // the result is Markup (golden-pinned as join-auto-markup-sep).
  const sepText = d instanceof Markup ? d.toString() : String(d);
  const parts = items.map((x) =>
    hasHtml(x) ? x.toString() : escapeAny(x).toString());
  return new Markup(parts.join(sepText));
}

// ------------------------------------------------------------------- sort

function do_sort(env, value, reverse = false, caseSensitive = false, attribute = null) {
  const getter = makeMultiAttrgetter(
    env, attribute, caseSensitive ? null : ignoreCase);
  const items = pyIter(value).map((v, i) => ({ v, i, k: getter(v) }));
  // stable sort (ECMAScript guarantees stability; python sorted is stable too)
  // NaN pairs are unordered: CPython's `<` yields False for them, which in a
  // stable sort means "keep the incoming order" -- encode that as compare 0.
  // CPython's list.sort is a binary insertion sort comparing the *incoming*
  // element against already-placed ones (`item < previous`), so a failing
  // comparison names operands incoming-first (golden-pinned as sort-mixed:
  // ['a',1] -> "'int' and 'str'"). The incoming element is whichever of the
  // two carries the larger original index; evaluate that order for both the
  // message shape AND ordering, then convert to the sign V8 expects for
  // (x, y). Ascending semantics are preserved either way.
  items.sort((x, y) => {
    const incoming = x.i > y.i ? x : y;
    const other = x.i > y.i ? y : x;
    let c = compareKeys(incoming.k, other.k);
    if (c === UNORDERED) return 0;
    return x.i > y.i ? c : -c;
  });
  const out = items.map((x) => x.v);
  if (reverse) out.reverse();
  return out;
}

function compareKeys(a, b) {
  // keys are lists (multi attrgetter): python list comparison semantics
  if (Array.isArray(a) && Array.isArray(b)) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const c = pyCompare(a[i], b[i]);
      if (c !== 0) return c;
    }
    return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
  }
  return pyCompare(a, b);
}

// ------------------------------------------------------------ map/select

function do_map(ctx, value, ...args) {
  // prepare_map equivalent. JS API: positional filter name + its args, or a
  // single options object {attribute, default} for the attribute form.
  // Python order note: argument-shape errors only surface for non-empty
  // sequences (`if value:` guards prepare_map), so check emptiness first.
  if (!truthy(value)) return [];
  let func;
  if (args.length === 1 && isOptions(args[0]) && "attribute" in args[0]) {
    const opts = args[0];
    const deflt = opts.default !== undefined ? opts.default : null;
    func = makeAttrgetter(ctx.environment, opts.attribute, null, deflt);
  } else if (args.length >= 1) {
    const name = args[0];
    const rest = args.slice(1);
    func = (item) => ctx.environment.callFilter(name, item, rest);
  } else {
    throw new TemplateError("map requires a filter argument");
  }
  return pyIter(value).map(func);
}

function isOptions(v) {
  return typeof v === "object" && v !== null && !Array.isArray(v) &&
    !(v instanceof Map) && !(v instanceof Set) && !(v instanceof Undefined) &&
    (Object.getPrototypeOf(v) === Object.prototype || Object.getPrototypeOf(v) === null);
}

function selectOrReject(ctx, value, args, modfunc, lookupAttr) {
  // prepare_select_or_reject equivalent: [attr] [testName testArgs...] or
  // truthiness when no test name is given.
  if (!truthy(value)) return [];
  let transfunc;
  let off = 0;
  if (lookupAttr) {
    if (args.length === 0)
      throw new TemplateError("Missing parameter for attribute name");
    transfunc = makeAttrgetter(ctx.environment, args[0]);
    off = 1;
  } else {
    transfunc = (x) => x;
  }
  let func;
  if (args.length > off) {
    const name = args[off];
    const rest = args.slice(off + 1);
    func = (item) => ctx.environment.callTest(name, item, rest);
  } else {
    func = (v) => truthy(v);
  }
  const out = [];
  for (const item of pyIter(value)) {
    const r = func(transfunc(item));
    if (truthy(modfunc(r))) out.push(item);
  }
  return out;
}

function do_select(ctx, value, ...args) {
  return selectOrReject(ctx, value, args, (x) => x, false);
}
function do_reject(ctx, value, ...args) {
  return selectOrReject(ctx, value, args, (x) => !truthy(x), false);
}
function do_selectattr(ctx, value, ...args) {
  return selectOrReject(ctx, value, args, (x) => x, true);
}
function do_rejectattr(ctx, value, ...args) {
  return selectOrReject(ctx, value, args, (x) => !truthy(x), true);
}

// ------------------------------------------------------------------ escape

function do_escape(v) { return escapeAny(v); }
function do_string(v) { return softStr(v).toString(); }

// --------------------------------------------------------------- registry

// Every entry carries an explicit `pass` mode ("" when the filter needs no
// injected first argument) so registry consumers never see undefined.
export const FILTERS = {
  "abs": { func: do_abs, pass: "" },
  "capitalize": { func: do_capitalize, pass: "" },
  "d": { func: do_default, pass: "" },
  "default": { func: do_default, pass: "" },
  "e": { func: do_escape, pass: "" },
  "escape": { func: do_escape, pass: "" },
  "first": { func: do_first, pass: "env" },
  "float": { func: do_float, pass: "" },
  "int": { func: do_int, pass: "" },
  "join": { func: do_join, pass: "evalCtx" },
  "last": { func: do_last, pass: "env" },
  "length": { func: do_length, pass: "" },
  "count": { func: do_length, pass: "" },
  "list": { func: do_list, pass: "" },
  "lower": { func: do_lower, pass: "" },
  "map": { func: do_map, pass: "ctx" },
  "reject": { func: do_reject, pass: "ctx" },
  "rejectattr": { func: do_rejectattr, pass: "ctx" },
  "replace": { func: do_replace, pass: "evalCtx" },
  "reverse": { func: do_reverse, pass: "" },
  "select": { func: do_select, pass: "ctx" },
  "selectattr": { func: do_selectattr, pass: "ctx" },
  "sort": { func: do_sort, pass: "env" },
  "string": { func: do_string, pass: "" },
  "title": { func: do_title, pass: "" },
  "trim": { func: do_trim, pass: "" },
  "upper": { func: do_upper, pass: "" },
};

function isPlainObj(v) {
  if (typeof v !== "object" || v === null) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}

/** Names the E0 subset deliberately does NOT port (decision D-E4: absent,
 *  never approximated). Tracked for full parity in TODO Phase E. */
export const ABSENT_FILTERS = [
  "attr", "batch", "center", "dictsort", "filesizeformat", "forceescape",
  "format", "groupby", "indent", "items", "max", "min", "pprint", "random",
  "round", "safe", "slice", "striptags", "sum", "truncate", "unique",
  "urlencode", "urlize", "wordcount", "wordwrap", "xmlattr", "tojson",
];
