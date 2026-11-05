// Environment scaffold for the JavaScript wrapper of jutsu (PLAN_JS.md
// slice E0). Mirrors src/jutsu/environment.py for everything the value
// bridge and filter/test invocation need; template compilation is a clean
// decline (`EngineUnavailable`) until an engine backend is attached --
// decision D-E3: no secret second interpreter.

import {
  missing, truthy, pyStr, Undefined, StrictUndefined, ChainableUndefined,
  DebugUndefined, TemplateError, FilterNotFound, TestNotFound,
  EngineUnavailable, UndefinedError,
} from "./values.mjs";
import { FILTERS as DEFAULT_FILTERS } from "./filters.mjs";
import { TESTS as DEFAULT_TESTS } from "./tests.mjs";

const NOT_FOUND = Symbol("jutsu.notFound");

// Names inherited from Object.prototype that python objects never resolve
// through getattr (user-defined methods named 'size'/'length' are NOT
// reserved -- python getattr would find them). Only the instance walk uses
// this; built-in types (Array/Map/Set/string) expose nothing at all via
// tryAttr's type guards, so Map.size / array.length never leak.
const RESERVED_ATTRS = new Set([
  "constructor", "toString", "valueOf", "hasOwnProperty", "__proto__",
  "prototype",
]);

export class Environment {
  constructor(opts = {}) {
    const undefinedOpt = opts.undefined ?? null;
    this.autoescape = opts.autoescape ?? false;
    if (typeof this.autoescape !== "boolean" && typeof this.autoescape !== "function")
      throw new TemplateError("'autoescape' must be a boolean or a function of the template name");

    // undefined factory selection, mirroring Environment.__init__'s check
    if (undefinedOpt === null) {
      this.undefinedClass = Undefined;
    } else if (typeof undefinedOpt === "function" &&
        (undefinedOpt.prototype instanceof Undefined || undefinedOpt === Undefined)) {
      this.undefinedClass = undefinedOpt;
    } else if (typeof undefinedOpt === "string") {
      const byName = { Undefined, StrictUndefined, ChainableUndefined, DebugUndefined }[
        undefinedOpt];
      if (!byName)
        throw new TemplateError(
          `'undefined' must be a subclass of 'jutsu.Undefined', got unknown name '${undefinedOpt}'`);
      this.undefinedClass = byName;
    } else {
      throw new TemplateError("'undefined' must be a subclass of 'jutsu.Undefined'");
    }

    // registries: Maps so names like 'toString' cannot collide with an
    // inherited Object.prototype member (python dicts have no such trap).
    this.filters = new Map();
    this.tests = new Map();
    for (const [name, entry] of Object.entries(DEFAULT_FILTERS)) this._register(this.filters, name, entry);
    for (const [name, entry] of Object.entries(DEFAULT_TESTS)) this._register(this.tests, name, entry);
    if (opts.filters) this._registerMany(opts.filters, this.filters, "filter");
    if (opts.tests) this._registerMany(opts.tests, this.tests, "test");

    // globals namespace placeholder: python's DEFAULT_NAMESPACE (range/dict/
    // lipsum/...) arrives with the engine backend; E0 keeps a store.
    this.globals = new Map(Object.entries(opts.globals ?? {}));
    this.backend = null;
  }

  _register(map, name, entry) {
    if (typeof entry === "function") map.set(name, { func: entry, pass: "" });
    else map.set(name, { func: entry.func, pass: entry.pass ?? "" });
  }

  _registerMany(src, map, kind) {
    for (const [name, fn] of Object.entries(src)) {
      if (typeof fn !== "function")
        throw new TemplateError(`${kind} '${name}' must be a function`);
      map.set(name, { func: fn, pass: fn.__jutsuPass ?? "" });
    }
  }

  hasFilter(name) { return this.filters.has(name); }
  hasTest(name) { return this.tests.has(name); }

  registerFilter(name, fn, pass = "") {
    if (typeof fn !== "function") throw new TemplateError("filter must be a function");
    this.filters.set(name, { func: fn, pass });
    return this;
  }
  unregisterFilter(name) { this.filters.delete(name); return this; }
  registerTest(name, fn, pass = "") {
    if (typeof fn !== "function") throw new TemplateError("test must be a function");
    this.tests.set(name, { func: fn, pass });
    return this;
  }
  unregisterTest(name) { this.tests.delete(name); return this; }

