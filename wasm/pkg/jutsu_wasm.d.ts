/* tslint:disable */
/* eslint-disable */

/**
 * Returns the error message from the last failed tokenize call (empty string if none).
 */
export function last_error(): string;

/**
 * Tokenize a template source string and return tokens as a JSON array.
 *
 * Each token is an object: `{"line": u32, "type": String, "value": String}`.
 * On error, returns `null` and sets the last error message accessible via
 * [`last_error`].
 */
export function tokenize(source: string): string | undefined;

/**
 * Tokenize with custom delimiters. All parameters are optional (use defaults if null).
 */
export function tokenize_custom(source: string, block_start: string, block_end: string, variable_start: string, variable_end: string): string | undefined;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly last_error: () => [number, number];
    readonly tokenize: (a: number, b: number) => [number, number];
    readonly tokenize_custom: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number];
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
