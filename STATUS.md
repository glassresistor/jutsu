# Jutsu Project Status

This information has been consolidated into ROADMAP.md which provides a complete summary of the project status and what remains to be done.

The jutsu project is a Rust port of Jinja2 with a Python wrapper. All major components have been implemented and tested:

- Phase A: Fork Jinja2 as `jutsu`; Rust lexer wired into `Lexer.tokeniter` ✅ Done
- Phase B: Full expression + statement grammar in Rust; AST rebuilt via `ast_rebuild.py`; default flipped ✅ Done
- Phase C: Compiler/evaluator: expressions → statements/scoping → filter blocks → macros → inheritance/includes/autoescape ✅ Done
- Phase D: Extensions (`loopcontrols`, `do`, `i18n`, `debug`) gating and third-party compat; loaders/bytecache audit; `NativeEnvironment` exclusion ✅ Done

For detailed status information, please see ROADMAP.md