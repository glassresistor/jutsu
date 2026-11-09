// jutsu-wasm renderer: uses the Rust tokenizer (via WASM) + a JS-side
// mini-interpreter for basic templates.
//
// Supports: variable interpolation, filters (upper, lower, capitalize,
// length, join, replace, default, trim, int, float, abs, round, first, last,
// reverse, sort, list, escape), for loops, if/elif/else, arithmetic,
// comparisons, string literals, lists, dicts, and the `~` concat operator.

import wasmInit, { tokenize as wasmTokenize, last_error } from '../pkg/jutsu_wasm.js';

let _initialized = false;

/**
 * Initialize the WASM module. Must be called before using render/tokenize.
 * In the browser this fetches and instantiates the .wasm binary.
 */
export async function init() {
    if (!_initialized) {
        await wasmInit();
        _initialized = true;
    }
}

/**
 * Tokenize a template source using the Rust lexer via WASM.
 * Returns an array of {line, type, value} objects, or throws on error.
 * Must call `await init()` first.
 */
export function tokenize(source) {
    if (!_initialized) throw new Error('Call await init() before tokenize/render');
    const raw = wasmTokenize(source);
    if (raw === undefined) {
        throw new Error(`Lexer error: ${last_error()}`);
    }
    return JSON.parse(raw);
}

/**
 * Render a template string with the given context.
 * Uses the Rust WASM tokenizer + JS interpreter.
 * Must call `await init()` first.
 */
export function render(source, context = {}) {
    if (!_initialized) throw new Error('Call await init() before tokenize/render');
    const tokens = tokenize(source);
    const interp = new Interpreter(tokens, context);
    return interp.render();
}

// ---------------------------------------------------------------------------
// Token types from the Rust lexer (measured, not assumed):
//   block_begin / block_end       — {% %}
//   variable_begin / variable_end — {{ }}
//   comment_begin / comment_end   — {# #}
//   data                          — raw text between tags
//   name                          — ALL identifiers (keywords included: for, if, true, none...)
//   string                        — string literal WITH quotes: "hello"
//   integer / float               — numeric literals
//   operator                      — ALL symbols: + - * / == != < > <= >= ~ | ( ) [ ] { } . , : = **
//   whitespace                    — spaces/tabs
// ---------------------------------------------------------------------------

class Interpreter {
    constructor(tokens, context) {
        this.tokens = tokens;
        this.pos = 0;
        this.context = context;
        this.scopeStack = [];
    }

    render() {
        let out = '';
        while (this.pos < this.tokens.length) {
            const tok = this.tokens[this.pos];
            if (tok.type === 'data') {
                out += tok.value;
                this.pos++;
            } else if (tok.type === 'comment_begin') {
                this.skipComment();
            } else if (tok.type === 'variable_begin') {
                out += String(this.evalVariable() ?? '');
            } else if (tok.type === 'block_begin') {
                const result = this.evalBlock();
                if (result !== null && result !== undefined) out += result;
            } else {
                this.pos++; // skip stray tokens
            }
        }
        return out;
    }

    skipComment() {
        while (this.pos < this.tokens.length && this.tokens[this.pos].type !== 'comment_end') {
            this.pos++;
        }
        this.pos++; // skip comment_end
    }

    evalVariable() {
        this.pos++; // skip variable_begin
        const value = this.evalExpression();
        while (this.pos < this.tokens.length && this.tokens[this.pos].type !== 'variable_end') {
            this.pos++;
        }
        this.pos++; // skip variable_end
        return value;
    }

    evalBlock() {
        this.pos++; // skip block_begin
        // Skip whitespace to find the keyword, then position past it
        while (this.pos < this.tokens.length && this.tokens[this.pos].type === 'whitespace') {
            this.pos++;
        }
        const kw = this.tokens[this.pos];
        if (!kw || kw.type !== 'name') { this.skipToBlockEnd(); return null; }
        this.pos++; // consume keyword name

        switch (kw.value) {
            case 'for': return this.evalFor();
            case 'if': return this.evalIf();
            case 'set': this.evalSet(); return null;
            default: this.skipToBlockEnd(); return null;
        }
    }

