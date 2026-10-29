//! Reproduce CPython's `repr()` for `str` values, used in error messages.
//!
//! Rules (from CPython's `unicode_repr`):
//! - quote selection: prefer ``'``, switch to ``"`` when the string contains
//!   `'` but not `"`.  When it contains both, single quotes are kept and only
//!   the apostrophes are escaped.
//! - the active quote and a literal backslash are escaped with a leading
//!   backslash; ``\n``, ``\t`` and ``\r`` get short escapes, every other
//!   non-printable character uses `\xHH` (< 0x100), `\uXXXX` (< 0x10000) or
//!   `\UXXXXXXXX`.
//! - everything else is emitted literally.

use crate::unicode_tables::is_printable;

pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for ch in s.chars() {
        if ch == quote || ch == '\\' {
            out.push('\\');
            out.push(ch);
        } else if ch == '\n' {
            out.push_str("\\n");
        } else if ch == '\t' {
            out.push_str("\\t");
        } else if ch == '\r' {
            out.push_str("\\r");
        } else if is_printable(ch) {
            out.push(ch);
        } else {
            let cp = ch as u32;
            if cp < 0x100 {
                out.push_str(&format!("\\x{:02x}", cp));
            } else if cp < 0x10000 {
                out.push_str(&format!("\\u{:04x}", cp));
            } else {
                out.push_str(&format!("\\U{:08x}", cp));
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes() {
        assert_eq!(py_repr_str("abc"), "'abc'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        // contains both quote chars -> python keeps single quotes, escapes only
        // the apostrophe and leaves the double quote alone
        assert_eq!(py_repr_str("both ' and \""), "'both \\' and \"'");
    }

    #[test]
    fn escapes() {
        assert_eq!(py_repr_str("\n"), "'\\n'");
        assert_eq!(py_repr_str("\t"), "'\\t'");
        assert_eq!(py_repr_str("\x07"), "'\\x07'");
        // U+0085 and U+2028 are not printable -> escaped at their widths
        assert_eq!(py_repr_str(&format!("{}", '\u{85}' as char)), "'\\x85'");
        assert_eq!(py_repr_str("\u{2028}"), "'\\u2028'");
        // printable characters are kept literally (like python's repr)
        assert_eq!(py_repr_str("\u{300}"), "'\u{300}'");
    }

    #[test]
    fn printable_kept() {
        assert_eq!(py_repr_str("caf\u{e9} \u{1f600}"), "'caf\u{e9} \u{1f600}'");
    }
}
