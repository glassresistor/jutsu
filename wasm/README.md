# @jutsu/wasm

WASM backend for the jutsu template engine.

This package provides a WASM-based implementation of the jutsu template engine core,
allowing it to run in web browsers and other WASM-enabled environments.

## Features

- WASM-based execution of jutsu templates
- Full compatibility with the JavaScript value bridge
- Support for all standard jutsu filters and tests

## Installation

```bash
npm install @jutsu/wasm
```

## Usage

```javascript
import { jutsuWasm } from '@jutsu/wasm';

// Initialize the WASM module
const wasm = await jutsuWasm.init();

// Compile a template
const compiled = wasm.compile('Hello {{ name }}!');

// Render with context
const result = wasm.render(compiled, { name: 'World' });
console.log(result); // "Hello World!"
```
