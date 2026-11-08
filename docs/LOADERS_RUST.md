# Loaders and Bytecode Cache with Rust Engines

This document describes the audit of loaders and bytecode cache behavior when using rust engines default-on.

## Overview

The loader system in jutsu is implemented entirely in Python and remains untouched by the Rust core. All loader classes (BaseLoader subclasses, FileSystemLoader, PackageLoader, DictLoader, FunctionLoader, PrefixLoader, ChoiceLoader) work identically with both python and rust engines.

The bytecode cache system also operates purely in Python and maintains identical behavior across engine modes.

## Profiling Memo

### Cold Load Costs

Cold-load costs are dominated by:
- File I/O operations (reading template files from disk)
- Pickle serialization/deserialization of compiled templates
- Network latency (for remote loaders)

Lexing and parsing costs are minimal in comparison, confirming that the loader system remains a pure-python component with no rust involvement.

### Performance Analysis

This profiling confirms that:
1. The loader system is not a performance bottleneck
2. All I/O operations occur at the Python level
3. No Rust components are involved in template loading or bytecode caching
4. The Python implementation is fully compatible with rust engine behavior

## Loader Classes Tested

- `BaseLoader` (abstract)
- `FileSystemLoader`
- `PackageLoader`
- `DictLoader`
- `FunctionLoader`
- `PrefixLoader`
- `ChoiceLoader`

All loader classes work identically with:
- Python engines (default behavior)
- Rust engines (with JUTSU_RUST_EVAL=1)

## Bytecode Cache Behavior

`FileSystemBytecodeCache` and other bytecode cache implementations:
- Operate entirely in Python
- Maintain identical round-trip behavior
- Work seamlessly with both engine modes

## Integration Test Results

The integration tests in `tests/test_loaders_rust.py` verify that:
1. All loader classes function correctly
2. Bytecode cache round-trips work properly
3. Templates render identically with both engine types
4. No rust core involvement occurs in loader operations

This behavior is now settled and documented as a pure-python component.