    peekKeyword() {
        let i = this.pos;
        while (i < this.tokens.length) {
            const t = this.tokens[i];
            if (t.type === 'whitespace') { i++; continue; }
            if (t.type === 'name') return t;
            return null;
        }
        return null;
    }

    skipToBlockEnd() {
        while (this.pos < this.tokens.length && this.tokens[this.pos].type !== 'block_end') {
            this.pos++;
        }
        this.pos++; // skip block_end
    }

    evalFor() {
        // {% for x in items %} ... {% endfor %}
        const vars = [];
        let foundIn = false;
        while (this.pos < this.tokens.length && !foundIn) {
            const t = this.tokens[this.pos];
            if (t.type === 'name') {
                if (t.value === 'in') { foundIn = true; }
                else { vars.push(t.value); }
            }
            this.pos++;
        }
        // Evaluate the iterable expression up to block_end
        const iterable = this.evalExpressionUntilBlockEnd();

        // Collect body tokens until matching endfor
        const bodyTokens = this.collectUntil('endfor');
        // Skip past the {% endfor %} block
        this.skipToBlockEnd();

        const items = Array.isArray(iterable) ? iterable : [];
        let out = '';
        for (let i = 0; i < items.length; i++) {
            this.pushScope();
            for (const v of vars) this.setVar(v, items[i]);
            this.setVar('loop', {
                index: i + 1,
                revindex: items.length - i,
                first: i === 0,
                last: i === items.length - 1,
                length: items.length
            });
            const sub = new Interpreter(bodyTokens, this.context);
            sub.scopeStack = [...this.scopeStack];
            out += sub.render();
            this.popScope();
        }
        return out;
    }

    evalIf() {
        // Single-pass approach: collect all tokens until endif, then walk branches.
        const firstCond = this.evalExpressionUntilBlockEnd();
        const allTokens = this.collectUntil('endif', ['else', 'elif']);
        this.skipToBlockEnd(); // skip past {% endif %}

        // Parse the token list into branches.
        // Structure: {% if C1 %} B1 {% elif C2 %} B2 ... {% else %} Bn {% endif %}
        // We need: branches = [{cond:C1, body:B1}, {cond:C2, body:B2}, ...] + elseBody
        const branches = [];
        let elseBody = null;
        let currentBody = [];
        let pendingCond = firstCond; // condition for the branch currently being collected

        for (let i = 0; i < allTokens.length; i++) {
            if (allTokens[i].type === 'block_begin') {
                let j = i + 1;
                while (j < allTokens.length && allTokens[j].type === 'whitespace') j++;
                const kwTok = allTokens[j];
                if (kwTok && kwTok.type === 'name' && (kwTok.value === 'elif' || kwTok.value === 'else')) {
                    let endIdx = i;
                    while (endIdx < allTokens.length && allTokens[endIdx].type !== 'block_end') endIdx++;

                    if (kwTok.value === 'elif') {
                        // Close current branch with its pending condition
                        branches.push({ cond: pendingCond, body: currentBody });
                        // Evaluate the new elif's condition for the NEXT branch
                        const condStart = j + 1;
                        const condTokens = allTokens.slice(condStart, endIdx);
                        const condInterp = new Interpreter(condTokens, this.context);
                        condInterp.scopeStack = [...this.scopeStack];
                        try { pendingCond = condInterp.evalExpression(); } catch(e) { pendingCond = false; }
                        currentBody = [];
                    } else {
                        // else: close current branch, rest is elseBody
                        branches.push({ cond: pendingCond, body: currentBody });
                        elseBody = allTokens.slice(endIdx + 1);
                        i = endIdx;
                        break;
                    }
                    i = endIdx;
                    continue;
                }
            }
            currentBody.push(allTokens[i]);
        }

        // If no else was found, the last accumulated body is still a branch
        if (elseBody === null) {
            branches.push({ cond: pendingCond, body: currentBody });
        }

        // Walk branches in order — first true condition wins
        for (const branch of branches) {
            if (branch.cond) {
                const sub = new Interpreter(branch.body, this.context);
                sub.scopeStack = [...this.scopeStack];
                return sub.render();
            }
        }

        // No branch matched — render else body if any
        if (elseBody) {
            const sub = new Interpreter(elseBody, this.context);
            sub.scopeStack = [...this.scopeStack];
            return sub.render();
        }
        return '';
    }

