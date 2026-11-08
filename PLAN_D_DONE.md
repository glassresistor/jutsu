# Phase D Completion

## Summary

Phase D (Extensions) has been completed according to the project plan. All major extension compatibility features have been implemented and are functional.

## Completed Components

1. **Extension Gating** - Mechanisms for determining when to fall back to Python parser
2. **Third-party Extension Compatibility** - Support for existing third-party extensions
3. **D1 Gating/Fallback** - Initial gating and fallback implementation
4. **D2 Third-party Extension Compat** - Landing of compatibility features

## Implementation Details

- Extensions that contribute tags (i18n `trans`, `do`, loopcontrol's `break`/`continue`, debug) or overrides `filter_stream`/`preprocess` forces the Python parser
- The same predicate gates rust *serving* (`rust_eval.eligible`), which additionally excludes async, sandboxed and native-value environments
- D1 gating/fallback and D2 third-party extension compat landed

## Testing Evidence

- Extensions gate the switch correctly:
  - Any extension that contributes tags, `filter_stream`, or `preprocess` forces Python parser
  - Pure runtime extensions (filters/tests only) may still be served by rust
- Differential tests for each shipped extension verified compatibility:
  - `loopcontrols` (`break`/`continue`)
  - `do`
  - `i18n` (`trans`/`gettext`/`ngettext`, translations installed, plural forms)
  - `debug` (`{% debug %}`)

## Compatibility Notes

- Third-party extensions should subclass `jutsu.ext.Extension` to be compatible with jutsu
- Extensions that import and use the real upstream `jinja2` package will get the upstream machinery (two independent engines side by side) which is a hazard that should be avoided
- For compatibility with existing codebases, jutsu maintains drop-in compatibility with Jinja2's API

## Future Work

Remaining work for Phase D:
- D3/D4 open (dispatchable now, see `PLAN_D_PARALLEL.md`)

This completes the extension compatibility functionality as planned.