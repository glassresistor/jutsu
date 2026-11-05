// Value bridge for the JavaScript wrapper of jutsu (PLAN_JS.md slice E0).
//
// Every rule here replaces a CPython behavior the Rust evaluator got for free
// by delegating to Python's runtime. Each is documented with its Python
// counterpart and pinned by tests in js/test/.
//
// JS<->python type mapping used throughout (parity caveat, recorded here):
//   python None        <-> JS null            (JS undefined is NOT None)
//   python True/False  <-> JS true/false      (bools are numbers for ==/< rules!)
//   python int         <-> JS number or bigint
//   python float       <-> JS number          (JS cannot distinguish 1 vs 1.0;
//                          `integer`/`float` tests approximate via Number.isInteger)
//   python str         <-> JS string
//   python list/tuple  <-> JS Array
//   python dict        <-> JS Map or plain object literal
//   python set         <-> JS Set
//   markupsafe Markup  <-> JS Markup (extends String)
//   objects with __html__() are trusted (the protocol becomes a JS convention)

/** Sentinel mirroring jinja2's internal `missing` (used where "no value" is
 *  distinct from None). */
export const missing = Symbol("jutsu.missing");

/** Python-style truthiness (jinja uses `if x:` everywhere: filters, tests,
 *  {% if %}, boolean defaults). In JS this must cover Array/Map/Set which are
 *  always truthy natively. */
export function truthy(v) {
  if (v === null || v === undefined) return false;
  if (typeof v === "boolean") return v;
  if (v instanceof Undefined) return v._bool(); // base False; Strict raises
  // CPython bool(float('nan')) is True (NaN != 0), unlike JS Boolean(NaN).
  if (typeof v === "number") return v !== 0;
  if (typeof v === "bigint") return v !== 0n;
  if (typeof v === "string") return v.length > 0;
  if (Array.isArray(v)) return v.length > 0;
  if (v instanceof Map) return v.size > 0;
  if (v instanceof Set) return v.size > 0;
  if (typeof Symbol !== "undefined" && v[Symbol.for("jutsu.bool")] !== undefined) {
    const r = v[Symbol.for("jutsu.bool")]();
    return typeof r === "boolean" ? r : truthy(r);
  }
  if (typeof v === "object") {
    // plain objects behave like mappings: empty {} is falsy in Python terms
    return Object.keys(v).length > 0;
  }
  return true;
}

/** Python `str()` for template output. Jinja's print function applies str()
 *  (never repr()) to values; Undefined prints as "". */
export function pyStr(v) {
  if (v === null) return "None";
  if (v === undefined) return ""; // Undefined-shaped absences print empty
  if (v instanceof Undefined) return v.toString();
  if (v instanceof Markup) return v.toString();
  if (typeof v === "number") {
    // CPython prints floats without trailing .0 -- closest JS equivalent:
    return String(v);
  }
  if (typeof v === "bigint") return v.toString();
  if (typeof v === "string") return v;
  if (typeof v === "boolean") return v ? "True" : "False";
  if (Array.isArray(v)) {
    // lists/tuples print with repr() elements in Python -- the templates we
    // target stringify via join for output; keep a readable fallback:
    return "[" + v.map(reprElement).join(", ") + "]";
  }
  if (v instanceof Map) {
    // python str(dict) shows the repr: Maps print like dict literals
    const parts = [...v].map(([k, val]) => `${keyRepr(k)}: ${reprElement(val)}`);
    return "{" + parts.join(", ") + "}";
  }
  if (v instanceof Set) {
    return "{" + [...v].map(reprElement).join(", ") + "}";
  }
  if (!isPlainObjectish(v)) return String(v);
  if (typeof v === "object") {
    const parts = Object.entries(v).map(([k, val]) => `'${k}': ${reprElement(val)}`);
    return "{" + parts.join(", ") + "}";
  }
  return String(v);
}
function reprElement(v) {
  if (typeof v === "string") return `'${v}'`;
  if (v === null) return "None";
  if (v === undefined || v instanceof Undefined) return "Undefined";
  if (typeof v === "boolean") return v ? "True" : "False";
  if (typeof v === "number" || typeof v === "bigint") return String(v);
  if (Array.isArray(v)) {
    return "[" + v.map(reprElement).join(", ") + "]";
  }
  if (v instanceof Map) {
    // python dicts print repr() keys and values; JS Maps keep insertion order,
    // closest readable equivalent:
    const parts = [...v].map(([k, val]) =>
      `${keyRepr(k)}: ${reprElement(val)}`);
    return "{" + parts.join(", ") + "}";
  }
  if (v instanceof Set) {
    return "{" + [...v].map(reprElement).join(", ") + "}";
  }
  if (typeof v === "object") {
    const parts = Object.entries(v).map(([k, val]) =>
      `'${k}': ${reprElement(val)}`);
    return "{" + parts.join(", ") + "}";
  }
  return String(v);
}