    findKeywordIn(tokens, fromIdx, kw) {
        let i = fromIdx + 1;
        while (i < tokens.length) {
            if (tokens[i].type === 'whitespace') { i++; continue; }
            if (tokens[i].type === 'name' && tokens[i].value === kw) return true;
            return false;
        }
        return false;
    }

    evalSet() {
        let varName = null;
        while (this.pos < this.tokens.length) {
            const t = this.tokens[this.pos];
            if (t.type === 'name' && !varName) { varName = t.value; this.pos++; break; }
            this.pos++;
        }
        // Skip to =
        while (this.pos < this.tokens.length) {
            const t = this.tokens[this.pos];
            if (t.type === 'operator' && t.value === '=') { this.pos++; break; }
            this.pos++;
        }
        this.evalExpressionUntilBlockEnd();
    }

    // Collect tokens until a block with the given closing keyword (e.g., endfor, endif).
    // Handles nesting of the same construct. Optionally includes stop-keyword blocks
    // (like 'else'/'elif' for if-blocks) in the returned token list.
    collectUntil(closeKw, stopKeywords = []) {
        const openKw = closeKw === 'endfor' ? 'for' : closeKw === 'endif' ? 'if' : null;
        const tokens = [];
        let depth = 1;

        while (this.pos < this.tokens.length) {
            const t = this.tokens[this.pos];

            if (t.type === 'block_begin') {
                // Find the keyword inside this block
                let j = this.pos + 1;
                while (j < this.tokens.length && this.tokens[j].type === 'whitespace') j++;
                const kwTok = this.tokens[j];
                const kwVal = (kwTok && kwTok.type === 'name') ? kwTok.value : null;

                if (kwVal === openKw) {
                    depth++;
                } else if (kwVal === closeKw) {
                    depth--;
                    if (depth === 0) {
                        // Don't consume this block_begin — return so caller can skip it
                        return tokens;
                    }
                } else if (stopKeywords.includes(kwVal) && depth === 1) {
                    // Include the stop-keyword block in the token list
                    // Consume through its block_end
                    let end = this.pos;
                    while (end < this.tokens.length && this.tokens[end].type !== 'block_end') end++;
                    for (let k = this.pos; k <= end; k++) tokens.push(this.tokens[k]);
                    this.pos = end + 1;
                    continue;
                }
            }

            tokens.push(t);
            this.pos++;
        }
        return tokens;
    }

    // -----------------------------------------------------------------------
    // Expression evaluation
    // -----------------------------------------------------------------------

    evalExpressionUntilBlockEnd() {
        const val = this.evalOr();
        while (this.pos < this.tokens.length) {
            const t = this.tokens[this.pos];
            if (t.type === 'block_end') break;
            if (t.type === 'whitespace') { this.pos++; continue; }
            break;
        }
        this.pos++; // skip block_end
        return val;
    }

    evalExpression() {
        return this.evalOr();
    }

    evalOr() {
        let left = this.evalAnd();
        while (this.matchName('or')) {
            const right = this.evalAnd();
            left = left ? left : right;
        }
        return left;
    }

    evalAnd() {
        let left = this.evalComparison();
        while (this.matchName('and')) {
            const right = this.evalComparison();
            left = left && right;
        }
        return left;
    }

