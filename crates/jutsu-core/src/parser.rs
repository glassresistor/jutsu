//! Port of jinja2's recursive-descent parser (`jutsu/parser.py`).
//!
//! The rust parser consumes *wrapped* tokens (values already converted by
//! `Lexer.wrap`) and builds an AST that is serialized into nested tuples which
//! `jutsu.ast_rebuild` reconstructs into `jutsu.nodes.*` instances.  Literal
//! int/float values are carried as opaque pyobjects (`Av::Val`) so arbitrary
//! precision survives the round trip.

#![allow(dead_code)]

use pyo3::prelude::*;
use std::sync::Arc;

use crate::pyrepr::py_repr_str;

/// Error kinds: `Syntax` maps to TemplateSyntaxError, `Assert` to
/// TemplateAssertionError (raised e.g. for imports starting with "_").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrCls {
    Syntax,
    Assert,
}

impl ErrCls {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrCls::Syntax => "syntax",
            ErrCls::Assert => "assert",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParseErr {
    pub message: String,
    pub lineno: Option<u32>,
    pub cls: ErrCls,
    /// Set when the error must be re-raised from stored python exception args.
    pub lexerr: Option<Arc<Py<PyAny>>>,
}

impl ParseErr {
    fn syntax(msg: impl Into<String>, lineno: Option<u32>) -> Self {
        ParseErr { message: msg.into(), lineno, cls: ErrCls::Syntax, lexerr: None }
    }