function keyRepr(k) {
  return typeof k === "string" ? `'${k}'` : reprElement(k);
}

/** Mirror of `jutsu.utils.object_type_repr`: the name of an object's type,
 *  with recognized singletons named directly.  The JS-side mapping is a
 *  documented approximation: str->str object, int/float->int/float object,
 *  boolean->bool object, arrays->list object, Maps->dict object,
 *  Sets->set object, plain objects->dict object; Undefined prints as
 *  "Undefined" (python's Undefined has no such singleton rule -- caveat
 *  recorded until the E0 test suites pin it). */
export function objectTypeRepr(v) {
  if (v === null) return "None";
  if (v === undefined || v instanceof Undefined) return "Undefined";
  switch (typeof v) {
    case "string": return "str object";
    case "boolean": return "bool object";
    case "bigint": return "int object";
    case "number":
      return Number.isInteger(v) ? "int object" : "float object";
  }
  if (Array.isArray(v)) return "list object";
  if (v instanceof Map) return "dict object";
  if (v instanceof Set) return "set object";
  if (typeof v === "object") {
    // recognized singletons end here as dict objects; class instances fall
    // through to their constructor name, mirroring module-qualified names:
    const ctor = v.constructor && v.constructor.name;
    return ctor && ctor !== "Object" ? `${ctor} object` : "dict object";
  }
  return String(v);
}

/** Plain-object test: object literals (or prototype-less maps) are the ones
 *  python would format as dicts; class instances stringify via their own
 *  toString instead. */
function isPlainObjectish(v) {
  if (typeof v !== "object" || v === null) return false;
  const proto = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
}