  /** Environment.undefined factory: hint/obj/name like runtime.Undefined's
   *  constructor. Filters call it as `env.undefined("hint text")`. */
  undefined(hint = null, obj = missing, name = null) {
    return new this.undefinedClass(hint, obj, name);
  }

  /** autoescape resolution: bool or function of the template name (the
   *  callable form from Environment.__init__ docstring). */
  resolveAutoescape(name = null) {
    if (typeof this.autoescape === "function")
      return !!this.autoescape(name === null ? undefined : name);
    return !!this.autoescape;
  }

  attachBackend(backend) {
    // E2 wiring point: an object exposing compile/render over the wasm core.
    if (typeof backend !== "object" || backend === null ||
        typeof backend.compile !== "function")
      throw new TemplateError("backend must expose a compile() function");
    this.backend = backend;
    return this;
  }

  compile(source, opts = {}) {
    if (!this.backend)
      throw new EngineUnavailable(
        "compile needs the engine backend attached (E0 scaffold declines; " +
        "attach via env.attachBackend) for source: " +
        (typeof source === "string" ? source.slice(0, 24) : String(source)));
    return this.backend.compile(source, { environment: this, ...opts });
  }

  render(sourceOrCompiled, vars = {}) {
    if (!this.backend)
      throw new EngineUnavailable(
        "render needs the engine backend attached (E0 scaffold declines; " +
        "attach via env.attachBackend)");
    return this.backend.render(sourceOrCompiled, vars, { environment: this });
  }

  // ---------------------------------------------------------- lookups

  /** Environment.getitem mirror: item lookup first, attribute fallback for
   *  string arguments, undefined otherwise. */
  getitem(obj, argument) {
    const viaItem = tryItem(obj, argument);
    if (viaItem !== NOT_FOUND) return viaItem;
    if (typeof argument === "string") {
      const viaAttr = tryAttr(obj, argument, this);
      if (viaAttr !== NOT_FOUND) return viaAttr;
    }
    return this.undefined(null, obj === undefined ? missing : obj, argument);
  }

  /** Environment.getattr mirror: attribute first, item fallback second. */
  getattr(obj, name) {
    const viaAttr = tryAttr(obj, name, this);
    if (viaAttr !== NOT_FOUND) return viaAttr;
    const viaItem = tryItem(obj, name);
    if (viaItem !== NOT_FOUND) return viaItem;
    return this.undefined(null, obj === undefined ? missing : obj, name);
  }

  // ----------------------------------------------------- invocation

  callFilter(name, value, args = [], context = null) {
    return this._callCommon(this.filters, "filter", name, value, args, context);
  }
  callTest(name, value, args = [], context = null) {
    return this._callCommon(this.tests, "test", name, value, args, context);
  }

  _callCommon(map, typeName, name, value, args, context) {
    const entry = map.get(name); if (!entry) {
      let msg;
      if (name instanceof Undefined) {
        // python builds the message from {name!r} first -- Undefined's repr
        // is "Undefined" -- then appends the failed-call hint.
        msg = `No ${typeName} named Undefined.`;
        try {
          name._fail();
        } catch (e) {
          if (e instanceof UndefinedError)
            msg = `${msg} (${e.message}; did you forget to quote the callable name?)`;
          else throw e;
        }
      } else {
        msg = `No ${typeName} named '${String(name)}'.`;
      }
      throw new TemplateError(msg);
    }
    const callArgs = [value, ...args];
    if (entry.pass === "ctx") {
      if (context === null || context === undefined)
        throw new TemplateError(
          `Attempted to invoke a context ${typeName} without context.`);
      callArgs.unshift(context);
    } else if (entry.pass === "env") {
      callArgs.unshift(this);
    } else if (entry.pass === "evalCtx") {
      // standalone calls have no template name: the autoescape function is
      // asked with a missing name (python EvalContext equivalent).
      callArgs.unshift({
        autoescape: this.resolveAutoescape(null), environment: this });
    }
    return entry.func(...callArgs);
  }
}

// ----------------------------------------------------------- lookup rules

