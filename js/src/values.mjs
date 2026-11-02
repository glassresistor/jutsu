// Value bridge for the JavaScript wrapper of jutsu (PLAN_JS.md slice E0).
//
// Every rule here replaces a CPython behavior the Rust evaluator got for free
// by delegating to Python's runtime. Each is documented with its Python
// counterpart and pinned by tests in js/test/.

/** Sentinel mirroring jinja2's internal `missing` (used where "no value" is
 *  distinct from None). */
export const missing = Symbol("jutsu.missing");

/** Python-style truthiness (jinja uses `if x:` everywhere: filters, tests,
 *  {% if %}, boolean defaults). In JS this must cover Array/Map/Set which are
 *  always truthy natively. */
export function truthy(v) {
  if (v === null || v === undefined) return false;
  if (typeof v === "boolean") return v;
  if (v instanceof Undefined) return false; // Undefined.__bool__ -> False
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
  if (v instanceof Undefined) return v._toString();
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
  if (v instanceof Map || v instanceof Set || (!isPlainObjectish(v))) {
    return String(v);
  }
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

export function escapeAny(v) {
  if (typeof v === "string") return new Markup(escapeHtmlRaw(v));
  if (v && typeof v.__html__ === "function") {
    const h = v.__html__();
    return new Markup(typeof h === "string" ? h : pyStr(h));
  }
  return new Markup(escapeHtmlRaw(pyStr(v)));
}

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
    return `'${objectTypeRepr(this._undefined_obj)}' has no attribute '${this._undefined_name}'`;
  }
  // Python counterparts: Undefined.__bool__ -> False, __str__ -> "", __len__ -> 0,
  // __iter__ -> empty iterator. Arithmetic/comparison/call raise UndefinedError.
  _fail() { throw new UndefinedError(this._undefinedMessage); }
  add() { this._fail(); }
  mul() { this._fail(); }
  call() { this._fail(); }
  toString() { return ""; }
  _toString() { return ""; }
}