    evalComparison() {
        let left = this.evalConcat();
        while (this.pos < this.tokens.length) {
            this.skipWs();
            const t = this.tokens[this.pos];
            if (!t) break;
            if (t.type === 'operator' && ['==', '!=', '<', '>', '<=', '>='].includes(t.value)) {
                this.pos++;
                const right = this.evalConcat();
                switch (t.value) {
                    case '==': left = left == right; break;
                    case '!=': left = left != right; break;
                    case '<': left = left < right; break;
                    case '>': left = left > right; break;
                    case '<=': left = left <= right; break;
                    case '>=': left = left >= right; break;
                }
            } else if (t.type === 'name' && t.value === 'in') {
                this.pos++;
                const right = this.evalConcat();
                left = Array.isArray(right) ? right.includes(left) : String(right ?? '').includes(String(left ?? ''));
            } else if (t.type === 'name' && t.value === 'not' && this.peekNameIs('in')) {
                this.pos += 2;
                const right = this.evalConcat();
                left = !(Array.isArray(right) ? right.includes(left) : String(right ?? '').includes(String(left ?? '')));
            } else if (t.type === 'name' && t.value === 'is') {
                this.pos++;
                let negated = false;
                if (this.matchName('not')) negated = true;
                const testTok = this.tokens[this.pos];
                if (testTok && testTok.type === 'name') {
                    this.pos++;
                    left = this.applyTest(left, testTok.value, negated);
                }
            } else break;
        }
        return left;
    }

    evalConcat() {
        let left = this.evalArithmetic();
        while (this.pos < this.tokens.length) {
            this.skipWs();
            const t = this.tokens[this.pos];
            if (!t) break;
            if (t.type === 'operator' && t.value === '~') {
                this.pos++;
                const right = this.evalArithmetic();
                left = String(left ?? '') + String(right ?? '');
            } else break;
        }
        return left;
    }

    evalArithmetic() {
        let left = this.evalTerm();
        while (this.pos < this.tokens.length) {
            this.skipWs();
            const t = this.tokens[this.pos];
            if (!t) break;
            if (t.type === 'operator' && ['+', '-', '%'].includes(t.value)) {
                this.pos++;
                const right = this.evalTerm();
                if (t.value === '+') left = (left ?? 0) + (right ?? 0);
                else if (t.value === '-') left = (left ?? 0) - (right ?? 0);
                else left = (left ?? 0) % (right ?? 1);
            } else break;
        }
        return left;
    }

    evalTerm() {
        let left = this.evalFactor();
        while (this.pos < this.tokens.length) {
            this.skipWs();
            const t = this.tokens[this.pos];
            if (!t) break;
            if (t.type === 'operator' && ['*', '/', '//'].includes(t.value)) {
                this.pos++;
                const right = this.evalFactor();
                if (t.value === '*') left = (left ?? 0) * (right ?? 0);
                else if (t.value === '/') left = (left ?? 0) / (right ?? 1);
                else left = Math.trunc((left ?? 0) / (right ?? 1));
            } else break;
        }
        return left;
    }

    evalFactor() {
        const t = this.peekNonWs();
        if (!t) return undefined;

        if (t.type === 'operator' && t.value === '-') {
            this.pos++;
            return -(this.evalFactor() ?? 0);
        }
        if (t.type === 'name' && t.value === 'not') {
            this.pos++;
            return !this.evalFactor();
        }

        const base = this.evalAtom();
        // Power (right-associative)
        this.skipWs();
        if (this.pos < this.tokens.length && this.tokens[this.pos]?.type === 'operator' && this.tokens[this.pos].value === '**') {
            this.pos++;
            const exp = this.evalFactor();
            return Math.pow(base ?? 0, exp ?? 0);
        }
        return base;
    }