const ENTITY_RE = /[&<>"']/g;
const ENTITIES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

export function escapeHtmlRaw(s) {
  return s.replace(ENTITY_RE, (c) => ENTITIES[c]);
}

export class Markup extends String {
  static escape(s) {
    if (typeof s === "string") return new Markup(escapeHtmlRaw(s));
    return new Markup(escapeHtmlRaw(pyStr(s)));
  }
  __html__() { return this; }
  join(iterable) {
    const parts = Array.from(iterable, (v) => escapeAny(v).toString());
    return new Markup(parts.join(this.toString()));
  }
}

// markupsafe.escape(): values implementing __html__() are trusted as-is;
// anything else is str()ed and escaped. None is NOT special-cased here
// (str(None) -> "None" escapes to "None"); use escapeSilent for that.
export function escapeAny(v) {
  if (typeof v === "string") return new Markup(escapeHtmlRaw(v));
  if (v && typeof v.__html__ === "function") {
    const h = v.__html__();
    return new Markup(typeof h === "string" ? h : pyStr(h));
  }
  return new Markup(escapeHtmlRaw(pyStr(v)));
}

// markupsafe.escape_silent(): None renders as the empty string.
export function escapeSilent(v) {
  if (v === null || v === undefined) return new Markup("");
  return escapeAny(v);
}

export function softStr(v) {
  if (typeof v === "string" || v instanceof Markup) return v.toString();
  return pyStr(v);
}

export class TemplateError extends Error {
  constructor(message) { super(message); this.name = this.constructor.name; }
}
export class UndefinedError extends TemplateError {}
export class NativeNameError extends TemplateError {}
export class FilterNotFound extends TemplateError {}
export class TestNotFound extends TemplateError {}
export class EngineUnavailable extends TemplateError {}

export class Undefined {
  constructor(hint = null, obj = missing, name = null) {
    this._undefined_hint = hint;
    this._undefined_obj = obj;
    this._undefined_name = name;
  }
  get _undefinedMessage() {
    if (this._undefined_hint) return this._undefined_hint;
    if (this._undefined_obj === missing) {
      return `'${this._undefined_name}' is undefined`;
    }
    if (typeof this._undefined_name !== "string") {
      return `${objectTypeRepr(this._undefined_obj)} has no element ${reprElement(this._undefined_name)}`;
    }
    // NOTE the repr on the object-type name, mirroring
    // `f"{object_type_repr(obj)!r} has no attribute {name!r}"`.
    return `'${objectTypeRepr(this._undefined_obj)}' has no attribute '${this._undefined_name}'`;
  }
  // Python counterparts: Undefined.__bool__ -> False, __str__ -> "", __len__ -> 0,
  // __iter__ -> empty iterator. Arithmetic/comparison/call raise UndefinedError.
  _fail() { throw new UndefinedError(this._undefinedMessage); }
  add() { this._fail(); }
  mul() { this._fail(); }
  call() { this._fail(); }
  // Undefined.__eq__: `type(self) is type(other)` -- two undefineds of the
  // *same class* compare equal, an undefined vs anything else unequal.
  _eq(other) {
    return other instanceof Undefined && other.constructor === this.constructor;
  }
  // Undefined.__bool__ / __str__ / __len__ / __iter__ for the bridge helpers.
  _bool() { return false; }
  _len() { return 0; }
  _iterArr() { return []; }
  // attribute/item lookups on a plain undefined fail (runtime.py __getattr__
  // and __getitem__ both route to _fail_with_undefined_error).
  _getattr() { this._fail(); }
  _getitem() { this._fail(); }
  toString() { return ""; }
}

/** ChainableUndefined (runtime.py): getattr/getitem return self instead of
 *  raising; prints as the empty string like the base class. */
export class ChainableUndefined extends Undefined {
  __html__() { return this.toString(); }
  _getattr() { return this; }
  _getitem() { return this; }
}

/** DebugUndefined (runtime.py): prints `{{ ... }}` describing the lookup. */
export class DebugUndefined extends Undefined {
  toString() {
    const m = this._undefined_message();
    return `{{ ${m} }}`;
  }
  _undefined_message() {
    if (this._undefined_hint)
      return `undefined value printed: ${this._undefined_hint}`;
    if (this._undefined_obj === missing) return this._undefined_name;
    return `no such element: ${objectTypeRepr(this._undefined_obj)}[${reprElement(this._undefined_name)}]`;
  }
}

/** StrictUndefined (runtime.py): barks on print, bool, len, iter and
 *  comparison -- nothing but the `defined`/`undefined` tests can touch it. */
export class StrictUndefined extends Undefined {
  _bool() { this._fail(); }
  _len() { this._fail(); }
  _iterArr() { this._fail(); }
  toString() { this._fail(); }
  _eq() { this._fail(); }
}

/** CPython's *type name* used inside built-in error messages ('int', 'str',
 *  'NoneType', ...), distinct from object_type_repr which says 'int object'. */
export function pyTypeName(v) {
  if (v === null || v === undefined) return "NoneType";
  if (v instanceof Undefined) return v.constructor.name; // 'Undefined' etc
  if (typeof v === "string") return "str";
  if (typeof v === "boolean") return "bool";
  if (typeof v === "bigint") return "int";
  if (typeof v === "number")
    return Number.isInteger(v) ? "int" : "float";
  if (Array.isArray(v)) return "list";
  if (v instanceof Map) return "dict";
  if (v instanceof Set) return "set";
  if (isPlainObjectish(v)) return "dict";
  if (v instanceof Markup) return "Markup"; // markupsafe's class name
  const ctor = v.constructor && v.constructor.name;
  return ctor ? ctor : typeof v;
}

/** Python `len()` for bridge consumers (the `length`/`count` filters).
 *  Message shapes follow CPython exactly (golden-pinned). */
export function pyLen(v) {
  if (v instanceof Undefined) return v._len();
  if (v === null || v === undefined)
    throw new TemplateError("object of type 'NoneType' has no len()");
  if (typeof v === "number" || typeof v === "bigint" || typeof v === "boolean")
    throw new TemplateError(`object of type '${pyTypeName(v)}' has no len()`);
  if (typeof v === "string") return v.length;
  if (Array.isArray(v)) return v.length;
  if (v instanceof Map || v instanceof Set) return v.size;
  if (isPlainObjectish(v)) return Object.keys(v).length;
  throw new TemplateError(`object of type '${pyTypeName(v)}' has no len()`);
}

/** Python iteration order for the bridge: dicts iterate over their KEYS,
 *  sets iterate values, everything else iterates itself. */
export function pyIter(v) {
  if (v instanceof Undefined) return v._iterArr();
  if (v === null || v === undefined)
    throw new TemplateError("'NoneType' object is not iterable");
  if (Array.isArray(v)) return v.slice();
  if (typeof v === "string") return [...v];
  if (v instanceof Map) return [...v.keys()];
  if (v instanceof Set) return [...v.values()];
  if (isPlainObjectish(v)) return Object.keys(v);
  if (typeof v[Symbol.iterator] === "function") return [...v];
  throw new TemplateError(
    `'${pyTypeName(v)}' object is not iterable`);
}

/** Python `==` for the bridge and the comparison tests. Named rules:
 *  - bools are numbers: True == 1, False == 0 (CPython), NOT JS semantics;
 *  - undefined == anything: base Undefined equals only same-class undefineds,
 *    StrictUndefined raises instead (runtime.py __eq__ override);
 *  - lists/tuples/dicts compare recursively by element equality;
 *  - numbers/strings compare directly; cross-type non-numeric pairs are
 *    simply unequal (Python `"a" == 1` is False, not an error).
 */
export function pyEq(a, b) {
  if (a instanceof Undefined || b instanceof Undefined) {
    // undefined compared via __eq__ on the defined side; python tries the
    // reflected op as well -- both sides end in Undefined.__eq__ type check.
    const undef = a instanceof Undefined ? a : b;
    return undef._eq(a instanceof Undefined ? b : a);
  }
  // Markup extends String and compares equal to its plain-text twin (the
  // markupsafe rule): normalize both sides through str first.
  if (a instanceof String) a = a.toString();
  if (b instanceof String) b = b.toString();
  if (a === null || b === null) return a === b;
  if (numericish(a) && numericish(b)) return numEq(a, b);
  if (typeof a === "string") {
    return typeof b === "string" && a === b;
  }
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    return a.every((x, i) => pyEq(x, b[i]));
  }
  const da = dictLike(a), db = dictLike(b);
  if (da && db) return pyDictEq(da, db);
  if (a instanceof Set && b instanceof Set) {
    if (a.size !== b.size) return false;
    const bs = [...b];
    return [...a].every((x) => bs.some((y) => pyEq(x, y)));
  }
  if (typeof a === "object" || typeof b === "object") {
    // non-dict object pairs fall through to identity unless both are simple
    // values already handled above; python's default __eq__ is identity.
    return a === b;
  }
  return a === b;
}

