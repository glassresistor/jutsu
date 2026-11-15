# Phase C Criteria & Status Update (Interpreter/Evaluator)

**Goal:** Complete the tree-walking evaluator to achieve full parity with Jinja2's CPython implementation.

**Status:** **✅ Done — C-v4b completed.** The core engine is stable and wired into `Template.render` behind `JUTSU_RUST_EVAL`. The "Retargeting Decision" (R1/R2) has been successfully implemented, moving from a "beat Python" gate to a "comparability band" gate.

### 🎯 Key Deliverables Achieved in Phase C:
* **Core Functionality:** Full implementation of expressions, statements/scoping, filter blocks, macros/call blocks, block inheritance (incl. scoped blocks), includes/imports, autoescape/Markup/volatile regions.
* **AST Caching:** Per-source AST caching is operational (C-v7a).
* **Serve Decisioning:** The C-v8a machinery is in place: the Rust core determines eligibility via `check_supported`, which feeds into Python-side predicates, allowing for per-template verdict caching on the template object.

### 🚧 Remaining Work (C-v9 Focus):
1.  **Resolve Async Environment Rendering (C-v9):** Complete the path for rendering templates within environments that support `async` operations (coroutines, async iterables). *Action:* Ensure the Rust evaluator can correctly await/yield results from async functions defined in the template body and propagate them into the final output stream via the Python runtime bridge.
2.  **Achieve Parent-Side Error Parity (C-v9):** Ensure that error semantics originating from an inherited block or parent scope are reproduced exactly as CPython does, rather than simply being reported as a fallback/divergence. This is critical for template inheritance.
3.  **Complete Performance Pass (C-v9):** Finish the optimizations related to reducing per-node Python object round-trips (`pyo3` calls) within the evaluator loop, ensuring the warm path consistently meets or beats the performance band established in C-v7a across all major template shapes.
4.  **Finalize Code Generator/Runtime Gaps (C-v9):** Address any remaining subtle divergences identified in fuzzing that are not covered by the current fixed test suites (e.g., specific edge cases involving `Markup` objects, complex dynamic call arguments).

### 📈 Performance Determination (Post-Optimization Goal)
* **Baseline Confirmed:** Warm Rust is currently $2.1\times / 3.3\times$ slower than cached Python for Expression/Statement shapes, respectively.
* **Target:** If the final Rust warm time drops below $\sim 5.0 \mu s$ (Expression) and $\sim 6.3 \mu s$ (Statement), the performance goal is achieved, demonstrating a potential **$\sim 1.2\times$ to $1.7\times$ speedup over Python's cached path.**

---
*This document replaces the previous plan detailing Phase C scope.*
