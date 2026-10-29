//! Port of jinja2's lexer (`jutsu/lexer.py`) to Rust.
//!
//! The python implementation drives a regex-based state machine; this port
//! reimplements the same semantics with hand-written matchers over a
//! `Vec<char>` (python indexes by code point, not byte) so token streams are
//! identical to the original implementation.  Fidelity is validated against
//! the original jinja2 lexer via differential tests.

use crate::unicode_tables::{is_digit, is_ident, is_ws};

pub const T_WS: &str = "whitespace";
pub const T_FLOAT: &str = "float";
pub const T_INT: &str = "integer";
pub const T_NAME: &str = "name";
pub const T_STRING: &str = "string";
pub const T_OPERATOR: &str = "operator";
pub const T_BLOCK_BEGIN: &str = "block_begin";
pub const T_BLOCK_END: &str = "block_end";
pub const T_VAR_BEGIN: &str = "variable_begin";
pub const T_VAR_END: &str = "variable_end";
pub const T_RAW_BEGIN: &str = "raw_begin";
pub const T_RAW_END: &str = "raw_end";
pub const T_COMMENT_BEGIN: &str = "comment_begin";
pub const T_COMMENT_END: &str = "comment_end";
pub const T_COMMENT: &str = "comment";
pub const T_LINESTMT_BEGIN: &str = "linestatement_begin";
pub const T_LINESTMT_END: &str = "linestatement_end";
pub const T_LINECOMMENT_BEGIN: &str = "linecomment_begin";
pub const T_LINECOMMENT_END: &str = "linecomment_end";
pub const T_LINECOMMENT: &str = "linecomment";
pub const T_DATA: &str = "data";

/// Errors raised by the lexer.  `Fail` corresponds to jinja's `Failure` class,
/// which is invoked as `error_class(message, lineno, filename)` -- i.e. the
/// filename ends up in the `name` slot (a quirk preserved for compatibility).
#[derive(Debug, Clone, PartialEq)]
pub enum LexError {
    /// Tokens already produced before the failure (jinja's tokeniter is a
    /// generator, so partial output must be preserved).
    Fail { message: String, lineno: u32, partial: Vec<Tok> },
    Syntax { message: String, lineno: u32, partial: Vec<Tok> },
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexError::Fail { message, .. }
            | LexError::Syntax { message, .. } => write!(f, "{}", message)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub block_start: String,
    pub block_end: String,
    pub variable_start: String,
    pub variable_end: String,
    pub comment_start: String,
    pub comment_end: String,
    pub line_statement_prefix: Option<String>,
    pub line_comment_prefix: Option<String>,
    pub trim_blocks: bool,
    pub lstrip_blocks: bool,
    pub keep_trailing_newline: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            block_start: "{%".into(),
            block_end: "%}".into(),
            variable_start: "{{".into(),
            variable_end: "}}".into(),
            comment_start: "{#".into(),
            comment_end: "#}".into(),
            line_statement_prefix: None,
            line_comment_prefix: None,
            trim_blocks: false,
            lstrip_blocks: false,
            keep_trailing_newline: false,
        }
    }
}

/// A raw token produced by the tokenizer: (lineno, token_type, value).
pub type Tok = (u32, String, String);

// ---------------------------------------------------------------------------
// helpers over char slices
// ---------------------------------------------------------------------------

fn starts_with(chars: &[char], p: usize, needle: &[char]) -> bool {
    if needle.is_empty() {
        return true;
    }
    let end = p + needle.len();
    end <= chars.len() && chars[p..end] == *needle
}

/// maximal run of python `\s` starting at `p`; returns end index.
fn ws_run(chars: &[char], p: usize) -> usize {
    let mut i = p;
    while i < chars.len() && is_ws(chars[i]) {
        i += 1
    }
    i
}

/// `[ \t\v]*` -- literal character set used by the line-statement rule.
fn htvp_run(chars: &[char], p: usize) -> usize {
    let mut i = p;
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || c == '\t' || c == '\u{b}' {
            i += 1
        } else {
            break;
        }
    }
    i
}

/// `[^\S\r\n]*` -- whitespace except CR/LF.
fn soft_ws_run(chars: &[char], p: usize) -> usize {
    let mut i = p;
    while i < chars.len() {
        let c = chars[i];
        if c != '\r' && c != '\n' && is_ws(c) {
            i += 1
        } else {
            break;
        }
    }
    i
}

fn count_newlines(chars: &[char], from: usize, to: usize) -> u32 {
    let mut n = 0u32;
    for c in chars[from..to].iter() {
        if *c == '\n' {
            n += 1
        }
    }
    n
}

fn join(chars: &[char], from: usize, to: usize) -> String {
    chars[from..to].iter().collect()
}