function numericish(v) {
  return typeof v === "number" || typeof v === "bigint" ||
    typeof v === "boolean";
}

function numEq(a, b) {
  const an = normNum(a), bn = normNum(b);
  if (an === null || bn === null) return false; // bool-vs-str etc: unequal
  if (typeof an === typeof bn) return an === bn; // NaN-safe: False vs itself
  return Number(an) === Number(bn); // bigint-vs-number, precision caveat
}

/** dict-like entries for equality across Map / plain-object representations;
 *  returns null when v is not a mapping under the bridge mapping. */
function dictLike(v) {
  if (v instanceof Map) return [...v];
  if (isPlainObjectish(v)) return Object.entries(v);
  return null;
}

function pyDictEq(ea, eb) {
  if (ea.length !== eb.length) return false;
  for (const [k, v] of ea) {
    let found = false;
    for (const [bk, bv] of eb) {
      // keys compare by python equality too (int 1 == str '1' is False,
      // so numeric dict keys inside plain objects are a caveat documented
      // with the type mapping at the top of this file).
      if (pyEq(bk, k) && pyEq(bv, v)) { found = true; break; }
    }
    if (!found) return false;
  }
  return true;
}

/** Numeric coercion under the bool-is-number rule (CPython); returns null
 *  when neither side participates numerically. */
