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
