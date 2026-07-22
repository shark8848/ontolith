//! Shared RDF term / Turtle token helpers.

use ontolith_core::domain::{Iri, LiteralValue, NodeId};
use ontolith_core::error::OntolithError;
use ontolith_rdf::domain::Term;
use ontolith_storage::application::DictionaryCodec;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PrefixMap {
    pub base: Option<String>,
    pub prefixes: HashMap<String, String>,
    blank_counter: u64,
}

impl Default for PrefixMap {
    fn default() -> Self {
        let mut prefixes = HashMap::new();
        // Common defaults useful in tests and documents without explicit prefix.
        prefixes.insert(
            "rdf".to_owned(),
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#".to_owned(),
        );
        prefixes.insert(
            "rdfs".to_owned(),
            "http://www.w3.org/2000/01/rdf-schema#".to_owned(),
        );
        prefixes.insert(
            "xsd".to_owned(),
            "http://www.w3.org/2001/XMLSchema#".to_owned(),
        );
        Self {
            base: None,
            prefixes,
            blank_counter: 0,
        }
    }
}

impl PrefixMap {
    pub fn with_base(base: Option<String>) -> Self {
        Self {
            base,
            ..Self::default()
        }
    }

    pub fn set_prefix(&mut self, prefix: impl Into<String>, iri: impl Into<String>) {
        self.prefixes.insert(prefix.into(), iri.into());
    }

    pub fn set_base(&mut self, base: impl Into<String>) {
        self.base = Some(base.into());
    }

    pub fn mint_blank(&mut self) -> String {
        self.blank_counter += 1;
        format!("b{}", self.blank_counter)
    }

    pub fn expand_prefixed(
        &self,
        token: &str,
        line: usize,
        col: usize,
    ) -> Result<String, OntolithError> {
        if token == "a" {
            return Ok("http://www.w3.org/1999/02/22-rdf-syntax-ns#type".to_owned());
        }
        if let Some((prefix, local)) = token.split_once(':') {
            if let Some(ns) = self.prefixes.get(prefix) {
                return Ok(format!("{ns}{local}"));
            }
            return Err(OntolithError::parse_at(
                line,
                col,
                format!("unknown prefix '{prefix}'"),
            ));
        }
        // relative against base
        if let Some(base) = &self.base
            && !token.contains(':')
        {
            return Ok(resolve_against_base(base, token));
        }
        Ok(token.to_owned())
    }

    pub fn expand_iri_ref(&self, iri: &str) -> String {
        if iri.contains(':') {
            iri.to_owned()
        } else if let Some(base) = &self.base {
            resolve_against_base(base, iri)
        } else {
            iri.to_owned()
        }
    }
}

fn resolve_against_base(base: &str, relative: &str) -> String {
    if relative.is_empty() {
        return base.to_owned();
    }
    if base.ends_with('/') || base.ends_with('#') {
        format!("{base}{relative}")
    } else if let Some(pos) = base.rfind('/') {
        format!("{}{relative}", &base[..=pos])
    } else {
        format!("{base}{relative}")
    }
}

pub fn unescape_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(v) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(v)
                {
                    out.push(ch);
                    continue;
                }
                out.push_str(&hex);
            }
            Some('U') => {
                let hex: String = chars.by_ref().take(8).collect();
                if let Ok(v) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(v)
                {
                    out.push(ch);
                    continue;
                }
                out.push_str(&hex);
            }
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

pub fn coerce_typed_literal(content: String, datatype: &str) -> LiteralValue {
    match datatype {
        "http://www.w3.org/2001/XMLSchema#integer"
        | "http://www.w3.org/2001/XMLSchema#int"
        | "http://www.w3.org/2001/XMLSchema#long" => content
            .parse::<i64>()
            .map(LiteralValue::Integer)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#double"
        | "http://www.w3.org/2001/XMLSchema#float"
        | "http://www.w3.org/2001/XMLSchema#decimal" => content
            .parse::<f64>()
            .map(LiteralValue::Decimal)
            .unwrap_or(LiteralValue::String(content)),
        "http://www.w3.org/2001/XMLSchema#boolean" => match content.as_str() {
            "true" | "1" => LiteralValue::Boolean(true),
            "false" | "0" => LiteralValue::Boolean(false),
            _ => LiteralValue::String(content),
        },
        _ => LiteralValue::String(content),
    }
}