function normNum(v) {
  if (typeof v === "boolean") return v ? 1 : 0;
  if (typeof v === "number" || typeof v === "bigint") return v;
  return null;
}

/** Python rich comparison (`<` etc.) for sort and the ordering tests.
 *  Cross-type non-numeric comparisons raise, mirroring CPython's TypeError:
 *  comparing str with int aborts the whole sort rather than guessing. */
// Sentinel returned when a comparison is unordered (NaN involved): CPython
// evaluates every rich comparison involving NaN to False rather than raising.
export const UNORDERED = Symbol("jutsu.unordered");

// sym: operator symbol used in the CPython-shaped TypeError text ('<', '>',
// '>=' ...), matching how each failing builtin names the operation.
export function pyCompare(a, b, sym = "<") {
  cmpUnwrap(a); cmpUnwrap(b); // Undefined comparisons raise UndefinedError
  if (a instanceof String) a = a.toString();
  if (b instanceof String) b = b.toString();
  const an = normNum(a), bn = normNum(b);
  if (an !== null && bn !== null) {
    if (Number.isNaN(Number(an)) || Number.isNaN(Number(bn))) return UNORDERED;
    if (an < bn) return -1;
    if (an > bn) return 1;
    return 0;
  }
  const sa = strComparable(a), sb = strComparable(b);
  if (sa !== null && sb !== null) {
    // python string comparison is code-point order; JS < on strings matches
    // for everything below the surrogate boundary -- documented caveat.
    return sa < sb ? -1 : sa > sb ? 1 : 0;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
      const c = pyCompare(a[i], b[i]);
      if (c !== 0) return c;
    }
    return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
  }
  return _cmpFail(a, b, sym);
}
function cmpUnwrap(v) {
  if (v instanceof Undefined) v._fail(); // comparisons raise on undefined
  return v;
}
function strComparable(v) {
  if (typeof v === "string") return v;
  if (v instanceof String || v instanceof Markup) return v.toString();
  return null;
}
// CPython's comparison TypeError names plain class names ('int', 'str'),
// not the object_type_repr form; golden-pinned against sorted() probes.
function _cmpFail(a, b, sym = "<") {
  throw new TemplateError(
    `'${sym ?? "<"}' not supported between instances of '${pyTypeName(a)}' and '${pyTypeName(b)}'`);
}

/** Python `x in y` containment for the bridge (`test_in`, `{% if x in seq %}`). */
export function pyContains(seq, v) {
  if (v instanceof Undefined || seq instanceof Undefined) {
    // StrictUndefined.__contains__ fails; base Undefined iterates empty so
    // containment is False once the strict side refused to raise.
    if (seq instanceof StrictUndefined) seq._fail();
    if (v instanceof StrictUndefined) v._fail();
    return false;
  }
  if (typeof seq === "string") {
    if (typeof v !== "string") throw new TemplateError(
      `'in <string>' requires string as left operand, not ${pyTypeName(v)}`);
    return seq.includes(v);
  }
  if (Array.isArray(seq)) return seq.some((x) => pyEq(x, v));
  if (seq instanceof Map) {
    if (seq.has(v)) return true;
    return [...seq.keys()].some((k) => pyEq(k, v));
  }
  if (seq instanceof Set) return [...seq].some((x) => pyEq(x, v));
  if (isPlainObjectish(seq)) {
    // dict membership checks keys
    if (typeof v !== "string") return false;
    return Object.prototype.hasOwnProperty.call(seq, v);
  }
  if (seq && typeof seq[Symbol.iterator] === "function") {
    for (const x of seq) if (pyEq(x, v)) return true;
    return false;
  }
  throw new TemplateError(`'${objectTypeRepr(seq)}' object is not in`);
}

