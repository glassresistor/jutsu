//! WASM bindings for the jutsu template lexer.
//!
//! Exposes the pure-Rust tokenizer to JavaScript via wasm-bindgen.
//! The JS side implements a simple renderer on top of the token stream
//! for basic templates (variable interpolation, simple filters).

#[path = "../../crates/jutsu-core/src/unicode_tables.rs"]
mod unicode_tables;

#[path = "../../crates/jutsu-core/src/lexer.rs"]
mod lexer;

use lexer::{Config, LexError};
use wasm_bindgen::prelude::*;

/// Tokenize a template source string and return tokens as a JSON array.
///
/// Each token is an object: `{"line": u32, "type": String, "value": String}`.
/// On error, returns `null` and sets the last error message accessible via
/// [`last_error`].
#[wasm_bindgen]
pub fn tokenize(source: &str) -> Option<String> {
    let cfg = Config::default();
    match lexer::tokenize(&cfg, source, None) {
        Ok(toks) => {
            let items: Vec<String> = toks
                .iter()
                .map(|(line, ttype, value)| {
                    format!(
                        "{{\"line\":{},\"type\":\"{}\",\"value\":\"{}\"}}",
                        line,
                        escape_json(ttype),
                        escape_json(value)
                    )
                })
                .collect();
            Some(format!("[{}]", items.join(",")))
        }
        Err(e) => {
            LAST_ERROR.set(format!("{} (line {})", e, match &e {
                LexError::Fail { lineno, .. } => *lineno,
                LexError::Syntax { lineno, .. } => *lineno,
            }));
            None
        }
    }
}

/// Tokenize with custom delimiters. All parameters are optional (use defaults if null).
#[wasm_bindgen]
pub fn tokenize_custom(
    source: &str,
    block_start: &str,
    block_end: &str,
    variable_start: &str,
    variable_end: &str,
) -> Option<String> {
    let d = Config::default();
    let cfg = Config {
        block_start: if block_start.is_empty() { d.block_start } else { block_start.into() },
        block_end: if block_end.is_empty() { d.block_end } else { block_end.into() },
        variable_start: if variable_start.is_empty() { d.variable_start } else { variable_start.into() },
        variable_end: if variable_end.is_empty() { d.variable_end } else { variable_end.into() },
        comment_start: d.comment_start,
        comment_end: d.comment_end,
        line_statement_prefix: None,
        line_comment_prefix: None,
        trim_blocks: false,
        lstrip_blocks: false,
        keep_trailing_newline: false,
    };
    match lexer::tokenize(&cfg, source, None) {
        Ok(toks) => {
            let items: Vec<String> = toks
                .iter()
                .map(|(line, ttype, value)| {
                    format!(
                        "{{\"line\":{},\"type\":\"{}\",\"value\":\"{}\"}}",
                        line,
                        escape_json(ttype),
                        escape_json(value)
                    )
                })
                .collect();
            Some(format!("[{}]", items.join(",")))
        }
        Err(e) => {
            LAST_ERROR.set(format!("{} (line {})", e, match &e {
                LexError::Fail { lineno, .. } => *lineno,
                LexError::Syntax { lineno, .. } => *lineno,
            }));
            None
        }
    }
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const {
        std::cell::RefCell::new(String::new())
    };
}

/// Returns the error message from the last failed tokenize call (empty string if none).
#[wasm_bindgen]
pub fn last_error() -> String {
    LAST_ERROR.with(|e| e.borrow().clone())
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
