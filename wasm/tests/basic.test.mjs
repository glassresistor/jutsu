import { test } from 'node:test';
import { strict as assert } from 'assert';

import { jutsuWasm } from '../src/index.mjs';

// Basic test to verify the WASM module can be imported and initialized

test('jutsuWasm can be imported', () => {
  assert.ok(jutsuWasm);
  assert.ok(typeof jutsuWasm.init === 'function');
});

// Test that we can create a basic template

test('can compile and render simple template', async () => {
  // We're testing the structure, not actual WASM execution since we don't have WASM yet
  const wasm = new jutsuWasm.constructor();
  
  // Test compilation
  const compiled = wasm.compile('Hello {{ name }}!');
  assert.ok(compiled);
  assert.equal(compiled.source, 'Hello {{ name }}!');
  
  // Test rendering (this will be stubbed in the basic implementation)
  const result = wasm.render(compiled, { name: 'World' });
  assert.ok(result);
});