pub fn subject_node(
    dictionary: &dyn DictionaryCodec,
    prefixes: &PrefixMap,
    token: &TurtleTerm,
    line: usize,
    col: usize,
) -> Result<NodeId, OntolithError> {
    match token {
        TurtleTerm::IriRef(s) => {
            let iri = prefixes.expand_iri_ref(s);
            Ok(dictionary.encode_node(&iri))
        }
        TurtleTerm::Prefixed(s) | TurtleTerm::Bare(s) => {
            let iri = prefixes.expand_prefixed(s, line, col)?;
            Ok(dictionary.encode_node(&iri))
        }
        TurtleTerm::Blank(label) => Ok(dictionary.encode_node(&format!("_:{label}"))),
        TurtleTerm::Literal { .. } => Err(OntolithError::parse_at(
            line,
            col,
            "subject cannot be a literal",
        )),
    }
}

pub fn predicate_iri(
    prefixes: &PrefixMap,
    token: &TurtleTerm,
    line: usize,
    col: usize,
) -> Result<Iri, OntolithError> {
    let s = match token {
        TurtleTerm::IriRef(s) => prefixes.expand_iri_ref(s),
        TurtleTerm::Prefixed(s) | TurtleTerm::Bare(s) => prefixes.expand_prefixed(s, line, col)?,
        TurtleTerm::Blank(_) | TurtleTerm::Literal { .. } => {
            return Err(OntolithError::parse_at(
                line,
                col,
                "predicate must be an IRI",
            ));
        }
    };
    Iri::parse(s).map_err(|e| OntolithError::parse_at(line, col, e.message()))
}