    evalAtom() {
        this.skipWs();
        const t = this.tokens[this.pos];
        if (!t) return undefined;

        // Parenthesized expression
        if (t.type === 'operator' && t.value === '(') {
            this.pos++;
            const val = this.evalOr();
            this.skipWs();
            if (this.tokens[this.pos]?.value === ')') this.pos++;
            return val;
        }

        // List literal
        if (t.type === 'operator' && t.value === '[') {
            this.pos++;
            const items = [];
            while (this.pos < this.tokens.length && this.tokens[this.pos].value !== ']') {
                this.skipWs();
                if (this.tokens[this.pos]?.value === ']') break;
                items.push(this.evalOr());
                this.skipWs();
                if (this.tokens[this.pos]?.value === ',') this.pos++;
            }
            this.pos++; // skip ]
            return items;
        }

        // Dict literal
        if (t.type === 'operator' && t.value === '{') {
            this.pos++;
            const obj = {};
            while (this.pos < this.tokens.length && this.tokens[this.pos].value !== '}') {
                this.skipWs();
                if (this.tokens[this.pos]?.value === '}') break;
                const key = this.evalOr();
                this.skipWs();
                if (this.tokens[this.pos]?.value === ':') this.pos++;
                obj[String(key)] = this.evalOr();
                this.skipWs();
                if (this.tokens[this.pos]?.value === ',') this.pos++;
            }
            this.pos++; // skip }
            return obj;
        }

        // String literal (includes quotes in value)
        if (t.type === 'string') {
            this.pos++;
            let s = t.value;
            // Strip surrounding quotes
            if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
                s = s.slice(1, -1);
            }
            return s;
        }

        // Integer
        if (t.type === 'integer') { this.pos++; return parseInt(t.value, 10); }

        // Float
        if (t.type === 'float') { this.pos++; return parseFloat(t.value); }

        // Name: could be literal (true/false/none) or variable reference
        if (t.type === 'name') {
            this.pos++;
            const name = t.value;
            if (name === 'true') return true;
            if (name === 'false') return false;
            if (name === 'none' || name === 'null') return null;
            let val = this.lookup(name);
            val = this.evalPostfix(val);
            return val;
        }