/// python `str.rstrip()` on a range (Py_UNICODE_ISSPACE, same as `\s`).
fn rstrip_end(chars: &[char], from: usize, to: usize) -> usize {
    let mut i = to;
    while i > from && is_ws(chars[i - 1]) {
        i -= 1
    }
    i
}

// ---------------------------------------------------------------------------
// numeric / literal matchers used inside tag states (block/variable/...)
// ---------------------------------------------------------------------------

/// maximal digit run at p, or None when it doesn't start with a digit.
fn digits_run_at(chars: &[char], p: usize) -> Option<usize> {
    let n = chars.len();
    if p >= n || !is_digit(chars[p]) {
        return None;
    }
    let mut i = p;
    while i < n && is_digit(chars[i]) {
        i += 1
    }
    Some(i)
}

/// `([0-9]+_)*[0-9]+` with full backtracking semantics of the python regex.
fn group_digits(chars: &[char], p: usize) -> Option<usize> {
    let n = chars.len();
    // recursion is safe: every path either fails fast or advances by >= 1.
    fn rec(chars: &[char], p: usize) -> Option<usize> {
        if let Some(e) = digits_run_at(chars, p) {
            if e < chars.len() && chars[e] == '_' {
                if let Some(r) = rec(chars, e + 1) {
                    return Some(r);
                }
            }
            return Some(e);
        }
        None
    }
    let _ = n;
    rec(chars, p)
}

/// python float regex anchored at p (re.I|re.X):
/// `(?<!\.)(\d+_)*\d+((\.group)?e[^]?group|.group)`
fn match_float(chars: &[char], p: usize) -> Option<usize> {
    // negative lookbehind at pattern start: previous char must not be '.'
    if p > 0 && chars[p - 1] == '.' {
        return None;
    }
    let int_end = group_digits(chars, p)?;
    let n = chars.len();

    // alternative A: optional fractional part, then exponent (required)
    fn exponent(chars: &[char], p: usize) -> Option<usize> {
        let n = chars.len();
        if p >= n {
            return None;
        }
        let c = chars[p];
        if c != 'e' && c != 'E' {
            return None;
        }
        let mut j = p + 1;
        if j < n && (chars[j] == '+' || chars[j] == '-') {
            j += 1;
        }
        group_digits(chars, j)
    }

    // A with fractional part
    if int_end + 1 < n && chars[int_end] == '.' {
        if let Some(f) = group_digits(chars, int_end + 1) {
            if let Some(e) = exponent(chars, f) {
                return Some(e);
            }
        }
    }
    // A without fractional part
    if let Some(e) = exponent(chars, int_end) {
        return Some(e);
    }
    // alternative B: fractional part required
    if int_end < n && chars[int_end] == '.' {
        if let Some(f) = group_digits(chars, int_end + 1) {
            return Some(f);
        }
    }
    None
}

fn hex_digit(c: char) -> bool {
    is_digit(c)
        || {
            let lc = c.to_lowercase().next().unwrap_or(c);
            lc >= 'a' && lc <= 'f'
        }
}

/// greedy repetition of `(_?X)+` -- the optional '_' is only consumed when a
/// following X char exists (regex backtracking).
fn pred_run(chars: &[char], p: usize, pred: fn(char) -> bool) -> Option<usize> {
    let n = chars.len();
    if p >= n || !pred(chars[p]) {
        return None;
    }
    let mut i = p + 1;
    loop {
        if i < n && chars[i] == '_' {
            if i + 1 < n && pred(chars[i + 1]) {
                i += 2;
                continue;
            }
            break;
        } else if i < n && pred(chars[i]) {
            i += 1;
            continue;
        } else {
            break;
        }
    }
    Some(i)
}

/// python integer regex alternatives, in alternation order (re.I|re.X).
fn match_int(chars: &[char], p: usize) -> Option<usize> {
    let n = chars.len();
    if p >= n || !is_digit(chars[p]) {
        return None;
    }
    if chars[p] == '0' {
        if p + 1 < n {
            let t = chars[p + 1].to_lowercase().next().unwrap_or(chars[p + 1]);
            if t == 'b' {
                if let Some(e) = pred_run(chars, p + 2, |c| c == '0' || c == '1') {
                    return Some(e);
                }
            }
            if t == 'o' {
                if let Some(e) = pred_run(chars, p + 2, |c| c >= '0' && c <= '7') {
                    return Some(e);
                }
            }
            if t == 'x' {
                if let Some(e) = pred_run(chars, p + 2, hex_digit) {
                    return Some(e);
                }
            }
        }
        // decimal zero: `0(_?0)*`
        let mut i = p + 1;
        loop {
            if i < n && chars[i] == '_' {
                if i + 1 < n && chars[i + 1] == '0' {
                    i += 2;
                    continue;
                }
                break;
            } else if i < n && chars[i] == '0' {
                i += 1;
                continue;
            } else {
                break;
            }
        }
        return Some(i);
    }
    // `[1-9](_?\d)*` -- each unit consumes exactly one digit (with an
    // optional leading underscore), so keep looping over units.
    if chars[p] >= '1' && chars[p] <= '9' {
        let mut i = p + 1;
        loop {
            if i < n && chars[i] == '_' {
                if i + 1 < n && is_digit(chars[i + 1]) {
                    i += 2;
                    continue;
                }
                break;
            } else if i < n && is_digit(chars[i]) {
                i += 1;
                continue;
            } else {
                break;
            }
        }
        return Some(i);
    }
    None
}

