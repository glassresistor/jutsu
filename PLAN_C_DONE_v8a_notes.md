# C-v8a design notes (working)

## Wiring approach (validated by probes above)

- Eligibility predicate `rust_eval.eligible(environment)`:
  core present + flag on (`JUTSU_RUST_EVAL`, default off during v8a) +
  not `_extensions_contribute` + not async + not sandboxed + default
  `Environment` class only (native types excluded -> D4).
- Serve decision per template, stored on the Template object:
  at first render run rust scan/analyses once against a throwaway
  context; supported -> mark served-from-rust; unsupported -> lazy
  python codegen + mark python-served forever.
- The verdict must not depend on mid-render state (never-guess).

## Key probes already green (see transcript)

1. eval_parsed on a real `t.new_context(...)` reproduces expression render.
2. eval_parsed with an extends source map reproduces inheritance.
3. Two envs sharing one AST handle DISAGREE on missing-filter scan ->
   serve verdict must be per-template (per-environment), never shared.

## Relocation of analyses (H2/H3 warm win)

scan/check_layout/prepare_unit/register_blocks run per render today via
`evaluate_with_sources`.  For the warm win they should run once per template
compile.  BUT: prepare_unit builds BlockFunc objects holding Rc<Session>
which carries live env/filters/undefined -> cannot be built at compile time
and reused across renders with different contexts.

Decision: introduce a rust-side `prepare` entry point returning ONLY the
static verdict (supported / reason) computed from scan+check_layout over the
tree + a lightweight analysis cache, WITHOUT building BlockFuncs.  The exec
path then still builds the session and registers blocks per render exactly as
today.  This keeps match-by-construction while making the warm path skip the
whole-tree scan/layout once per template (cached verdict validated against a
cheap env fingerprint: filters/tests dict identity via id + version guard).

TODO verify fingerprint safety with dedicated probes (B2 rule): any caching
decision must pass the two-environment probe for BOTH engines.