    pub fn from_lex(args: &Arc<Py<PyAny>>) -> Self {
        ParseErr {
            message: String::new(),
            lineno: None,
            cls: ErrCls::Syntax,
            lexerr: Some(args.clone()),
        }
    }
}

/// Token value carrier. `Num` holds the original python object untouched.
/// Wrapped in Arc so tokens can be cloned without touching the GIL.
#[derive(Clone)]
pub enum PV {
    Str(String),
    Num(Arc<Py<PyAny>>),
}

#[derive(Clone)]
pub struct PToken {
    pub lineno: u32,
    pub ttype: String,
    pub pv: PV,
}

impl PToken {
    fn str_value(&self) -> Option<&str> {
        match &self.pv {
            PV::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `Token.test` from the python lexer: bare type or `type:value`.
    fn test(&self, expr: &str) -> bool {
        if self.ttype == expr {
            return true;
        }
        if let Some((t, v)) = expr.split_once(':') {
            if t == self.ttype {
                if let PV::Str(sv) = &self.pv {
                    return sv == v;
                }
            }
        }
        false
    }

    fn test_any(&self, exprs: &[String]) -> bool {
        exprs.iter().any(|e| self.test(e))
    }
}

/// AST value.  Serialized into the tuple encoding consumed by `ast_rebuild`.
#[derive(Clone)]
pub enum Av {
    /// ("__node__", class, lineno, [args])
    Node(&'static str, Option<u32>, Vec<Av>),
    /// ("__list__", [...])
    List(Vec<Av>),
    /// plain string field -> ("__val__", str)
    Str(String),
    Bool(bool),
    NoneV,
    /// plain python tuple -> ("__tup__", [...])   (FromImport names entries)
    Tuple(Vec<Av>),
    /// pass-through literal object -> ("__val__", obj)
    Val(Arc<Py<PyAny>>),
    /// deferred ''.join of string parts -> ("__joinstr__", [...])
    Join(Vec<Av>),
}

impl Av {
    fn node(name: &'static str, lineno: Option<u32>, args: Vec<Av>) -> Av {
        Av::Node(name, lineno, args)
    }
    fn list(items: Vec<Av>) -> Av {
        Av::List(items)
    }
    fn str_(s: impl Into<String>) -> Av {
        Av::Str(s.into())
    }
    fn boolv(b: bool) -> Av {
        Av::Bool(b)
    }
    fn nonev() -> Av {
        Av::NoneV
    }
}

pub const KNOWN_TAGS: [&str; 14] = [
    "for", "if", "block", "extends", "print", "macro", "include", "from", "import",
    "set", "with", "autoescape", "call", "filter",
];

/// `lexer._describe_token_type`
/// lexer.py `reverse_operators`: token type -> operator symbol.
fn op_symbol(t: &str) -> Option<&'static str> {
    match t {
        "add" => Some("+"),
        "sub" => Some("-"),
        "div" => Some("/"),
        "floordiv" => Some("//"),
        "mul" => Some("*"),
        "mod" => Some("%"),
        "pow" => Some("**"),
        "tilde" => Some("~"),
        "lbracket" => Some("["),
        "rbracket" => Some("]"),
        "lparen" => Some("("),
        "rparen" => Some(")"),
        "lbrace" => Some("{"),
        "rbrace" => Some("}"),
        "eq" => Some("=="),
        "ne" => Some("!="),
        "gt" => Some(">"),
        "gteq" => Some(">="),
        "lt" => Some("<"),
        "lteq" => Some("<="),
        "assign" => Some("="),
        "dot" => Some("."),
        "colon" => Some(":"),
        "pipe" => Some("|"),
        "comma" => Some(","),
        "semicolon" => Some(";"),
        _ => None,
    }
}

fn desc_type(t: &str) -> String {
    if let Some(sym) = op_symbol(t) {
        return sym.to_string();
    }
    match t {
        "comment_begin" => "begin of comment",
        "comment_end" => "end of comment",
        "comment" => "comment",
        "linecomment" => "comment",
        "block_begin" => "begin of statement block",
        "block_end" => "end of statement block",
        "variable_begin" => "begin of print statement",
        "variable_end" => "end of print statement",
        "linestatement_begin" => "begin of line statement",
        "linestatement_end" => "end of line statement",
        "data" => "template data / text",
        "eof" => "end of template",
        _ => return t.to_string(),
    }
    .to_string()
}

/// `lexer.describe_token_expr`
fn describe_token_expr(expr: &str) -> String {
    if let Some((t, v)) = expr.split_once(':') {
        if t == "name" {
            return v.to_string();
        }
        let _ = v;
    } else {
        // fall through
        let _ = expr;
    }
    // note: for a bare expr (no colon) or a non-name type, describe the type.
    let ttype = if let Some((t, _)) = expr.split_once(':') { t } else { expr };
    desc_type(ttype)
}

/// `lexer.describe_token(token)`
fn describe_token(tok: &PToken) -> String {
    if tok.ttype == "name" {
        if let Some(s) = tok.str_value() {
            return s.to_string();
        }
    }
    desc_type(&tok.ttype)
}

pub struct RtParser {
    toks: Vec<PToken>,
    i: usize,
    tag_stack: Vec<String>,
    end_token_stack: Vec<Vec<String>>,
    /// Pending lexer error (the python `TemplateSyntaxError` args tuple).  When
    /// set, the token iterator raises instead of yielding EOF -- reproduce that
    /// by failing at the fetch that would have produced EOF.
    lexerr: Option<Arc<Py<PyAny>>>,
}

impl RtParser {
    pub fn new(toks: Vec<PToken>, lexerr: Option<Arc<Py<PyAny>>>) -> Self {
        RtParser {
            toks,
            i: 0,
            tag_stack: Vec::new(),
            end_token_stack: Vec::new(),
            lexerr,
        }
    }

    /// Token at absolute index.  Past the real tokens python's `TokenStream`
    /// either closes into an EOF token or -- when a lexer error is pending --
    /// raises instead.
    fn tok_at(&self, idx: usize) -> Result<PToken, ParseErr> {
        if let Some(t) = self.toks.get(idx) {
            return Ok(t.clone());
        }
        match &self.lexerr {
            Some(args) => Err(ParseErr::from_lex(args)),
            None => {
                let ln = self.toks.last().map(|t| t.lineno).unwrap_or(1);
                Ok(PToken { lineno: ln, ttype: "eof".into(), pv: PV::Str(String::new()) })
            }
        }
    }

    fn current(&self) -> Result<PToken, ParseErr> {
        self.tok_at(self.i)
    }

    /// Like `current` but never raises.  Only used by error-message helpers,
    /// which can only run while positioned on real tokens (any pending lexer
    /// error would have surfaced during the fetch that got us here).
    fn cur_safe(&self) -> PToken {
        match self.tok_at(self.i) {
            Ok(t) => t,
            Err(_) => {
                let ln = self.toks.last().map(|t| t.lineno).unwrap_or(1);
                PToken { lineno: ln, ttype: "eof".into(), pv: PV::Str(String::new()) }
            }
        }
    }

    fn look(&self) -> Result<PToken, ParseErr> {
        self.tok_at(self.i + 1)
    }

    /// python `next(stream)`: returns the old token and advances.  The pull
    /// that fills `current` happens during the call, so a pending lexer error
    /// surfaces here rather than at the following `current()`.
    fn next_tok(&mut self) -> Result<PToken, ParseErr> {
        let rv = self.tok_at(self.i)?;
        if self.toks.get(self.i + 1).is_none() && self.lexerr.is_some() {
            return Err(ParseErr::from_lex(self.lexerr.as_ref().unwrap()));
        }
        self.i += 1;
        Ok(rv)
    }

    fn skip(&mut self, n: usize) -> Result<(), ParseErr> {
        for _ in 0..n {
            self.next_tok()?;
        }
        Ok(())
    }

    fn next_if(&mut self, expr: &str) -> Result<Option<PToken>, ParseErr> {
        if self.current()?.test(expr) {
            Ok(Some(self.next_tok()?))
        } else {
            Ok(None)
        }
    }

    fn skip_if(&mut self, expr: &str) -> Result<bool, ParseErr> {
        Ok(self.next_if(expr)?.is_some())
    }

    /// `TokenStream.expect` with its exact error messages.
    fn expect(&mut self, expr: &str) -> Result<PToken, ParseErr> {
        let cur = self.current()?;
        if !cur.test(expr) {
            let described = describe_token_expr(expr);
            if cur.ttype == "eof" {
                return Err(ParseErr::syntax(
                    format!(
                        "unexpected end of template, expected {}.",
                        py_repr_str(&described)
                    ),
                    Some(cur.lineno),
                ));
            }
            return Err(ParseErr::syntax(
                format!(
                    "expected token {}, got {}",
                    py_repr_str(&described),
                    py_repr_str(&describe_token(&cur))
                ),
                Some(cur.lineno),
            ));
        }
        self.next_tok()
    }

    fn fail(&self, msg: impl Into<String>, lineno: Option<u32>) -> ParseErr {
        let lineno = match lineno {
            Some(l) => Some(l),
            None => Some(self.cur_safe().lineno),
        };
        ParseErr { message: msg.into(), lineno, cls: ErrCls::Syntax, lexerr: None }
    }

    /// `Parser._fail_ut_eof`
    fn fail_ut_eof(
        &self,
        name: Option<&str>,
        stack: &[Vec<String>],
        lineno: Option<u32>,
    ) -> ParseErr {
        let mut expected: Vec<String> = Vec::new();
        for exprs in stack {
            for e in exprs {
                let d = describe_token_expr(e);
                if !expected.contains(&d) {
                    expected.push(d);
                }
            }
        }
        let currently_looking: Option<String> = if let Some(last) = stack.last() {
            // python: " or ".join(map(repr, map(describe_token_expr, last)))
            let parts: Vec<String> =
                last.iter().map(|e| py_repr_str(&describe_token_expr(e))).collect();
            Some(parts.join(" or "))
        } else {
            None
        };

        let mut message: Vec<String> = Vec::new();
        if name.is_none() {
            message.push("Unexpected end of template.".into());
        } else {
            message.push(format!(
                "Encountered unknown tag {}.",
                py_repr_str(name.unwrap())
            ));
        }

        if let Some(cl) = &currently_looking {
            if name.is_some() && expected.iter().any(|e| e == name.unwrap()) {
                message.push(format!(
                    "You probably made a nesting mistake. Jinja is expecting this tag, but currently looking for {cl}."
                ));
            } else {
                message.push(format!(
                    "Jinja was looking for the following tags: {cl}."
                ));
            }
        }

        if !self.tag_stack.is_empty() {
            message.push(format!(
                "The innermost block that needs to be closed is {}.",
                py_repr_str(self.tag_stack.last().unwrap())
            ));
        }

        let joined = message.join(" ");
        // Parser.fail resolves a missing lineno to the current token's line.
        let lineno = Some(lineno.unwrap_or_else(|| self.cur_safe().lineno));
        ParseErr {
            message: joined,
            lineno: Some(lineno.unwrap()),
            cls: ErrCls::Syntax,
            lexerr: None,
        }
    }

    /// `Parser.fail_unknown_tag`
    fn fail_unknown_tag(&self, name: &str, lineno: Option<u32>) -> ParseErr {
        self.fail_ut_eof(Some(name), &self.end_token_stack, lineno)
    }

    /// `Parser.fail_eof`
    fn fail_eof(&self, end_tokens: Option<&[String]>, lineno: Option<u32>) -> ParseErr {
        let mut stack = self.end_token_stack.clone();
        if let Some(et) = end_tokens {
            stack.push(et.to_vec());
        }
        self.fail_ut_eof(None, &stack, lineno)
    }

    fn is_tuple_end(&self, extra: Option<&[String]>) -> bool {
        let cur = self.cur_safe();
        if cur.ttype == "variable_end" || cur.ttype == "block_end" || cur.ttype == "rparen"
        {
            return true;
        }
        if let Some(e) = extra {
            return cur.test_any(e);
        }
        false
    }
}

// ---------------------------------------------------------------------------
// helpers over the Av tree
// ---------------------------------------------------------------------------

/// `Node.can_assign` dispatch.
pub fn can_assign(a: &Av) -> bool {
    match a {
        Av::Node("Name", _, args) => {
            if let Some(Av::Str(name)) = args.first() {
                name != "true"
                    && name != "false"
                    && name != "none"
                    && name != "True"
                    && name != "False"
                    && name != "None"
            } else {
                false
            }
        }
        Av::Node("NSRef", _, _) => true,
        Av::Node("Tuple", _, args) => {
            if let Some(Av::List(items)) = args.first() {
                items.iter().all(can_assign)
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Recursively set `ctx` on every node that carries a ctx field (the last
/// positional argument of Name/Tuple/Getattr/Getitem), mirroring python's
/// `Node.set_ctx`.
pub fn set_ctx(a: &mut Av, ctx: &str) {
    match a {
        Av::Node(name, _, args) => {
            let has_ctx = matches!(*name, "Name" | "Tuple" | "Getattr" | "Getitem");
            if has_ctx && !args.is_empty() {
                let n = args.len() - 1;
                args[n] = Av::Str(ctx.into());
            }
            // recurse into all children (python's set_ctx walks child nodes)
            for arg in args.iter_mut() {
                set_ctx(arg, ctx);
            }
        }
        Av::List(items) => {
            for it in items.iter_mut() {
                set_ctx(it, ctx);
            }
        }
        _ => {}
    }
}

/// Extract the `name` field of a Name node (as python's `.name`).
pub fn av_name(a: &Av) -> Option<&str> {
    match a {
        Av::Node("Name", _, args) => match args.first() {
            Some(Av::Str(s)) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// The lineno carried by an Av node (python `node.lineno` for fail messages).
pub fn av_lineno(a: &Av) -> Option<u32> {
    match a {
        Av::Node(_, l, _) => *l,
        _ => None,
    }
}

fn python_isspace(s: &str) -> bool {
    !s.is_empty() && s.chars().all(crate::unicode_tables::is_ws)
}

impl RtParser {
    fn fail_assert(&self, msg: impl Into<String>, lineno: Option<u32>) -> ParseErr {
        let mut e = self.fail(msg, lineno);
        e.cls = ErrCls::Assert;
        e
    }

    /// `Parser.parse_statement`.  Always returns a Vec (autoescape wraps its
    /// Scope into a one-element list exactly like python does).
    fn parse_statement(&mut self) -> Result<Vec<Av>, ParseErr> {
        let token = self.current()?;
        if token.ttype != "name" {
            return Err(self.fail("tag name expected", Some(token.lineno)));
        }
        let keyword = match token.str_value() { Some(s) => s.to_string(), None => String::new() };
        // python pushes the token value onto _tag_stack, but for unknown tags
        // pops it again *before* building the error message.  Extension tags
        // never reach the rust path (gated in python).
        if !KNOWN_TAGS.contains(&keyword.as_str()) {
            return Err(self.fail_unknown_tag(&keyword, Some(token.lineno)));
        }
        self.tag_stack.push(keyword.clone());
        let res = self.dispatch_statement(&keyword, token.lineno);
        self.tag_stack.pop();
        res
    }

    fn dispatch_statement(
        &mut self,
        keyword: &str,
        kwlinen: u32,
    ) -> Result<Vec<Av>, ParseErr> {
        match keyword {
            "for" => Ok(vec![self.parse_for()?]),
            "if" => Ok(vec![self.parse_if()?]),
            "block" => Ok(vec![self.parse_block()?]),
            "extends" => Ok(vec![self.parse_extends()?]),
            "print" => Ok(vec![self.parse_print()?]),
            "macro" => Ok(vec![self.parse_macro()?]),
            "include" => Ok(vec![self.parse_include()?]),
            "from" => Ok(vec![self.parse_from_import()?]),
            "import" => Ok(vec![self.parse_import()?]),
            "set" => {
                let r = self.parse_set()?;
                // Assign | AssignBlock -> single node
                Ok(vec![r])
            }
            "with" => Ok(vec![self.parse_with()?]),
            "autoescape" => {
                // python returns a Scope node wrapping one modifier stmt; the
                // caller extends body with it (Scope is itself the statement).
                Ok(vec![self.parse_autoescape()?])
            }
            "call" => Ok(vec![self.parse_call_block()?]),
            "filter" => Ok(vec![self.parse_filter_block()?]),
            _ => Err(self.fail_unknown_tag(keyword, Some(kwlinen))),
        }
    }

    /// `Parser.parse_statements`
    fn parse_statements(
        &mut self,
        end_tokens: Vec<String>,
        drop_needle: bool,
    ) -> Result<Vec<Av>, ParseErr> {
        self.skip_if("colon")?;
        self.expect("block_end")?;
        let result = self.subparse(Some(&end_tokens))?;
        if self.current()?.ttype == "eof" {
            return Err(self.fail_eof(Some(&end_tokens), None));
        }
        if drop_needle {
            self.next_tok()?;
        }
        Ok(result)
    }

    fn parse_set(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        // python: parse_assign_target(with_namespace=True)
        let target = self.parse_assign_target(false, true, None, true)?;
        if self.skip_if("assign")? {
            let expr = self.parse_tuple(false, true, None, false, false)?;
            return Ok(Av::node("Assign", Some(lineno), vec![target, expr]));
        }
        // filter branch (``{% set x | filter %}..{% endset %}``)
        let filter = self.parse_filter(None, false)?;
        let body = self.parse_statements(vec!["name:endset".into()], true)?;
        Ok(Av::node(
            "AssignBlock",
            Some(lineno),
            vec![target, filter.unwrap_or(Av::NoneV), Av::List(body)],
        ))
    }

    fn parse_for(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.expect("name:for")?.lineno;
        let target =
            self.parse_assign_target(false, true, Some(vec!["name:in".into()]), false)?;
        self.expect("name:in")?;
        let iter = self.parse_tuple(
            false,
            false,
            Some(vec!["name:recursive".into()]),
            false,
            false,
        )?;
        let mut test = Av::NoneV;
        if self.skip_if("name:if")? {
            test = self.parse_expression(true)?;
        }
        let recursive = self.skip_if("name:recursive")?;
        let body = self.parse_statements(
            vec!["name:endfor".into(), "name:else".into()],
            false,
        )?;
        let after = self.next_tok()?;
        let else_: Vec<Av> = if after.str_value() == Some("endfor") {
            Vec::new()
        } else {
            self.parse_statements(vec!["name:endfor".into()], true)?
        };
        Ok(Av::node(
            "For",
            Some(lineno),
            vec![target, iter, Av::List(body), Av::List(else_), test, Av::Bool(recursive)],
        ))
    }

    fn parse_if(&mut self) -> Result<Av, ParseErr> {
        // Mirrors python's loop: the outer If holds test/body of the first
        // branch; every `elif` becomes a flat child If with empty elif_/else_
        // (python resets those before checking the next token), and `else`
        // populates only the outer node's else_.
        let lineno = self.expect("name:if")?.lineno;
        let test1 = self.parse_tuple(false, false, None, false, false)?;
        let body1 = self.parse_statements(
            vec!["name:elif".into(), "name:else".into(), "name:endif".into()],
            false,
        )?;
        let mut elifs: Vec<Av> = Vec::new();
        let mut els: Vec<Av> = Vec::new();
        loop {
            let token = self.next_tok()?;
            if token.test("name:elif") {
                let inner_lineno = Some(self.current()?.lineno);
                let itest = self.parse_tuple(false, false, None, false, false)?;
                let ibody = self.parse_statements(
                    vec!["name:elif".into(), "name:else".into(), "name:endif".into()],
                    false,
                )?;
                elifs.push(Av::node(
                    "If",
                    inner_lineno,
                    vec![itest, Av::List(ibody), Av::List(vec![]), Av::List(vec![])],
                ));
                continue;
            } else if token.test("name:else") {
                els = self.parse_statements(vec!["name:endif".into()], true)?;
            }
            break;
        }
        Ok(Av::node(
            "If",
            Some(lineno),
            vec![test1, Av::List(body1), Av::List(elifs), Av::List(els)],
        ))
    }

    fn parse_with(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let mut targets: Vec<Av> = Vec::new();
        let mut values: Vec<Av> = Vec::new();
        while self.current()?.ttype != "block_end" {
            if !targets.is_empty() {
                self.expect("comma")?;
            }
            let mut t = self.parse_assign_target(false, true, None, false)?;
            set_ctx(&mut t, "param");
            targets.push(t);
            self.expect("assign")?;
            values.push(self.parse_expression(true)?);
        }
        let body = self.parse_statements(vec!["name:endwith".into()], true)?;
        Ok(Av::node(
            "With",
            Some(lineno),
            vec![Av::List(targets), Av::List(values), Av::List(body)],
        ))
    }

    fn parse_autoescape(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let expr = self.parse_expression(true)?;
        // Keyword("autoescape", expr) carries no explicit lineno.
        let kw = Av::node("Keyword", None, vec![Av::str_("autoescape"), expr]);
        let body = self.parse_statements(vec!["name:endautoescape".into()], true)?;
        let modifier = Av::node(
            "ScopedEvalContextModifier",
            None,
            vec![Av::List(vec![kw]), Av::List(body)],
        );
        Ok(Av::node("Scope", None, vec![Av::List(vec![modifier])]))
    }

    fn parse_block(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let name_token = self.expect("name")?;
        let name = match name_token.str_value() {
            Some(s) => s.to_string(),
            None => String::new(),
        };
        let scoped = self.skip_if("name:scoped")?;
        let required = self.skip_if("name:required")?;

        if self.current()?.ttype == "sub" {
            return Err(self.fail(
                "Block names in Jinja have to be valid Python identifiers and may not \
                 contain hyphens, use an underscore instead.",
                None,
            ));
        }

        let body = self.parse_statements(vec!["name:endblock".into()], true)?;

        if required {
            for body_node in &body {
                let ok = match body_node {
                    Av::Node("Output", _, args) => match args.first() {
                        Some(Av::List(items)) => items.iter().all(|output_node| {
                            match output_node {
                                Av::Node("TemplateData", _, td_args) => {
                                    match td_args.first() {
                                        Some(Av::Str(data)) => python_isspace(data),
                                        _ => false,
                                    }
                                }
                                _ => false,
                            }
                        }),
                        _ => true,
                    },
                    _ => false,
                };
                if !ok {
                    return Err(self.fail(
                        "Required blocks can only contain comments or whitespace",
                        None,
                    ));
                }
            }
        }

        // optional matching endblock name: {% endblock foo %}
        let expr = format!("name:{}", name);
        self.skip_if(&expr);
        Ok(Av::node(
            "Block",
            Some(lineno),
            vec![Av::str_(name), Av::List(body), Av::Bool(scoped), Av::Bool(required)],
        ))
    }

    fn parse_extends(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let template = self.parse_expression(true)?;
        Ok(Av::node("Extends", Some(lineno), vec![template]))
    }

    /// `Parser.parse_import_context` -- computes the with_context flag.
    fn import_context(&mut self, default: bool) -> Result<bool, ParseErr> {
        let cur = self.current()?;
        if (cur.test("name:with") || cur.test("name:without")) && self.look()?.test("name:context")
        {
            let tok = self.next_tok()?;
            let is_with = tok.str_value() == Some("with");
            self.skip(1)?; // consume the "context" token
            Ok(is_with)
        } else {
            Ok(default)
        }
    }

    fn parse_include(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let template = self.parse_expression(true)?;
        let ignore_missing =
            if self.current()?.test("name:ignore") && self.look()?.test("name:missing") {
                self.skip(2)?;
                true
            } else {
                false
            };
        let with_context = self.import_context(true)?;
        Ok(Av::node(
            "Include",
            Some(lineno),
            vec![template, Av::Bool(with_context), Av::Bool(ignore_missing)],
        ))
    }

    fn parse_import(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let template = self.parse_expression(true)?;
        self.expect("name:as")?;
        let target = self.parse_assign_target(true, false, None, false)?;
        let target_name = av_name(&target).unwrap_or_default().to_string();
        let with_context = self.import_context(false)?;
        Ok(Av::node(
            "Import",
            Some(lineno),
            vec![template, Av::str_(target_name), Av::Bool(with_context)],
        ))
    }

    fn parse_from_import(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let template = self.parse_expression(true)?;
        self.expect("name:import")?;
        let mut names: Vec<Av> = Vec::new();
        let mut with_context = false;
        loop {
            if !names.is_empty() {
                self.expect("comma")?;
            }
            if self.current()?.ttype == "name" {
                if self.parse_context_flag(&mut with_context)? {
                    break;
                }
                let target = self.parse_assign_target(true, false, None, false)?;
                let tname = av_name(&target).unwrap_or_default().to_string();
                if tname.starts_with('_') {
                    return Err(self.fail_assert(
                        "names starting with an underline can not be imported",
                        av_lineno(&target),
                    ));
                }
                if self.skip_if("name:as")? {
                    let alias = self.parse_assign_target(true, false, None, false)?;
                    let aname = av_name(&alias).unwrap_or_default().to_string();
                    names.push(Av::Tuple(vec![Av::str_(tname), Av::str_(aname)]));
                } else {
                    names.push(Av::str_(tname));
                }
                if self.parse_context_flag(&mut with_context)? || self.current()?.ttype != "comma" {
                    break;
                }
            } else {
                // python: self.stream.expect("name") -- always fails here
                self.expect("name")?;
            }
        }
        Ok(Av::node(
            "FromImport",
            Some(lineno),
            vec![template, Av::List(names), Av::Bool(with_context)],
        ))
    }

    /// The nested `parse_context` closure inside parse_from.
    fn parse_context_flag(&mut self, wc: &mut bool) -> Result<bool, ParseErr> {
        let cur = self.current()?;
        let is_candidate = match cur.str_value() {
            Some(s) => s == "with" || s == "without",
            None => false,
        };
        if is_candidate && self.look()?.test("name:context") {
            let tok = self.next_tok()?;
            *wc = tok.str_value() == Some("with");
            self.skip(1)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// `Parser.parse_signature` -- fills args/defaults of a macro/callblock.
    fn parse_signature(&mut self) -> Result<(Vec<Av>, Vec<Av>), ParseErr> {
        let mut args: Vec<Av> = Vec::new();
        let mut defaults: Vec<Av> = Vec::new();
        self.expect("lparen")?;
        while self.current()?.ttype != "rparen" {
            if !args.is_empty() {
                self.expect("comma")?;
            }
            let mut arg = self.parse_assign_target(true, false, None, false)?;
            set_ctx(&mut arg, "param");
            if self.skip_if("assign")? {
                defaults.push(self.parse_expression(true)?);
            } else if !defaults.is_empty() {
                return Err(self.fail(
                    "non-default argument follows default argument",
                    None,
                ));
            }
            args.push(arg);
        }
        self.expect("rparen")?;
        Ok((args, defaults))
    }

    fn parse_call_block(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let (args, defaults): (Vec<Av>, Vec<Av>) = if self.current()?.ttype == "lparen" {
            self.parse_signature()?
        } else {
            (Vec::new(), Vec::new())
        };
        let call_node = self.parse_expression(true)?;
        let is_call = matches!(&call_node, Av::Node("Call", _, _));
        if !is_call {
            return Err(self.fail("expected call", Some(lineno)));
        }
        let body = self.parse_statements(vec!["name:endcall".into()], true)?;
        Ok(Av::node(
            "CallBlock",
            Some(lineno),
            vec![call_node, Av::List(args), Av::List(defaults), Av::List(body)],
        ))
    }

    fn parse_filter_block(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let filter = self.parse_filter(None, true)?;
        let body = self.parse_statements(vec!["name:endfilter".into()], true)?;
        Ok(Av::node(
            "FilterBlock",
            Some(lineno),
            vec![Av::List(body), filter.unwrap_or(Av::NoneV)],
        ))
    }

    fn parse_macro(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let name_target = self.parse_assign_target(true, false, None, false)?;
        let name = av_name(&name_target).unwrap_or_default().to_string();
        let (args, defaults) = self.parse_signature()?;
        let body = self.parse_statements(vec!["name:endmacro".into()], true)?;
        Ok(Av::node(
            "Macro",
            Some(lineno),
            vec![Av::str_(name), Av::List(args), Av::List(defaults), Av::List(body)],
        ))
    }

    fn parse_print(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.next_tok()?.lineno;
        let mut items: Vec<Av> = Vec::new();
        while self.current()?.ttype != "block_end" {
            if !items.is_empty() {
                self.expect("comma")?;
            }
            items.push(self.parse_expression(true)?);
        }
        Ok(Av::node("Output", Some(lineno), vec![Av::List(items)]))
    }

    fn parse_assign_target(
        &mut self,
        name_only: bool,
        with_tuple: bool,
        extra_end_rules: Option<Vec<String>>,
        with_namespace: bool,
    ) -> Result<Av, ParseErr> {
        let mut target: Av;
        if name_only {
            let token = self.expect("name")?;
            let v = token.str_value().unwrap_or_default().to_string();
            target = Av::node(
                "Name",
                Some(token.lineno),
                vec![Av::str_(v), Av::str_("store")],
            );
        } else {
            if with_tuple {
                target = self.parse_tuple(
                    true,
                    true,
                    extra_end_rules,
                    false,
                    with_namespace,
                )?;
            } else {
                target = self.parse_primary(with_namespace)?;
            }
            set_ctx(&mut target, "store");
        }
        if !can_assign(&target) {
            let kind = match &target {
                Av::Node(n, _, _) => n.to_lowercase(),
                _ => String::from("node"),
            };
            return Err(self.fail(
                format!("can't assign to {}", py_repr_str(&kind)),
                av_lineno(&target),
            ));
        }
        Ok(target)
    }

    // -------------------------------------------------------------------
    // expression grammar (precedence chain from parser.py)
    // -------------------------------------------------------------------

    fn parse_expression(&mut self, with_condexpr: bool) -> Result<Av, ParseErr> {
        if with_condexpr {
            self.parse_condexpr()
        } else {
            self.parse_or()
        }
    }

    fn parse_condexpr(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut expr1 = self.parse_or()?;
        while self.skip_if("name:if")? {
            let expr2 = self.parse_or()?;
            let expr3 = if self.skip_if("name:else")? {
                Box::new(Some(self.parse_condexpr()?))
            } else {
                Box::new(None)
            };
            let e3 = match *expr3 {
                Some(e) => e,
                None => Av::NoneV,
            };
            expr1 = Av::node("CondExpr", lineno, vec![expr2, std::mem::replace(&mut expr1, Av::NoneV), e3]);
            lineno = Some(self.current()?.lineno);
        }
        Ok(expr1)
    }

    fn parse_or(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut left = self.parse_and()?;
        while self.skip_if("name:or")? {
            let right = self.parse_and()?;
            left = Av::node(
                "Or",
                lineno,
                vec![std::mem::replace(&mut left, Av::NoneV), right],
            );
            lineno = Some(self.current()?.lineno);
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut left = self.parse_not()?;
        while self.skip_if("name:and")? {
            let right = self.parse_not()?;
            left = Av::node(
                "And",
                lineno,
                vec![std::mem::replace(&mut left, Av::NoneV), right],
            );
            lineno = Some(self.current()?.lineno);
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Av, ParseErr> {
        if self.current()?.test("name:not") {
            let lineno = self.next_tok()?.lineno;
            let inner = self.parse_not()?;
            return Ok(Av::node("Not", Some(lineno), vec![inner]));
        }
        self.parse_compare()
    }

    fn parse_compare(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let expr = self.parse_math1()?;
        let mut ops: Vec<Av> = Vec::new();
        loop {
            let cur = self.current()?;
            let opname: Option<String> = match cur.ttype.as_str() {
                "eq" | "ne" | "lt" | "lteq" | "gt" | "gteq" => {
                    self.next_tok()?;
                    let rhs = self.parse_math1()?;
                    ops.push(Av::node(
                        "Operand",
                        None,
                        vec![Av::str_(cur.ttype.clone()), rhs],
                    ));
                    continue;
                }
                _ => None,
            };
            let _ = opname;
            if self.skip_if("name:in")? {
                let rhs = self.parse_math1()?;
                ops.push(Av::node(
                    "Operand",
                    None,
                    vec![Av::str_("in"), rhs],
                ));
            } else if self.current()?.test("name:not") && self.look()?.test("name:in") {
                self.skip(2)?;
                let rhs = self.parse_math1()?;
                ops.push(Av::node(
                    "Operand",
                    None,
                    vec![Av::str_("notin"), rhs],
                ));
            } else {
                break;
            }
            lineno = Some(self.current()?.lineno);
        }
        if ops.is_empty() {
            return Ok(expr);
        }
        Ok(Av::node("Compare", lineno, vec![expr, Av::List(ops)]))
    }

    fn parse_math1(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut left = self.parse_concat()?;
        loop {
            let cls = match self.current()?.ttype.as_str() {
                "add" => "Add",
                "sub" => "Sub",
                _ => break,
            };
            self.next_tok()?;
            let right = self.parse_concat()?;
            left = Av::node(cls, lineno, vec![std::mem::replace(&mut left, Av::NoneV), right]);
            lineno = Some(self.current()?.lineno);
        }
        Ok(left)
    }

    fn parse_concat(&mut self) -> Result<Av, ParseErr> {
        let lineno = Some(self.current()?.lineno);
        let mut args: Vec<Av> = vec![self.parse_math2()?];
        while self.current()?.ttype == "tilde" {
            self.next_tok()?;
            args.push(self.parse_math2()?);
        }
        if args.len() == 1 {
            return Ok(args.swap_remove(0));
        }
        Ok(Av::node("Concat", lineno, vec![Av::List(args)]))
    }

    fn parse_math2(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut left = self.parse_pow()?;
        loop {
            let cls = match self.current()?.ttype.as_str() {
                "mul" => "Mul",
                "div" => "Div",
                "floordiv" => "FloorDiv",
                "mod" => "Mod",
                _ => break,
            };
            self.next_tok()?;
            let right = self.parse_pow()?;
            left = Av::node(cls, lineno, vec![std::mem::replace(&mut left, Av::NoneV), right]);
            lineno = Some(self.current()?.lineno);
        }
        Ok(left)
    }

    fn parse_pow(&mut self) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut left = self.parse_unary(true)?;
        while self.current()?.ttype == "pow" {
            self.next_tok()?;
            let right = self.parse_unary(true)?;
            left = Av::node("Pow", lineno, vec![std::mem::replace(&mut left, Av::NoneV), right]);
            lineno = Some(self.current()?.lineno);
        }
        Ok(left)
    }

    fn parse_unary(&mut self, with_filter: bool) -> Result<Av, ParseErr> {
        let ttype = self.current()?.ttype.clone();
        let lineno = self.current()?.lineno;
        let mut node: Av = if ttype == "sub" {
            self.next_tok()?;
            let inner = self.parse_unary(false)?;
            Av::node("Neg", Some(lineno), vec![inner])
        } else if ttype == "add" {
            self.next_tok()?;
            let inner = self.parse_unary(false)?;
            Av::node("Pos", Some(lineno), vec![inner])
        } else {
            self.parse_primary(false)?
        };
        node = self.parse_postfix(node)?;
        if with_filter {
            node = self.parse_filter_expr(node)?;
        }
        Ok(node)
    }

    fn parse_primary(&mut self, with_namespace: bool) -> Result<Av, ParseErr> {
        let token = self.current()?;
        let node: Av;
        if token.ttype == "name" {
            self.next_tok()?;
            let v = token.str_value().unwrap_or_default().to_string();
            match v.as_str() {
                s @ ("true" | "false" | "True" | "False") => {
                    let b = s == "true" || s == "True";
                    node = Av::node(
                        "Const",
                        Some(token.lineno),
                        vec![Av::Bool(b)],
                    );
                }
                "none" | "None" => {
                    node = Av::node("Const", Some(token.lineno), vec![Av::NoneV])
                }
                _ => {
                    if with_namespace && self.current()?.ttype == "dot" {
                        self.next_tok()?;
                        let attr = self.expect("name")?;
                        let av = attr.str_value().unwrap_or_default().to_string();
                        node = Av::node(
                            "NSRef",
                            Some(token.lineno),
                            vec![Av::str_(v), Av::str_(av)],
                        );
                    } else {
                        node = Av::node(
                            "Name",
                            Some(token.lineno),
                            vec![Av::str_(v), Av::str_("load")],
                        );
                    }
                }
            }
        } else if token.ttype == "string" {
            self.next_tok()?;
            let mut parts: Vec<Av> = vec![pv_to_av(&token.pv)];
            while self.current()?.ttype == "string" {
                let t2 = self.current()?;
                parts.push(pv_to_av(&t2.pv));
                self.next_tok()?;
            }
            if parts.len() == 1 {
                node = Av::node("Const", Some(token.lineno), vec![parts.swap_remove(0)])
            } else {
                node = Av::node(
                    "Const",
                    Some(token.lineno),
                    vec![Av::Join(parts)],
                )
            }
        } else if token.ttype == "integer" || token.ttype == "float" {
            self.next_tok()?;
            node = Av::node("Const", Some(token.lineno), vec![pv_to_av(&token.pv)])
        } else if token.ttype == "lparen" {
            self.next_tok()?;
            node = self.parse_tuple(false, true, None, true, false)?;
            self.expect("rparen")?;
        } else if token.ttype == "lbracket" {
            node = self.parse_list()?;
        } else if token.ttype == "lbrace" {
            node = self.parse_dict()?;
        } else {
            return Err(self.fail(
                format!("unexpected {}", py_repr_str(&describe_token(&token))),
                Some(token.lineno),
            ));
        }
        Ok(node)
    }

    fn parse_tuple(
        &mut self,
        simplified: bool,
        with_condexpr: bool,
        extra_end_rules: Option<Vec<String>>,
        explicit_parentheses: bool,
        with_namespace: bool,
    ) -> Result<Av, ParseErr> {
        let mut lineno = Some(self.current()?.lineno);
        let mut args: Vec<Av> = Vec::new();
        let mut is_tuple = false;
        loop {
            if !args.is_empty() {
                self.expect("comma")?;
            }
            if self.is_tuple_end(extra_end_rules.as_deref()) {
                break;
            }
            let item = if simplified {
                self.parse_primary(with_namespace)?
            } else {
                self.parse_expression(with_condexpr)?
            };
            args.push(item);
            if self.current()?.ttype == "comma" {
                is_tuple = true;
            } else {
                break;
            }
            lineno = Some(self.current()?.lineno);
        }
        if !is_tuple {
            if !args.is_empty() {
                return Ok(args.swap_remove(0));
            }
            if !explicit_parentheses {
                let cur = self.current()?;
                return Err(self.fail(
                    format!("Expected an expression, got {}", py_repr_str(&describe_token(&cur))),
                    Some(cur.lineno),
                ));
            }
        }
        Ok(Av::node(
            "Tuple",
            lineno,
            vec![Av::List(args), Av::str_("load")],
        ))
    }

    fn parse_list(&mut self) -> Result<Av, ParseErr> {
        let token = self.expect("lbracket")?;
        let mut items: Vec<Av> = Vec::new();
        while self.current()?.ttype != "rbracket" {
            if !items.is_empty() {
                self.expect("comma")?;
            }
            if self.current()?.ttype == "rbracket" {
                break;
            }
            items.push(self.parse_expression(true)?);
        }
        self.expect("rbracket")?;
        Ok(Av::node("List", Some(token.lineno), vec![Av::List(items)]))
    }

    fn parse_dict(&mut self) -> Result<Av, ParseErr> {
        let token = self.expect("lbrace")?;
        let mut items: Vec<Av> = Vec::new();
        while self.current()?.ttype != "rbrace" {
            if !items.is_empty() {
                self.expect("comma")?;
            }
            if self.current()?.ttype == "rbrace" {
                break;
            }
            let key = self.parse_expression(true)?;
            self.expect("colon")?;
            let value = self.parse_expression(true)?;
            let kln = av_lineno(&key);
            items.push(Av::node("Pair", kln, vec![key, value]));
        }
        self.expect("rbrace")?;
        Ok(Av::node("Dict", Some(token.lineno), vec![Av::List(items)]))
    }

    fn parse_postfix(&mut self, mut node: Av) -> Result<Av, ParseErr> {
        loop {
            let ttype = self.current()?.ttype.clone();
            if ttype == "dot" || ttype == "lbracket" {
                node = self.parse_subscript(node)?;
            } else if ttype == "lparen" {
                node = self.parse_call(node)?;
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_filter_expr(&mut self, mut node: Av) -> Result<Av, ParseErr> {
        loop {
            let cur = self.current()?;
            if cur.ttype == "pipe" {
                let next = self.parse_filter(Some(node.clone()), false)?;
                match next {
                    Some(n) => node = n,
                    None => {}
                }
            } else if cur.ttype == "name" && cur.str_value() == Some("is") {
                node = self.parse_test(node)?;
            } else if cur.ttype == "lparen" {
                node = self.parse_call(node)?;
            } else {
                break;
            }
        }
        Ok(node)
    }

    fn parse_subscript(&mut self, node: Av) -> Result<Av, ParseErr> {
        let token = self.next_tok()?;
        if token.ttype == "dot" {
            let attr_token = self.current()?;
            self.next_tok()?;
            if attr_token.ttype == "name" {
                let v = attr_token.str_value().unwrap_or_default().to_string();
                return Ok(Av::node(
                    "Getattr",
                    Some(token.lineno),
                    vec![node, Av::str_(v), Av::str_("load")],
                ));
            } else if attr_token.ttype != "integer" {
                return Err(self.fail(
                    "expected name or number",
                    Some(attr_token.lineno),
                ));
            }
            let arg = Av::node(
                "Const",
                Some(attr_token.lineno),
                vec![pv_to_av(&attr_token.pv)],
            );
            return Ok(Av::node(
                "Getitem",
                Some(token.lineno),
                vec![node, arg, Av::str_("load")],
            ));
        }
        if token.ttype == "lbracket" {
            let mut args: Vec<Av> = Vec::new();
            while self.current()?.ttype != "rbracket" {
                if !args.is_empty() {
                    self.expect("comma")?;
                }
                args.push(self.parse_subscribed()?);
            }
            self.expect("rbracket")?;
            let arg: Av = if args.len() == 1 {
                args.swap_remove(0)
            } else {
                Av::node(
                    "Tuple",
                    Some(token.lineno),
                    vec![Av::List(args), Av::str_("load")],
                )
            };
            return Ok(Av::node(
                "Getitem",
                Some(token.lineno),
                vec![node, arg, Av::str_("load")],
            ));
        }
        Err(self.fail("expected subscript expression", Some(token.lineno)))
    }

    fn parse_subscribed(&mut self) -> Result<Av, ParseErr> {
        let lineno = self.current()?.lineno;
        // python builds a 3-element args list with None for missing parts.
        let mut args: Vec<Av> = Vec::new();
        if self.current()?.ttype == "colon" {
            self.next_tok()?;
            args.push(Av::NoneV);
        } else {
            let node = self.parse_expression(true)?;
            if self.current()?.ttype != "colon" {
                return Ok(node);
            }
            self.next_tok()?;
            args.push(node);
        }
        // second part
        if self.current()?.ttype == "colon" {
            args.push(Av::NoneV);
        } else if self.current()?.ttype != "rbracket" && self.current()?.ttype != "comma" {
            args.push(self.parse_expression(true)?);
        } else {
            args.push(Av::NoneV);
        }
        // third part
        if self.current()?.ttype == "colon" {
            self.next_tok()?;
            if self.current()?.ttype != "rbracket" && self.current()?.ttype != "comma" {
                args.push(self.parse_expression(true)?);
            } else {
                args.push(Av::NoneV);
            }
        } else {
            args.push(Av::NoneV);
        }
        Ok(Av::node(
            "Slice",
            Some(lineno),
            vec![
                std::mem::replace(&mut args[0], Av::NoneV),
                std::mem::replace(&mut args[1], Av::NoneV),
                std::mem::replace(&mut args[2], Av::NoneV),
            ],
        ))
    }

    fn parse_call_args(&mut self) -> Result<(Vec<Av>, Vec<Av>, Option<Av>, Option<Av>), ParseErr> {
        let token = self.expect("lparen")?;
        let mut args: Vec<Av> = Vec::new();
        let mut kwargs: Vec<Av> = Vec::new();
        let mut dyn_args: Option<Av> = None;
        let mut dyn_kwargs: Option<Av> = None;
        let mut require_comma = false;

        macro_rules! ensure {
            ($cond:expr) => {
                if !$cond {
                    return Err(self.fail(
                        "invalid syntax for function call expression",
                        Some(token.lineno),
                    ));
                }
            };
        }

        while self.current()?.ttype != "rparen" {
            if require_comma {
                self.expect("comma")?;
                if self.current()?.ttype == "rparen" {
                    break;
                }
            }
            let cur = self.current()?;
            if cur.ttype == "mul" {
                ensure!(dyn_args.is_none() && dyn_kwargs.is_none());
                self.next_tok()?;
                dyn_args = Some(self.parse_expression(true)?);
            } else if cur.ttype == "pow" {
                ensure!(dyn_kwargs.is_none());
                self.next_tok()?;
                dyn_kwargs = Some(self.parse_expression(true)?);
            } else if cur.ttype == "name" && self.look()?.ttype == "assign" {
                ensure!(dyn_kwargs.is_none());
                let key = cur.str_value().unwrap_or_default().to_string();
                self.skip(2)?;
                let value = self.parse_expression(true)?;
                let vln = av_lineno(&value);
                kwargs.push(Av::node("Keyword", vln, vec![Av::str_(key), value]));
            } else {
                ensure!(dyn_args.is_none() && dyn_kwargs.is_none() && kwargs.is_empty());
                args.push(self.parse_expression(true)?);
            }
            require_comma = true;
        }
        self.expect("rparen")?;
        Ok((args, kwargs, dyn_args, dyn_kwargs))
    }

    fn parse_call(&mut self, node: Av) -> Result<Av, ParseErr> {
        // lineno recorded before the stream advances (python comment).
        let token = self.current()?;
        let (args, kwargs, dyn_args, dyn_kwargs) = self.parse_call_args()?;
        Ok(Av::node(
            "Call",
            Some(token.lineno),
            vec![
                node,
                Av::List(args),
                Av::List(kwargs),
                dyn_args.unwrap_or(Av::NoneV),
                dyn_kwargs.unwrap_or(Av::NoneV),
            ],
        ))
    }

    fn parse_filter(
        &mut self,
        mut node: Option<Av>,
        mut start_inline: bool,
    ) -> Result<Option<Av>, ParseErr> {
        while self.current()?.ttype == "pipe" || start_inline {
            if !start_inline {
                self.next_tok()?;
            }
            let token = self.expect("name")?;
            let mut name = token.str_value().unwrap_or_default().to_string();
            while self.current()?.ttype == "dot" {
                self.next_tok()?;
                let part = self.expect("name")?;
                name.push('.');
                name.push_str(part.str_value().unwrap_or_default());
            }
            let (args, kwargs, dyn_args, dyn_kwargs) = if self.current()?.ttype == "lparen" {
                self.parse_call_args()?
            } else {
                (Vec::new(), Vec::new(), None, None)
            };
            node = Some(Av::node(
                "Filter",
                Some(token.lineno),
                vec![
                    node.unwrap_or(Av::NoneV),
                    Av::str_(name),
                    Av::List(args),
                    Av::List(kwargs),
                    dyn_args.unwrap_or(Av::NoneV),
                    dyn_kwargs.unwrap_or(Av::NoneV),
                ],
            ));
            start_inline = false;
        }
        Ok(node)
    }

    fn parse_test(&mut self, node: Av) -> Result<Av, ParseErr> {
        let token = self.next_tok()?; // the "is" token
        let negated = if self.current()?.test("name:not") {
            self.next_tok()?;
            true
        } else {
            false
        };
        let mut name = self.expect("name")?.str_value().unwrap_or_default().to_string();
        while self.current()?.ttype == "dot" {
            self.next_tok()?;
            let part = self.expect("name")?;
            name.push('.');
            name.push_str(part.str_value().unwrap_or_default());
        }
        let mut dyn_args: Option<Av> = None;
        let mut dyn_kwargs: Option<Av> = None;
        let mut kwargs: Vec<Av> = Vec::new();
        let args: Vec<Av> = if self.current()?.ttype == "lparen" {
            let (a, k, da, dk) = self.parse_call_args()?;
            dyn_args = da;
            dyn_kwargs = dk;
            kwargs = k;
            a
        } else if [
            "name",
            "string",
            "integer",
            "float",
            "lparen",
            "lbracket",
            "lbrace",
        ]
        .contains(&self.current()?.ttype.as_str())
            && !{ let c = self.current()?; c.test_any(&vec![
                    "name:else".into(),
                    "name:or".into(),
                    "name:and".into(),
                ]) }
        {
            if self.current()?.test("name:is") {
                return Err(self.fail(
                    "You cannot chain multiple tests with is",
                    None,
                ));
            }
            let mut arg_node = self.parse_primary(false)?;
            arg_node = self.parse_postfix(arg_node)?;
            vec![arg_node]
        } else {
            Vec::new()
        };
        let mut result = Av::node(
            "Test",
            Some(token.lineno),
            vec![
                node,
                Av::str_(name),
                Av::List(args),
                Av::List(kwargs),
                dyn_args.unwrap_or(Av::NoneV),
                dyn_kwargs.unwrap_or(Av::NoneV),
            ],
        );
        if negated {
            result = Av::node("Not", Some(token.lineno), vec![result]);
        }
        Ok(result)
    }

    fn subparse(&mut self, end_tokens: Option<&[String]>) -> Result<Vec<Av>, ParseErr> {
        if let Some(et) = end_tokens {
            self.end_token_stack.push(et.to_vec());
        }
        let mut body: Vec<Av> = Vec::new();
        let mut data_buffer: Vec<Av> = Vec::new();

        let outcome = self.subparse_loop(&mut body, &mut data_buffer, end_tokens);
        if end_tokens.is_some() {
            self.end_token_stack.pop();
        }
        outcome
    }

    fn subparse_loop(
        &mut self,
        body: &mut Vec<Av>,
        data_buffer: &mut Vec<Av>,
        end_tokens: Option<&[String]>,
    ) -> Result<Vec<Av>, ParseErr> {
        // python `while self.stream:` -- stream truthy == not at EOF.
        while self.current()?.ttype != "eof" {
            let token = self.current()?;
            if token.ttype == "data" {
                let nonempty = match &token.pv {
                    PV::Str(s) => !s.is_empty(),
                    // surrogate-fallback strings are non-empty by construction
                    PV::Num(_) => true,
                };
                if nonempty {
                    data_buffer.push(Av::node(
                        "TemplateData",
                        Some(token.lineno),
                        vec![pv_to_av(&token.pv)],
                    ));
                }
                self.next_tok()?;
            } else if token.ttype == "variable_begin" {
                self.next_tok()?;
                let t = self.parse_tuple(false, true, None, false, false)?;
                data_buffer.push(t);
                self.expect("variable_end")?;
            } else if token.ttype == "block_begin" {
                flush_data(body, data_buffer)?;
                self.next_tok()?;
                if let Some(et) = end_tokens {
                    if self.current()?.test_any(&et.iter().cloned().collect::<Vec<String>>()) {
                        // python returns body here without flushing pending
                        // data (there is none: flush happened above).
                        return Ok(body.clone());
                    }
                }
                let rv = self.parse_statement()?;
                body.extend(rv);
                self.expect("block_end")?;
            } else {
                return Err(ParseErr::syntax("internal parsing error", None));
            }
        }
        flush_data(body, data_buffer)?;
        Ok(body.clone())
    }

    /// `Parser.parse` -- the whole template.
    pub fn parse(&mut self) -> Result<Av, ParseErr> {
        let body = self.subparse(None)?;
        Ok(Av::node("Template", Some(1), vec![Av::List(body)]))
    }
}

fn flush_data(body: &mut Vec<Av>, data_buffer: &mut Vec<Av>) -> Result<(), ParseErr> {
    if !data_buffer.is_empty() {
        let lineno = av_lineno(&data_buffer[0]);
        body.push(Av::node(
            "Output",
            lineno,
            vec![Av::List(std::mem::take(data_buffer))],
        ));
    }
    Ok(())
}

fn pv_to_av(pv: &PV) -> Av {
    match pv {
        PV::Str(s) => Av::Str(s.clone()),
        PV::Num(o) => Av::Val(o.clone()),
    }
}