        // Fallback
        this.pos++;
        return undefined;
    }

    evalPostfix(val) {
        while (this.pos < this.tokens.length) {
            this.skipWs();
            if (this.pos >= this.tokens.length) break;
            const t = this.tokens[this.pos];
            if (!t) break;

            // Filter: | name(args)
            if (t.type === 'operator' && t.value === '|') {
                this.pos++;
                this.skipWs();
                const ftok = this.tokens[this.pos];
                if (ftok && ftok.type === 'name') {
                    this.pos++;
                    let args = [];
                    this.skipWs();
                    if (this.tokens[this.pos]?.value === '(') {
                        this.pos++;
                        while (this.pos < this.tokens.length && this.tokens[this.pos].value !== ')') {
                            this.skipWs();
                            if (this.tokens[this.pos]?.value === ')') break;
                            args.push(this.evalOr());
                            this.skipWs();
                            if (this.tokens[this.pos]?.value === ',') this.pos++;
                        }
                        this.pos++; // skip )
                    }
                    val = this.applyFilter(val, ftok.value, args);
                }
            }
            // Attribute: .name
            else if (t.type === 'operator' && t.value === '.') {
                this.pos++;
                const atok = this.tokens[this.pos];
                if (atok && atok.type === 'name') {
                    this.pos++;
                    val = val != null ? val[atok.value] : undefined;
                    // Method call: .name(args)
                    this.skipWs();
                    if (this.tokens[this.pos]?.value === '(') {
                        this.pos++;
                        let args = [];
                        while (this.pos < this.tokens.length && this.tokens[this.pos].value !== ')') {
                            this.skipWs();
                            if (this.tokens[this.pos]?.value === ')') break;
                            args.push(this.evalOr());
                            this.skipWs();
                            if (this.tokens[this.pos]?.value === ',') this.pos++;
                        }
                        this.pos++;
                        if (typeof val === 'function') val = val(...args);
                    }
                }
            }
            // Index: [expr]
            else if (t.type === 'operator' && t.value === '[') {
                this.pos++;
                const idx = this.evalOr();
                this.skipWs();
                if (this.tokens[this.pos]?.value === ']') this.pos++;
                val = val != null ? val[idx] : undefined;
            }
            else break;
        }
        return val;
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    skipWs() {
        while (this.pos < this.tokens.length && this.tokens[this.pos].type === 'whitespace') {
            this.pos++;
        }
    }

    peekNonWs() {
        let i = this.pos;
        while (i < this.tokens.length && this.tokens[i].type === 'whitespace') i++;
        return this.tokens[i];
    }

    matchName(word) {
        this.skipWs();
        const t = this.tokens[this.pos];
        if (t && t.type === 'name' && t.value === word) {
            this.pos++;
            return true;
        }
        return false;
    }

    peekNameIs(word) {
        const t = this.peekNonWs();
        return t && t.type === 'name' && t.value === word;
    }

    lookup(name) {
        for (let i = this.scopeStack.length - 1; i >= 0; i--) {
            if (name in this.scopeStack[i]) return this.scopeStack[i][name];
        }
        if (name in this.context) return this.context[name];
        // Builtins
        if (name === 'range') return (n) => Array.from({length: n}, (_, i) => i);
        if (name === 'len') return (v) => v != null ? (Array.isArray(v) ? v.length : String(v).length) : 0;
        return undefined;
    }

    setVar(name, value) {
        if (this.scopeStack.length > 0) {
            this.scopeStack[this.scopeStack.length - 1][name] = value;
        } else {
            this.context[name] = value;
        }
    }

    pushScope() { this.scopeStack.push({}); }
    popScope() { this.scopeStack.pop(); }

    applyFilter(val, name, args) {
        switch (name) {
            case 'upper': return String(val ?? '').toUpperCase();
            case 'lower': return String(val ?? '').toLowerCase();
            case 'capitalize': { const s = String(val ?? ''); return s.charAt(0).toUpperCase() + s.slice(1); }
            case 'title': return String(val ?? '').replace(/\w\S*/g, (w) => w.charAt(0).toUpperCase() + w.slice(1));
            case 'length': return val != null ? (Array.isArray(val) ? val.length : String(val).length) : 0;
            case 'join': { const sep = args[0] ?? ''; return Array.isArray(val) ? val.join(sep) : String(val ?? ''); }
            case 'replace': return String(val ?? '').split(args[0] ?? '').join(args[1] ?? '');
            case 'default': return (val === undefined || val === null || val === '') ? (args[0] ?? '') : val;
            case 'trim': case 'strip': return String(val ?? '').trim();
            case 'int': return parseInt(val, 10) || 0;
            case 'float': return parseFloat(val) || 0;
            case 'abs': return Math.abs(Number(val) || 0);
            case 'round': return Math.round(Number(val) || 0);
            case 'first': return Array.isArray(val) ? val[0] : String(val ?? '').charAt(0);
            case 'last': { const s = Array.isArray(val) ? val : String(val ?? ''); return s[s.length - 1]; }
            case 'reverse': { if (Array.isArray(val)) return [...val].reverse(); return String(val ?? '').split('').reverse().join(''); }
            case 'sort': { if (Array.isArray(val)) return [...val].sort(); return String(val ?? '').split('').sort().join(''); }
            case 'list': return Array.isArray(val) ? val : [val];
            case 'safe': return String(val ?? '');
            case 'escape': { const s = String(val ?? ''); return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;').replace(/"/g,'&quot;'); }
            default: return val;
        }
    }

    applyTest(val, name, negated) {
        let result = false;
        switch (name) {
            case 'defined': result = val !== undefined; break;
            case 'undefined': result = val === undefined; break;
            case 'none': result = val === null || val === undefined; break;
            case 'true': result = !!val; break;
            case 'false': result = !val; break;
            case 'even': result = Number(val) % 2 === 0; break;
            case 'odd': result = Number(val) % 2 !== 0; break;
            case 'string': result = typeof val === 'string'; break;
            case 'number': result = typeof val === 'number' || (!isNaN(Number(val)) && val !== ''); break;
            case 'integer': result = Number.isInteger(Number(val)); break;
            case 'sequence': result = Array.isArray(val) || typeof val === 'string'; break;
            case 'mapping': result = val !== null && typeof val === 'object' && !Array.isArray(val); break;
            default: result = false;
        }
        return negated ? !result : result;
    }
}
