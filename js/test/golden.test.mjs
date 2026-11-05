// Golden replays (PLAN_JS.md slice E0, decision D-E1): every case captured
// from CPython/jutsu by scripts/js_golden.py is re-executed against the JS
// bridge and asserted to produce the same value or the same error message.
// Cases whose python shape cannot exist in JS (tuples) or depends on the
// engine backend (templates, E2/E3) are skipped with a reason.

import test from "node:test";
import * as V from "../src/index.mjs";
import { loadGolden, decode, encode, expectOutcome } from "./golden.mjs";

const data = loadGolden();
if (!data) {
  console.error("golden.json not present -- run python scripts/js_golden.py; skipping golden replays");
}

const SKIPS = {
  "undef-tuple-element": "tuples have no JS representation",
  "float-3.0": "JS cannot distinguish int 3 from float 3.0 (documented approximation)",
  "float-3": "JS cannot distinguish int 3 from float 3.0 (documented approximation)",
  "float-bad": "python returns float 0.0; JS 0 encodes as int (same documented approximation)",
};

const UNDEF_CLASSES = {
  base: V.Undefined, strict: V.StrictUndefined,
  chainable: V.ChainableUndefined, debug: V.DebugUndefined,
};

function mkUndef(c) {
  const cls = UNDEF_CLASSES[c.kind];
  const obj = c.obj === null || c.obj === undefined ? V.missing : decode(c.obj);
  return new cls(c.hint ?? null, obj, c.name ?? null);
}

const env = new V.Environment();
const autoenv = new V.Environment({ autoescape: true });

function runCases(opFilter) {
  if (!data) return;
  for (const c of data.cases.filter((x) => opFilter(x.op))) {
    if (SKIPS[c.id]) continue;
    test(`golden ${c.id}`, () => {
      const ok = replay(c);
      if (!ok) throw new Error(`golden ${c.id} failed`);
    });
  }
}

function replay(c) {
  switch (c.op) {
    case "undefMessage": {
      const u = mkUndef(c);
      return expectOutcome(null, c, () => {
        throw new V.UndefinedError(u._undefinedMessage);
      });
    }
    case "undefOp": {
      const u = mkUndef(c);
      const actions = {
        str: () => V.pyStr(u), bool: () => V.truthy(u),
        len: () => V.pyLen(u), iter: () => V.pyIter(u),
        add: () => u.add(), call: () => u.call(),
        get: () => u._getattr(c.params.attr ?? "x"),
        item: () => u._getitem(decode({ num: [c.params.key, "int"] })),
      };
      return expectOutcome(null, c, actions[c.action]);
    }
    case "filter": {
      const value = decode(c.input.value);
      const args = c.input.args.map(decode);
      return expectOutcome(null, c, () => env.callFilter(c.name, value, args));
    }
    case "evalFilter": {
      const args = c.input.args.map(decode);
      return expectOutcome(null, c,
        () => autoenv.callFilter(c.name, args[0], args.slice(1)));
    }
    case "test": {
      const value = decode(c.input.value);
      const args = c.input.args.map(decode);
      return expectOutcome(null, c, () => env.callTest(c.name, value, args));
    }
    case "getitem":
    case "getattr": {
      const obj = decode(c.obj);
      const arg = decode(c.arg);
      return expectOutcome(null, c, () =>
        (c.op === "getitem" ? env.getitem(obj, arg) : env.getattr(obj, arg)));
    }
    default:
      return true; // template cases and unknown ops: recorded for E2/E3
  }
}

// template cases are recorded for the E2/E3 wasm suites, not replayable now
runCases((op) => op !== "template");