pub fn object_term(
    dictionary: &dyn DictionaryCodec,
    prefixes: &PrefixMap,
    token: &TurtleTerm,
    line: usize,
    col: usize,
) -> Result<Term, OntolithError> {
    match token {
        TurtleTerm::IriRef(s) => {
            let iri = prefixes.expand_iri_ref(s);
            Ok(Term::Iri(Iri::parse(iri).map_err(|e| {
                OntolithError::parse_at(line, col, e.message())
            })?))
        }
        TurtleTerm::Prefixed(s) | TurtleTerm::Bare(s) => {
            let iri = prefixes.expand_prefixed(s, line, col)?;
            Ok(Term::Iri(Iri::parse(iri).map_err(|e| {
                OntolithError::parse_at(line, col, e.message())
            })?))
        }
        TurtleTerm::Blank(label) => Ok(Term::BlankNode(
            dictionary.encode_node(&format!("_:{label}")),
        )),
        TurtleTerm::Literal {
            value,
            language,
            datatype,
        } => {
            if language.is_some() {
                return Ok(Term::Literal(LiteralValue::String(value.clone())));
            }
            if let Some(dt) = datatype {
                let dt_iri = if dt.starts_with('<') {
                    prefixes.expand_iri_ref(dt.trim_matches(|c| c == '<' || c == '>'))
                } else {
                    prefixes.expand_prefixed(dt, line, col)?
                };
                return Ok(Term::Literal(coerce_typed_literal(value.clone(), &dt_iri)));
            }
            // bare numbers / booleans already normalized into value+datatype by lexer
            Ok(Term::Literal(LiteralValue::String(value.clone())))
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurtleTerm {
    IriRef(String),
    Prefixed(String),
    Bare(String),
    Blank(String),
    Literal {
        value: String,
        language: Option<String>,
        datatype: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Term(TurtleTerm),
    Prefix,
    Base,
    AtPrefix,
    AtBase,
    Dot,
    Semicolon,
    Comma,
    OpenBracket,
    CloseBracket,
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    A,
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    pub pos: usize,
    pub line: usize,
    pub col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while let Some(c) = self.peek_char() {
                if c == ' ' || c == '\t' || c == '\r' || c == '\n' {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.peek_char() == Some('#') {
                while let Some(c) = self.peek_char() {
                    self.bump();
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    pub fn next_token(&mut self) -> Result<Tok, OntolithError> {
        self.skip_ws_and_comments();
        let line = self.line;
        let col = self.col;
        let Some(c) = self.peek_char() else {
            return Ok(Tok::Eof);
        };
        match c {
            '.' => {
                self.bump();
                Ok(Tok::Dot)
            }
            ';' => {
                self.bump();
                Ok(Tok::Semicolon)
            }
            ',' => {
                self.bump();
                Ok(Tok::Comma)
            }
            '[' => {
                self.bump();
                Ok(Tok::OpenBracket)
            }
            ']' => {
                self.bump();
                Ok(Tok::CloseBracket)
            }
            '(' => {
                self.bump();
                Ok(Tok::OpenParen)
            }
            ')' => {
                self.bump();
                Ok(Tok::CloseParen)
            }
            '{' => {
                self.bump();
                Ok(Tok::OpenBrace)
            }
            '}' => {
                self.bump();
                Ok(Tok::CloseBrace)
            }
            '<' => self.lex_iri(line, col),
            '"' | '\'' => self.lex_literal(line, col),
            '_' => self.lex_blank(line, col),
            '@' => self.lex_at_keyword(line, col),
            _ => self.lex_word(line, col),
        }
    }

    fn lex_iri(&mut self, line: usize, col: usize) -> Result<Tok, OntolithError> {
        self.bump(); // <
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c == '>' {
                let iri = self.input[start..self.pos].to_owned();
                self.bump();
                return Ok(Tok::Term(TurtleTerm::IriRef(iri)));
            }
            if c == ' ' || c == '\n' || c == '\t' {
                return Err(OntolithError::parse_at(line, col, "whitespace in IRI"));
            }
            self.bump();
        }
        Err(OntolithError::parse_at(line, col, "unterminated IRI"))
    }

    fn lex_blank(&mut self, line: usize, col: usize) -> Result<Tok, OntolithError> {
        self.bump(); // _
        if self.peek_char() != Some(':') {
            return Err(OntolithError::parse_at(
                line,
                col,
                "expected '_:' blank node",
            ));
        }
        self.bump();
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(OntolithError::parse_at(line, col, "empty blank label"));
        }
        Ok(Tok::Term(TurtleTerm::Blank(
            self.input[start..self.pos].to_owned(),
        )))
    }

    fn lex_at_keyword(&mut self, line: usize, col: usize) -> Result<Tok, OntolithError> {
        self.bump(); // @
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphabetic() {
                self.bump();
            } else {
                break;
            }
        }
        let kw = self.input[start..self.pos].to_ascii_lowercase();
        match kw.as_str() {
            "prefix" => Ok(Tok::AtPrefix),
            "base" => Ok(Tok::AtBase),
            _ => Err(OntolithError::parse_at(
                line,
                col,
                format!("unknown @ directive '@{kw}'"),
            )),
        }
    }

    fn lex_literal(&mut self, line: usize, col: usize) -> Result<Tok, OntolithError> {
        let quote = self.bump().unwrap();
        let long =
            self.peek_char() == Some(quote) && self.input[self.pos..].chars().nth(1) == Some(quote);
        if long {
            self.bump();
            self.bump();
        }
        let start = self.pos;
        loop {
            let Some(c) = self.peek_char() else {
                return Err(OntolithError::parse_at(line, col, "unterminated string"));
            };
            if c == '\\' {
                self.bump();
                self.bump();
                continue;
            }
            if long {
                if c == quote {
                    let rest: String = self.input[self.pos..].chars().take(3).collect();
                    if rest.chars().filter(|&ch| ch == quote).count() >= 3 {
                        let raw = &self.input[start..self.pos];
                        self.bump();
                        self.bump();
                        self.bump();
                        return self.finish_literal(unescape_string(raw), line, col);
                    }
                }
                self.bump();
            } else if c == quote {
                let raw = &self.input[start..self.pos];
                self.bump();
                return self.finish_literal(unescape_string(raw), line, col);
            } else if c == '\n' {
                return Err(OntolithError::parse_at(
                    line,
                    col,
                    "newline in short string literal",
                ));
            } else {
                self.bump();
            }
        }
    }

    fn finish_literal(
        &mut self,
        value: String,
        line: usize,
        col: usize,
    ) -> Result<Tok, OntolithError> {
        let mut language = None;
        let mut datatype = None;
        if self.peek_char() == Some('@') {
            self.bump();
            let start = self.pos;
            while let Some(c) = self.peek_char() {
                if c.is_ascii_alphanumeric() || c == '-' {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.pos == start {
                return Err(OntolithError::parse_at(line, col, "empty language tag"));
            }
            language = Some(self.input[start..self.pos].to_ascii_lowercase());
        } else if self.input[self.pos..].starts_with("^^") {
            self.bump();
            self.bump();
            match self.next_token()? {
                Tok::Term(TurtleTerm::IriRef(s)) => datatype = Some(format!("<{s}>")),
                Tok::Term(TurtleTerm::Prefixed(s)) | Tok::Term(TurtleTerm::Bare(s)) => {
                    datatype = Some(s)
                }
                _ => {
                    return Err(OntolithError::parse_at(
                        line,
                        col,
                        "datatype after ^^ must be an IRI",
                    ));
                }
            }
        }
        Ok(Tok::Term(TurtleTerm::Literal {
            value,
            language,
            datatype,
        }))
    }

    fn lex_word(&mut self, line: usize, col: usize) -> Result<Tok, OntolithError> {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_whitespace()
                || matches!(
                    c,
                    ',' | ';' | '.' | '[' | ']' | '(' | ')' | '{' | '}' | '<' | '"' | '#'
                )
            {
                break;
            }
            self.bump();
        }
        let word = &self.input[start..self.pos];
        if word.is_empty() {
            return Err(OntolithError::parse_at(line, col, "unexpected character"));
        }
        let lower = word.to_ascii_lowercase();
        Ok(match lower.as_str() {
            "prefix" => Tok::Prefix,
            "base" => Tok::Base,
            "a" => Tok::A,
            "true" => Tok::Term(TurtleTerm::Literal {
                value: "true".into(),
                language: None,
                datatype: Some("<http://www.w3.org/2001/XMLSchema#boolean>".into()),
            }),
            "false" => Tok::Term(TurtleTerm::Literal {
                value: "false".into(),
                language: None,
                datatype: Some("<http://www.w3.org/2001/XMLSchema#boolean>".into()),
            }),
            _ if word.contains(':') => Tok::Term(TurtleTerm::Prefixed(word.to_owned())),
            _ if is_number(word) => {
                let dt = if word.contains('.') || word.contains('e') || word.contains('E') {
                    "<http://www.w3.org/2001/XMLSchema#double>"
                } else {
                    "<http://www.w3.org/2001/XMLSchema#integer>"
                };
                Tok::Term(TurtleTerm::Literal {
                    value: word.to_owned(),
                    language: None,
                    datatype: Some(dt.into()),
                })
            }
            _ => Tok::Term(TurtleTerm::Bare(word.to_owned())),
        })
    }
}

fn is_number(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    if matches!(chars.peek(), Some('+') | Some('-')) {
        chars.next();
    }
    let mut saw_digit = false;
    let mut saw_dot = false;
    for c in chars {
        if c.is_ascii_digit() {
            saw_digit = true;
        } else if c == '.' && !saw_dot {
            saw_dot = true;
        } else if (c == 'e' || c == 'E') && saw_digit {
            return s[1..].chars().any(|ch| ch.is_ascii_digit()) || saw_digit;
        } else {
            return false;
        }
    }
    saw_digit
}