/** Item-access probe (`obj[arg]` in python). Named JS rules per type:
 *  - Undefined: _getitem (base raises UndefinedError, chainable returns
 *    self -- those propagate, python does not catch them here);
 *  - Array: only real integer numbers/bigints index, and python's negative
 *    subscripts wrap (list[-1] is the last element -- a direct consequence of
 *    getitem being `obj[argument]`, pinned golden gi-list-neg/-oob); strings
 *    reject; attribute fallback then fails too;
 *  - Map: direct key presence (`has`); missing -> NOT_FOUND (KeyError path);
 *  - plain object: string keys via own-property only;
 *  - Set / null / primitives: never item-accessible through this probe
 *    (python str[int] works -- documented deviation: templates index
 *    strings via slices/loops, not via getitem; E0 does not port the
 *    `str[0]` form because getattr would also expose str methods as
 *    attributes, see tryAttr).
 */
function tryItem(obj, arg) {
  if (obj instanceof Undefined) return obj._getitem(arg);
  if (obj === null || obj === undefined) return NOT_FOUND;
  if (Array.isArray(obj)) {
    let idx = intIndex(arg);
    if (idx === null) return NOT_FOUND;
    // python list semantics: negative subscripts wrap against length.
    if (idx < 0) idx += obj.length;
    return idx >= 0 && idx < obj.length ? obj[idx] : NOT_FOUND;
  }
  if (obj instanceof Map) {
    if (obj.has(arg)) return obj.get(arg);
    // python dict keys are typed; a numeric arg must not hit a string key
    return NOT_FOUND;
  }
  if (obj instanceof Set) return NOT_FOUND;
  if (isPlain(obj)) {
    if (typeof arg === "string" && Object.prototype.hasOwnProperty.call(obj, arg))
      return obj[arg];
    return NOT_FOUND;
  }
  return NOT_FOUND; // instances: python __getitem__ not modeled in E0
}

/** Attribute probe (`getattr(obj, name)`). Named JS rules:
 *  - Undefined: _getattr (base raises, chainable returns self);
 *  - plain object: own-property only -- inherited Object.prototype members
 *    are RESERVED_ATTRS and never resolve (python dicts expose no attrs);
 *  - Array/Map/Set/string/number/boolean: nothing resolves (python list/dict/
 *    str have no attribute names; JS built-in props like `size`/`length`
 *    must not leak into templates);
 *  - custom class instances: own properties plus the prototype chain below
 *    Object.prototype, minus RESERVED_ATTRS;
 *  - null/undefined: nothing.
 */
function tryAttr(obj, name, env) {
  if (obj instanceof Undefined) return obj._getattr(name);
  if (obj === null || obj === undefined) return NOT_FOUND;
  if (typeof name !== "string") return NOT_FOUND;
  if (isPlain(obj)) {
    if (Object.prototype.hasOwnProperty.call(obj, name)) return obj[name];
    return NOT_FOUND;
  }
  if (Array.isArray(obj) || obj instanceof Map || obj instanceof Set ||
      typeof obj === "string" || typeof obj === "number" ||
      typeof obj === "boolean" || typeof obj === "bigint") {
    // custom-props on plain-object-shaped instances handled below; built-ins
    // expose nothing here.
    if (Array.isArray(obj) && Object.prototype.hasOwnProperty.call(obj, name) &&
        !/^(?:0|[1-9]\d*)$/.test(name))
      return obj[name]; // only a property explicitly assigned onto the array;
    // positional indices are NOT attributes (python list['0'] is undefined,
    // golden-pinned as gi-list-str)
    return NOT_FOUND;
  }
  // class instance: walk own then prototype chain, stopping at
  // Object.prototype -- inherited Object members ('constructor' etc) are
  // never found there in python either. User methods named 'size'/'length'
  // resolve normally (python getattr would find them).
  if (Object.prototype.hasOwnProperty.call(obj, name)) return obj[name];
  let proto = Object.getPrototypeOf(obj);
  while (proto && proto !== Object.prototype) {
    if (RESERVED_ATTRS.has(name)) break;
    if (Object.prototype.hasOwnProperty.call(proto, name)) return proto[name];
    proto = Object.getPrototypeOf(proto);
  }
  return NOT_FOUND;
}

function intIndex(arg) {
  if (typeof arg === "bigint") return Number(arg);
  if (typeof arg === "number" && Number.isInteger(arg)) return arg;
  return null;
}

function isPlain(v) {
  if (typeof v !== "object" || v === null) return false;
  const p = Object.getPrototypeOf(v);
  return p === Object.prototype || p === null;
}