fn match_name(chars: &[char], p: usize) -> Option<usize> {
    if p >= chars.len() || !is_ident(chars[p]) {
        return None;
    }
    let mut i = p + 1;
    while i < chars.len() && is_ident(chars[i]) {
        i += 1
    }
    Some(i)
}

/// `('([^'\\]*(?:\\.[^'\\]*)*)'|"...")` with re.S
fn match_string(chars: &[char], p: usize) -> Option<usize> {
    if p >= chars.len() {
        return None;
    }
    let q = chars[p];
    if q != '\'' && q != '"' {
        return None;
    }
    let mut i = p + 1;
    loop {
        while i < chars.len() && chars[i] != q && chars[i] != '\\' {
            i += 1
        }
        if i >= chars.len() {
            return None;
        }
        if chars[i] == q {
            return Some(i + 1);
        }
        // backslash: `\.` with re.S consumes whatever char follows
        if i + 1 >= chars.len() {
            return None;
        }
        i += 2;
    }
}

const OPERATORS: &[&str] = &[
    "//", "**", "==", "!=", ">=", "<=", "+", "-", "/", "*", "%", "~", "[", "]", "(", ")", "{",
    "}", ">", "<", "=", ".", ":", "|", ",", ";",
];

fn match_operator(chars: &[char], p: usize) -> Option<(usize, String)> {
    let limit = std::cmp::min(chars.len(), p + 2);
    let s: String = chars[p..limit].iter().collect();
    for op in OPERATORS {
        if s.starts_with(op) {
            return Some((p + op.chars().count(), (*op).to_string()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// root-state tag candidates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Raw,
    Comment,
    Block,
    Variable,
    LineStatement,
    LineComment,
}

impl Kind {
    fn token(&self) -> &'static str {
        match self {
            Kind::Raw => T_RAW_BEGIN,
            Kind::Comment => T_COMMENT_BEGIN,
            Kind::Block => T_BLOCK_BEGIN,
            Kind::Variable => T_VAR_BEGIN,
            Kind::LineStatement => T_LINESTMT_BEGIN,
            Kind::LineComment => T_LINECOMMENT_BEGIN,
        }
    }
    fn state(&self) -> &'static str {
        match self {
            Kind::Raw => "raw_begin",
            Kind::Comment => "comment_begin",
            Kind::Block => "block_begin",
            Kind::Variable => "variable_begin",
            Kind::LineStatement => "linestatement_begin",
            Kind::LineComment => "linecomment_begin",
        }
    }
}

struct Cand {
    kind: Kind,
    prefix: Vec<char>,
}

/// `(\-|\+|)`: '-' preferred, then '+', then nothing.  Value is "" for none.
fn opt_sign(chars: &[char], p: usize) -> (usize, String) {
    if p < chars.len() && chars[p] == '-' {
        return (p + 1, "-".into());
    }
    if p < chars.len() && chars[p] == '+' {
        return (p + 1, "+".into());
    }
    (p, "".into())
}

impl Config {
    fn bs(&self) -> Vec<char> {
        self.block_start.chars().collect()
    }
    fn be(&self) -> Vec<char> {
        self.block_end.chars().collect()
    }
    fn ce(&self) -> Vec<char> {
        self.comment_end.chars().collect()
    }
    fn ve(&self) -> Vec<char> {
        self.variable_end.chars().collect()
    }

    /// candidates for the root rule, in priority order (raw first).
    fn candidates(&self) -> Vec<Cand> {
        let mut v = vec![Cand { kind: Kind::Raw, prefix: vec![] }];
        let mut rules: Vec<(usize, &str, Vec<char>)> = vec![
            (
                self.comment_start.chars().count(),
                "comment_begin",
                self.comment_start.chars().collect(),
            ),
            (
                self.block_start.chars().count(),
                "block_begin",
                self.block_start.chars().collect(),
            ),
            (
                self.variable_start.chars().count(),
                "variable_begin",
                self.variable_start.chars().collect(),
            ),
        ];
        if let Some(ref pfx) = self.line_statement_prefix {
            rules.push((
                pfx.chars().count(),
                "linestatement_begin",
                pfx.chars().collect(),
            ));
        }
        if let Some(ref pfx) = self.line_comment_prefix {
            rules.push((
                pfx.chars().count(),
                "linecomment_begin",
                pfx.chars().collect(),
            ));
        }
        // python: sorted(rules, reverse=True) over (len, name, escaped)
        rules.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        for (_, name, prefix) in rules {
            let kind = match name {
                "comment_begin" => Kind::Comment,
                "block_begin" => Kind::Block,
                "variable_begin" => Kind::Variable,
                "linestatement_begin" => Kind::LineStatement,
                _ => Kind::LineComment,
            };
            v.push(Cand { kind, prefix });
        }
        v
    }

    /// end-tag variants `+X | -X\s* | X(suffix)` where suffix = `\n?` when
    /// `trim_suffix` is true.  Returns end index past the whole variant.
    fn end_variant(&self, chars: &[char], p: usize, endstr: &[char], trim_suffix: bool) -> Option<usize> {
        let n = chars.len();
        // +end
        if p < n && chars[p] == '+' && starts_with(chars, p + 1, endstr) {
            return Some(p + 1 + endstr.len());
        }
        // -end \s*
        if p < n && chars[p] == '-' && starts_with(chars, p + 1, endstr) {
            return Some(ws_run(chars, p + 1 + endstr.len()));
        }
        // end (+ optional trailing newline when trim_blocks)
        if starts_with(chars, p, endstr) {
            let mut i = p + endstr.len();
            if trim_suffix && i < n && chars[i] == '\n' {
                i += 1;
            }
            return Some(i);
        }
        None
    }

    /// variable-end variants: `\-X\s* | X` (no '+' variant, no trim suffix)
    fn var_end_variant(&self, chars: &[char], p: usize, endstr: &[char]) -> Option<usize> {
        let n = chars.len();
        if p < n && chars[p] == '-' && starts_with(chars, p + 1, endstr) {
            return Some(ws_run(chars, p + 1 + endstr.len()));
        }
        if starts_with(chars, p, endstr) {
            return Some(p + endstr.len());
        }
        None
    }

    /// match a root candidate at position p; returns (end, sign) where sign is
    /// the control-group value: "-" / "+" / "".
    fn match_cand(&self, cand: &Cand, chars: &[char], p: usize) -> Option<(usize, String)> {
        match cand.kind {
            Kind::Raw => {
                // (?P<raw_begin>{bs}(\-|\+|)\s*raw\s*(?:\+be|\-be\s*|be suffix))
                if !starts_with(chars, p, &self.bs()) {
                    return None;
                }
                let (mut i, sign) = opt_sign(chars, p + self.block_start.chars().count());
                i = ws_run(chars, i);
                if !starts_with(chars, i, &['r', 'a', 'w']) {
                    return None;
                }
                i += 3;
                i = ws_run(chars, i);
                // variants for the raw begin tag are `-be\s* | be` only (no
                // `+be`, no trim-blocks suffix) -- unlike other end tags.
                let n2 = chars.len();
                if i < n2 && chars[i] == '-' && starts_with(chars, i + 1, &self.be()) {
                    let e = ws_run(chars, i + 1 + self.block_end.chars().count());
                    return Some((e, sign));
                }
                if starts_with(chars, i, &self.be()) {
                    return Some((i + self.block_end.chars().count(), sign));
                }
                None
            }
            Kind::Comment | Kind::Block | Kind::Variable => {
                if !starts_with(chars, p, &cand.prefix) {
                    return None;
                }
                let (i, sign) = opt_sign(chars, p + cand.prefix.len());
                Some((i, sign))
            }
            Kind::LineStatement => {
                // ^ [ \t\v]* prefix (\-|\+|)
                if !(p == 0 || chars[p - 1] == '\n') {
                    return None;
                }
                let i = htvp_run(chars, p);
                if !starts_with(chars, i, &cand.prefix) {
                    return None;
                }
                let (j, sign) = opt_sign(chars, i + cand.prefix.len());
                Some((j, sign))
            }
            Kind::LineComment => {
                // (?:^|(?<=\S)) [^\S\r\n]* prefix (\-|\+|)
                let ok_start = p == 0 || chars[p - 1] == '\n' || !is_ws(chars[p - 1]);
                if !ok_start {
                    return None;
                }
                let i = soft_ws_run(chars, p);
                if !starts_with(chars, i, &cand.prefix) {
                    return None;
                }
                let (j, sign) = opt_sign(chars, i + cand.prefix.len());
                Some((j, sign))
            }
        }
    }

    /// match an `endraw` tag at position p. returns (end, sign).
    fn match_endraw(&self, chars: &[char], p: usize) -> Option<(usize, String)> {
        if !starts_with(chars, p, &self.bs()) {
            return None;
        }
        let (mut i, sign) = opt_sign(chars, p + self.block_start.chars().count());
        i = ws_run(chars, i);
        if !starts_with(chars, i, &['e', 'n', 'd', 'r', 'a', 'w']) {
            return None;
        }
        i += 6;
        i = ws_run(chars, i);
        let end = self.end_variant(chars, i, &self.be(), self.trim_blocks)?;
        Some((end, sign))
    }
}

// ---------------------------------------------------------------------------
// strip / lstrip handling shared by root and raw states
// ---------------------------------------------------------------------------

/// Applies the OptionalLStrip logic to text range [text_start..tag_start).
/// Returns (data_end, newlines_stripped).
fn apply_strip(
    cfg: &Config,
    chars: &[char],
    text_start: usize,
    tag_start: usize,
    sign: &str,
    is_variable: bool,
    line_starting: bool,
) -> (usize, u32) {
    let mut data_end = tag_start;
    let mut stripped_nl = 0u32;
    if sign == "-" {
        let stripped = rstrip_end(chars, text_start, tag_start);
        stripped_nl = count_newlines(chars, stripped, tag_start);
        data_end = stripped;
    } else if sign != "+" && cfg.lstrip_blocks && !is_variable {
        // l_pos (relative to the text group) = last '\n' index + 1
        let mut rel: Option<usize> = None;
        for i in text_start..tag_start {
            if chars[i] == '\n' {
                rel = Some(i - text_start);
            }
        }
        let (l_pos, proceed) = match rel {
            Some(k) => (k + 1, true),
            None => (0, line_starting),
        };
        if proceed {
            // whitespace_re.fullmatch(text, l_pos): nonempty all-\s run from
            // l_pos to end of text.
            let abs = text_start + l_pos;
            let mut all_ws = true;
            let mut any = false;
            for i in abs..tag_start {
                any = true;
                if !is_ws(chars[i]) {
                    all_ws = false;
                    break;
                }
            }
            if all_ws && any {
                data_end = abs;
            }
        }
    }
    (data_end, stripped_nl)
}

// ---------------------------------------------------------------------------
// main tokenizer -- mirrors Lexer.tokeniter exactly
// ---------------------------------------------------------------------------

fn normalize_source(cfg: &Config, source: &str) -> String {
    // lines = newline_re.split(source)[::2]  then join with "\n"
    let mut texts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let cs: Vec<char> = source.chars().collect();
    let mut i = 0usize;
    while i < cs.len() {
        if cs[i] == '\r' {
            texts.push(std::mem::take(&mut cur));
            i += if i + 1 < cs.len() && cs[i + 1] == '\n' { 2 } else { 1 };
        } else if cs[i] == '\n' {
            texts.push(std::mem::take(&mut cur));
            i += 1;
        } else {
            cur.push(cs[i]);
            i += 1;
        }
    }
    texts.push(cur);
    // if !keep_trailing_newline and lines[-1] == "": del lines[-1]
    if !cfg.keep_trailing_newline && texts.last().map(|s| s.as_str()) == Some("") {
        texts.pop();
    }
    texts.join("\n")
}

/// python's repr() for a single-char string (enough fidelity for error msgs).
fn python_repr_char(c: char) -> String {
    let body = match c {
        '\n' => "\\n".to_string(),
        '\t' => "\\t".to_string(),
        '\r' => "\\r".to_string(),
        '\\' => "\\\\".to_string(),
        c if (c as u32) < 0x20 || ((c as u32) >= 0x7f && (c as u32) <= 0xa0) => {
            format!("\\x{:02x}", c as u32)
        }
        _ => c.to_string(),
    };
    // python picks "..." when the value contains ' but no "
    if body.contains('\'') && !body.contains('"') {
        format!("\"{}\"", body)
    } else {
        format!("'{}'", body)
    }
}

pub fn tokenize(cfg: &Config, source: &str, state: Option<&str>) -> Result<Vec<Tok>, LexError> {
    let norm = normalize_source(cfg, source);
    let chars: Vec<char> = norm.chars().collect();
    let n = chars.len();
    let mut out: Vec<Tok> = Vec::new();
    let mut pos: usize = 0;
    let mut lineno: u32 = 1;
    let mut stack: Vec<String> = vec!["root".into()];
    if let Some(st) = state {
        if st != "root" {
            if st != "variable" && st != "block" {
                return Err(LexError::Syntax {
                    message: "invalid state".into(),
                    lineno,
                    partial: Vec::new(),
                });
            }
            stack.push(format!("{}_begin", st));
        }
    }
    let cands = cfg.candidates();
    let mut balancing: Vec<char> = Vec::new();
    let mut newlines_stripped = 0u32;
    let mut line_starting = true;

    'outer: loop {
        let top = stack[stack.len() - 1].clone();
        match top.as_str() {
            "root" => {
                // find earliest tag match at or after pos
                let mut found: Option<(usize, usize, Kind, String)> = None;
                let mut p = pos;
                while p < n && found.is_none() {
                    for cand in &cands {
                        if let Some((end, sign)) = cfg.match_cand(cand, &chars, p) {
                            found = Some((p, end, cand.kind, sign));
                            break;
                        }
                    }
                    if found.is_none() {
                        p += 1;
                    }
                }
                if let Some((tp, end, kind, sign)) = found {
                    let is_variable = kind == Kind::Variable;
                    let (data_end, nl) =
                        apply_strip(cfg, &chars, pos, tp, &sign, is_variable, line_starting);
                    newlines_stripped = nl;
                    if data_end > pos {
                        out.push((lineno, T_DATA.into(), join(&chars, pos, data_end)));
                    }
                    lineno += count_newlines(&chars, pos, data_end) + newlines_stripped;
                    newlines_stripped = 0;
                    // "#bygroup": the tag token
                    out.push((lineno, kind.token().into(), join(&chars, tp, end)));
                    lineno += count_newlines(&chars, tp, end);
                    stack.push(kind.state().into());
                    line_starting = chars[end - 1] == '\n';
                    pos = end;
                } else if pos < n {
                    // rule ".+": consume everything remaining as data
                    out.push((lineno, T_DATA.into(), join(&chars, pos, n)));
                    lineno += count_newlines(&chars, pos, n);
                    line_starting = chars[n - 1] == '\n';
                    pos = n;
                } else {
                    break;
                }
            }
            "comment_begin" => {
                let ce = cfg.ce();
                let mut found: Option<(usize, usize)> = None;
                let mut q = pos;
                while q < n && found.is_none() {
                    if let Some(e) = cfg.end_variant(&chars, q, &ce, cfg.trim_blocks) {
                        found = Some((q, e));
                    } else {
                        q += 1;
                    }
                }
                if let Some((tp, end)) = found {
                    // tokens: (comment, comment_end)
                    if tp > pos {
                        out.push((lineno, T_COMMENT.into(), join(&chars, pos, tp)));
                    }
                    lineno += count_newlines(&chars, pos, tp) + newlines_stripped;
                    newlines_stripped = 0;
                    out.push((lineno, T_COMMENT_END.into(), join(&chars, tp, end)));
                    lineno += count_newlines(&chars, tp, end);
                    stack.pop();
                    line_starting = chars[end - 1] == '\n';
                    pos = end;
                } else if pos >= n {
                    // the failure rule `(.)` cannot match at end of input, so
                    // jinja just ends the stream instead of raising.
                    break;
                } else {
                    return Err(LexError::Fail {
                        message: "Missing end of comment tag".into(),
                        lineno,
                        partial: std::mem::take(&mut out),
                    });
                }
            }
            "raw_begin" => {
                let mut found: Option<(usize, usize, String)> = None;
                let mut q = pos;
                while q < n && found.is_none() {
                    if let Some((e, sign)) = cfg.match_endraw(&chars, q) {
                        found = Some((q, e, sign));
                    } else {
                        q += 1;
                    }
                }
                if let Some((tp, end, sign)) = found {
                    // OptionalLStrip tuple (data, raw_end)
                    let (data_end, nl) =
                        apply_strip(cfg, &chars, pos, tp, &sign, false, line_starting);
                    newlines_stripped = nl;
                    if data_end > pos {
                        out.push((lineno, T_DATA.into(), join(&chars, pos, data_end)));
                    }
                    lineno += count_newlines(&chars, pos, data_end) + newlines_stripped;
                    newlines_stripped = 0;
                    out.push((lineno, T_RAW_END.into(), join(&chars, tp, end)));
                    lineno += count_newlines(&chars, tp, end);
                    stack.pop();
                    line_starting = chars[end - 1] == '\n';
                    pos = end;
                } else if pos >= n {
                    break;
                } else {
                    return Err(LexError::Fail {
                        message: "Missing end of raw directive".into(),
                        lineno,
                        partial: std::mem::take(&mut out),
                    });
                }
            }
            "linecomment_begin" => {
                // (.*?)()(?=\n|$): text up to first newline or end
                let mut stop = n;
                for i in pos..n {
                    if chars[i] == '\n' {
                        stop = i;
                        break;
                    }
                }
                if stop > pos {
                    out.push((lineno, T_LINECOMMENT.into(), join(&chars, pos, stop)));
                }
                lineno += count_newlines(&chars, pos, stop) + newlines_stripped;
                newlines_stripped = 0;
                out.push((lineno, T_LINECOMMENT_END.into(), String::new()));
                // the match does not consume the newline
                stack.pop();
                line_starting = false;
                pos = stop;
            }
            "block_begin" | "variable_begin" | "linestatement_begin" => {
                let mut handled = false;
                // end-tag rule (skipped when braces are unbalanced).  Note the
                // rule may still match at pos == n for line statements, whose
                // end rule can match an empty `$`-anchored string.
                if balancing.is_empty() && !handled {
                    let mut matched: Option<(usize, &'static str)> = None;
                    if top == "block_begin" {
                        if let Some(e) = cfg.end_variant(&chars, pos, &cfg.be(), cfg.trim_blocks) {
                            matched = Some((e, T_BLOCK_END));
                        }
                    } else if top == "variable_begin" {
                        if let Some(e) = cfg.var_end_variant(&chars, pos, &cfg.ve()) {
                            matched = Some((e, T_VAR_END));
                        }
                    } else {
                        // \s*(\n|$) with greedy-then-backtrack semantics: the
                        // engine first tries the maximal \s* run; only when
                        // that fails does it backtrack to shorter runs ending
                        // at a newline.
                        let w = ws_run(&chars, pos);
                        let mut total: Option<usize> = None;
                        if w == n {
                            // greedy run reaches the end; `$` matches there.
                            total = Some(w);
                        } else {
                            let mut q = w;
                            while q > pos {
                                q -= 1;
                                if chars[q] == '\n' {
                                    total = Some(q + 1);
                                    break;
                                }
                            }
                        }
                        if let Some(e) = total {
                            matched = Some((e, T_LINESTMT_END));
                        }
                    }
                    if let Some((end, tok)) = matched {
                        out.push((lineno, tok.into(), join(&chars, pos, end)));
                        lineno += count_newlines(&chars, pos, end);
                        stack.pop();
                        line_starting = end > pos && chars[end - 1] == '\n';
                        pos = end;
                        handled = true;
                    }
                }
                if handled {
                    continue 'outer;
                }

                // tag_rules scan: whitespace, float, integer, name, string, operator
                let mut done: Option<(usize, &'static str)> = None;
                if pos < n {
                    if is_ws(chars[pos]) {
                        done = Some((ws_run(&chars, pos), T_WS));
                    } else if let Some(e) = match_float(&chars, pos) {
                        done = Some((e, T_FLOAT));
                    } else if let Some(e) = match_int(&chars, pos) {
                        done = Some((e, T_INT));
                    } else if let Some(e) = match_name(&chars, pos) {
                        done = Some((e, T_NAME));
                    } else if let Some(e) = match_string(&chars, pos) {
                        done = Some((e, T_STRING));
                    } else if let Some((e, _op)) = match_operator(&chars, pos) {
                        done = Some((e, T_OPERATOR));
                    }
                }
                if let Some((end, tok)) = done {
                    if tok == T_OPERATOR {
                        let data = join(&chars, pos, end);
                        let c0: char = data.chars().next().unwrap();
                        if c0 == '{' {
                            balancing.push('}');
                        } else if c0 == '(' {
                            balancing.push(')');
                        } else if c0 == '[' {
                            balancing.push(']');
                        } else if c0 == '}' || c0 == ')' || c0 == ']' {
                            if balancing.is_empty() {
                                return Err(LexError::Syntax {
                                    message: format!("unexpected '{}'", c0),
                                    lineno,
                                    partial: std::mem::take(&mut out),
                                });
                            }
                            let expected = balancing.pop().unwrap();
                            if expected != c0 {
                                return Err(LexError::Syntax {
                                    message: format!(
                                        "unexpected '{}', expected '{}'",
                                        c0, expected
                                    ),
                                    lineno,
                                    partial: std::mem::take(&mut out),
                                });
                            }
                        }
                    }
                    out.push((lineno, tok.into(), join(&chars, pos, end)));
                    lineno += count_newlines(&chars, pos, end);
                    line_starting = chars[end - 1] == '\n';
                    pos = end;
                } else if pos >= n {
                    // nothing left: implicit EOF
                    break;
                } else {
                    let c = chars[pos];
                    return Err(LexError::Syntax {
                        message: format!("unexpected char {} at {}", python_repr_char(c), pos),
                        lineno,
                        partial: std::mem::take(&mut out),
                    });
                }
            }
            _ => unreachable!("unknown lexer state {}", top),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn plain_text() {
        let toks = tokenize(&cfg(), "Hello world", None).unwrap();
        assert_eq!(toks, vec![(1, T_DATA.into(), "Hello world".into())]);
    }

    #[test]
    fn empty_source_yields_nothing() {
        let toks = tokenize(&cfg(), "", None).unwrap();
        assert!(toks.is_empty());
    }

    #[test]
    fn variable_block() {
        let toks = tokenize(&cfg(), "{{ foo }}", None).unwrap();
        let types: Vec<&str> = toks.iter().map(|(_, t, _)| t.as_str()).collect();
        assert_eq!(types, vec!["variable_begin", "whitespace", "name", "whitespace", "variable_end"]);
    }

    #[test]
    fn numbers() {
        let cases = [
            ("{{ 1 }}", T_INT),
            ("{{ 1_000 }}", T_INT),
            ("{{ 0x1f }}", T_INT),
            ("{{ 1.5 }}", T_FLOAT),
            ("{{ 1e5 }}", T_FLOAT),
            ("{{ 1_2 }}", T_INT),
        ];
        for (src, want) in cases {
            let toks = tokenize(&cfg(), src, None).unwrap();
            // variable_begin, whitespace, literal
            assert_eq!(toks[2].1, want, "{}", src);
        }
    }

    #[test]
    fn group_digits_backtracking() {
        // `1_2` matches the digit group but not as a float
        let toks = tokenize(&cfg(), "{{ 1.5e3 }}", None).unwrap();
        assert_eq!(toks[2].1, T_FLOAT);
        assert_eq!(toks[2].2, "1.5e3");
    }

    #[test]
    fn unbalanced_braces_error() {
        let err = tokenize(&cfg(), "{{ ('a' }}", None).unwrap_err();
        match err {
            LexError::Syntax { message, .. } => {
                assert_eq!(message, "unexpected '}', expected ')'")
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn comment_missing_end() {
        let err = tokenize(&cfg(), "{# nope", None).unwrap_err();
        assert_eq!(
            err,
            LexError::Fail {
                message: "Missing end of comment tag".into(),
                lineno: 1,
                // the comment_begin token is emitted before the failure
                partial: vec![(1, T_COMMENT_BEGIN.into(), "{#".into())],
            }
        );
    }

    #[test]
    fn comment_begin_at_eof_does_not_raise() {
        // jinja's failure rule `(.)` cannot match at end of input, so the
        // stream simply ends.
        let toks = tokenize(&cfg(), "foo {#", None).unwrap();
        assert_eq!(toks.len(), 2);
    }

    #[test]
    fn raw_begin_at_eof_does_not_raise() {
        let toks = tokenize(&cfg(), "{% raw %}", None).unwrap();
        // stream ends without an error
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].1, T_RAW_BEGIN);
    }

    #[test]
    fn raw_block() {
        let toks = tokenize(&cfg(), "{% raw %}{{ nope }}{% endraw %}", None).unwrap();
        let types: Vec<&str> = toks.iter().map(|t| t.1.as_str()).collect();
        assert_eq!(types, vec!["raw_begin", "data", "raw_end"]);
        assert_eq!(toks[1].2, "{{ nope }}");
    }

    #[test]
    fn strip_minus() {
        let toks = tokenize(&cfg(), "   {{- foo -}}   ", None).unwrap();
        // leading whitespace stripped by `{{-`; trailing whitespace consumed
        // into the variable_end token value by the `-}}\s*` variant.
        assert_eq!(toks.len(), 5);
        assert_eq!(toks[0].2, "{{-");
        assert_eq!(toks[4].2, "-}}   ");
    }

    #[test]
    fn lstrip_blocks() {
        let mut c = cfg();
        c.lstrip_blocks = true;
        let toks = tokenize(&c, "   {% foo %}", None).unwrap();
        // the leading whitespace is dropped entirely (empty data token skipped)
        assert_eq!(toks[0].1, T_BLOCK_BEGIN);
    }

    #[test]
    fn line_statements() {
        let mut c = cfg();
        c.line_statement_prefix = Some("%".into());
        let toks = tokenize(&c, "%if true\n", None).unwrap();
        assert_eq!(toks[0].1, T_LINESTMT_BEGIN);
    }

    #[test]
    fn line_comments() {
        let mut c = cfg();
        c.line_comment_prefix = Some("#".into());
        let toks = tokenize(&c, "foo # bar\nbaz", None).unwrap();
        assert_eq!(toks[0].1, T_DATA);
        assert_eq!(toks[0].2, "foo");
        // the comment token includes the preceding whitespace run
        assert_eq!(toks[1].1, T_LINECOMMENT_BEGIN);
        assert_eq!(toks[1].2, " #");
    }
}
