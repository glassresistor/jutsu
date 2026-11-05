// Environment scaffold rule suites (PLAN_JS.md slice E0): constructor rules,
// undefined-factory selection, autoescape resolution, registry operations and
// pass variants, the decline-not-guess render contract (decision D-E3) plus
// the getitem/getattr lookup matrix. Message shapes beyond golden-captured
// paths are pinned in this suite instead of golden (gi-list-str precedent).

import test from "node:test";
import assert from "node:assert/strict";
import * as V from "../src/index.mjs";

function expectRaises(fn, message) {
  return assert.throws(fn, (e) => {
    if (!(e instanceof V.TemplateError)) throw new Error("wrong error type");
    if (message !== undefined && e.message !== message) {
      throw new Error(`got '${e.message}' want '${message}'`);
    }
    return true;
  });
}

test("constructor validation", () => {
  expectRaises(() => new V.Environment({ autoescape: "yes" }),
    "'autoescape' must be a boolean or a function of the template name");
});

test("undefined factory selection by names and classes", () => {
  for (const cls of [V.Undefined, V.StrictUndefined, V.ChainableUndefined,
    V.DebugUndefined]) {
    const e = new V.Environment({ undefined: cls.name });

    assert.equal(e.undefinedClass, cls);
  }
});

test("undefined option validation", () => {
  expectRaises(() => new V.Environment({ undefined: "Nope" }),
    "'undefined' must be a subclass of 'jutsu.Undefined', got unknown name " +
      "'Nope'");

  class Nope {}
  expectRaises(() => new V.Environment({ undefined: Nope }),
    "'undefined' must be a subclass of 'jutsu.Undefined'");

  // custom subclasses select by class object through the prototype chain
  class Custom extends V.ChainableUndefined {}
  assert.equal(new V.Environment({ undefined: Custom }).undefinedClass, Custom);
});

test("compile and render decline without backend", () => {
  const e = new V.Environment();
  expectRaises(() => e.compile("{{ x }}"),
    "compile needs the engine backend attached (E0 scaffold declines; attach via env.attachBackend) for source: {{ x }}");
  expectRaises(() => e.render("x"), "render needs the engine backend attached (E0 scaffold declines; attach via env.attachBackend)")
  expectRaises(() => e.attachBackend({}), "backend must expose a compile() function");
  const rec = [];
  const fake={
  compile(src,o){rec.push(["c",src,Object.keys(o)]);return src.toUpperCase();},
  render(a,b,c2){rec.push(["r",String(a),Object.keys(b||{}).length,String(!!c2.environment)]);return "R";}
};
  assert.equal(e.attachBackend(fake), e, "attach returns same instance");
  assert.equal(e.compile("t"), "T");
  const r = new V.Environment().attachBackend(fake).render("C", { a: 1 });
  assert.equal(r, "R");
  assert.deepEqual(rec.at(-1), ["r", "C", 1, "true"]);
});


test("autoescape callable receives template names", () => {
  const seen = [];
  const spy = function (nm) {
    seen.push(nm === undefined ? "(none)" : String(nm));
    return nm === undefined;
  };
  const e2 = new V.Environment({ autoescape: spy });
  assert.equal(e2.resolveAutoescape(), true);
  e2.resolveAutoescape("tpl.html");
  assert.deepEqual(seen, ["(none)", "tpl.html"]);
  const e5 = new V.Environment({ autoescape: () => ({ a: 1 }) });
  assert.equal(e5.resolveAutoescape("z"), true, "answer coerced through boolean");
});
