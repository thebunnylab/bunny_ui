//! The reference scanner — which functions of a crate are previews.
//!
//! A preview is a free function with no parameters that carries the
//! `#[preview]` attribute (by any path whose last segment is `preview`:
//! `#[preview]`, `#[preview(..)]`, `#[bunny_ui::preview]`, a renamed
//! crate), or that `declare_preview!` names. The scan reads the source
//! for it: a scanner that reads symbols would need a build, and one that
//! reads tokens needs no parser — a lexer that knows strings, comments
//! and brackets is enough to find an attribute and the item under it.
//!
//! What it refuses, and says where: an attribute on something that is
//! not a function, on an associated function (inside `impl` or `trait`),
//! on a function with parameters or type parameters, on an `async`,
//! `unsafe` or `extern` one, an argument it does not know, an argument
//! that is not `name = "…"`. `#[cfg_attr(…, preview)]` is not a preview:
//! a preview behind a cfg is one the scan could not list without
//! evaluating the cfg, and the honest answer is not to pretend.
//!
//! The fixtures under `tests/scan/` and their expected JSON are the
//! contract: Trinity's tree-sitter scan lists the same previews, at the
//! same sites, with the same refusals, or one of the two is wrong.
//!
//! ## Wiring
//!
//! [`scan_source`] reads one file; [`scan_dir`] walks a crate's `src/`
//! (every `.rs`, sorted) and folds the files into one [`ScanReport`],
//! naming the ident two files both claim as a [`Duplicate`] — the
//! alphabetically first file keeps it, since one symbol cannot answer
//! for two functions.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// One preview the scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewRef {
    /// The function's name.
    pub ident: String,
    /// The symbol a preview build exports for it.
    pub symbol: String,
    /// The component's name (`name = "…"`; the ident otherwise).
    pub name: String,
    /// The gallery section (`scope = "…"`; empty otherwise).
    pub scope: String,
    /// This preview's label (`example = "…"`; the ident otherwise).
    pub example: String,
    /// The file, as the scan was told to name it.
    pub file: String,
    /// The line of the function's first token after its attributes, 1-based.
    pub line: u32,
    /// Its column, 1-based, in characters.
    pub column: u32,
}

/// Where something stood.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Site {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

/// Why an attribute was not a preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// `app` belongs to the whole-app entry exported by `app!`.
    Reserved,
    /// `#[preview]` on something that is not a free function.
    NotAFunction,
    /// On a function inside `impl` or `trait`.
    Associated,
    /// The function takes parameters.
    Parameters,
    /// The function takes type parameters.
    Generic,
    /// The function is `async`, `unsafe` or `extern`.
    Qualifier(String),
    /// An argument the attribute does not take.
    UnknownArgument(String),
    /// An argument that is not `name = "…"` (or `crate = path`).
    BadArgument(String),
}

impl Reason {
    /// The stable spelling the JSON carries.
    pub fn tag(&self) -> String {
        match self {
            Reason::Reserved => String::from("reserved"),
            Reason::NotAFunction => String::from("not_a_function"),
            Reason::Associated => String::from("associated"),
            Reason::Parameters => String::from("parameters"),
            Reason::Generic => String::from("generic"),
            Reason::Qualifier(word) => format!("qualifier:{word}"),
            Reason::UnknownArgument(arg) => format!("unknown_argument:{arg}"),
            Reason::BadArgument(arg) => format!("bad_argument:{arg}"),
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::Reserved => f.write_str("is reserved for the App scene exported by `app!`; rename the preview"),
            Reason::NotAFunction => f.write_str("`#[preview]` must sit on a free function"),
            Reason::Associated => f.write_str("an associated function; a preview is a free function"),
            Reason::Parameters => f.write_str("takes parameters; a preview takes none"),
            Reason::Generic => f.write_str("is generic; a preview is called with no type"),
            Reason::Qualifier(word) => write!(f, "is `{word}`; a preview is a plain function"),
            Reason::UnknownArgument(arg) => write!(f, "`#[preview]` does not take `{arg}`"),
            Reason::BadArgument(arg) => write!(f, "`{arg}` must be `{arg} = \"…\"`"),
        }
    }
}

/// An attribute the scan refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    /// The attribute's site, or the function's when the function is what is wrong.
    pub site: Site,
    /// The function, when there was one.
    pub ident: Option<String>,
    pub reason: Reason,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.ident {
            Some(ident) if !matches!(self.reason, Reason::NotAFunction | Reason::UnknownArgument(_) | Reason::BadArgument(_)) => {
                write!(f, "{}: `{ident}` {}", self.site, self.reason)
            }
            _ => write!(f, "{}: {}", self.site, self.reason),
        }
    }
}

/// One ident two files both claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Duplicate {
    pub ident: String,
    pub kept: Site,
    pub dropped: Site,
}

/// What a scan found and refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// The first whole-app invocation, when this crate has one.
    pub app: Option<Site>,
    /// The previews, in file then position order.
    pub previews: Vec<PreviewRef>,
    /// What was refused, in position order.
    pub refused: Vec<Refusal>,
    /// The idents claimed twice.
    pub duplicates: Vec<Duplicate>,
    /// Files that could not be read, with the error.
    pub unreadable: Vec<(String, String)>,
}

impl ScanReport {
    /// Folds `other` in after this one: a preview whose ident this report
    /// already holds becomes a duplicate.
    pub fn merge(&mut self, other: ScanReport) {
        let mut seen: HashMap<String, Site> = self
            .previews
            .iter()
            .map(|preview| (preview.ident.clone(), site_of(preview)))
            .collect();
        for preview in other.previews {
            match seen.get(&preview.ident) {
                Some(kept) => self.duplicates.push(Duplicate { ident: preview.ident.clone(), kept: kept.clone(), dropped: site_of(&preview) }),
                None => {
                    seen.insert(preview.ident.clone(), site_of(&preview));
                    self.previews.push(preview);
                }
            }
        }
        if let Some(site) = other.app {
            self.record_app(site);
        }
        self.refused.extend(other.refused);
        self.duplicates.extend(other.duplicates);
        self.unreadable.extend(other.unreadable);
    }

    fn record_app(&mut self, site: Site) {
        match &self.app {
            Some(kept) => self.duplicates.push(Duplicate { ident: String::from(crate::wire::APP_IDENT), kept: kept.clone(), dropped: site }),
            None => self.app = Some(site),
        }
    }

    /// The report as JSON — the shape the fixtures are committed in, one
    /// object per line.
    pub fn json(&self) -> String {
        let mut out = String::from("{\n  \"previews\": [");
        rows(&mut out, &self.previews, |out, p| {
            out.push_str(&format!(
                "{{\"ident\": {}, \"symbol\": {}, \"name\": {}, \"scope\": {}, \"example\": {}, \"file\": {}, \"line\": {}, \"column\": {}}}",
                quote(&p.ident), quote(&p.symbol), quote(&p.name), quote(&p.scope), quote(&p.example), quote(&p.file), p.line, p.column
            ));
        });
        out.push_str("],\n  \"refused\": [");
        rows(&mut out, &self.refused, |out, r| {
            let ident = r.ident.as_deref().map_or_else(|| String::from("null"), quote);
            out.push_str(&format!(
                "{{\"file\": {}, \"line\": {}, \"column\": {}, \"ident\": {ident}, \"reason\": {}}}",
                quote(&r.site.file), r.site.line, r.site.column, quote(&r.reason.tag())
            ));
        });
        out.push_str("],\n  \"duplicates\": [");
        rows(&mut out, &self.duplicates, |out, d| {
            out.push_str(&format!(
                "{{\"ident\": {}, \"kept\": {}, \"dropped\": {}}}",
                quote(&d.ident), quote(&d.kept.to_string()), quote(&d.dropped.to_string())
            ));
        });
        out.push_str("],\n  \"unreadable\": [");
        rows(&mut out, &self.unreadable, |out, (file, error)| {
            out.push_str(&format!("{{\"file\": {}, \"error\": {}}}", quote(file), quote(error)));
        });
        out.push(']');
        if let Some(site) = &self.app {
            out.push_str(&format!(
                ",\n  \"app\": {{\"file\": {}, \"line\": {}, \"column\": {}}}",
                quote(&site.file),
                site.line,
                site.column
            ));
        }
        out.push_str("\n}\n");
        out
    }
}

fn site_of(preview: &PreviewRef) -> Site {
    Site { file: preview.file.clone(), line: preview.line, column: preview.column }
}

fn rows<T>(out: &mut String, items: &[T], mut row: impl FnMut(&mut String, &T)) {
    for (index, item) in items.iter().enumerate() {
        out.push_str(if index == 0 { "\n    " } else { ",\n    " });
        row(out, item);
    }
    if !items.is_empty() {
        out.push_str("\n  ");
    }
}

/// A JSON string literal.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The symbol a preview build exports for `ident`.
pub fn symbol(ident: &str) -> String {
    format!("bunny_preview__{ident}")
}

/// Scans a crate: every `.rs` under `root`, sorted by path, named
/// relative to `root` with `/` between the parts.
pub fn scan_dir(root: &Path) -> ScanReport {
    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort();
    let mut report = ScanReport::default();
    for relative in files {
        let path = root.join(&relative);
        let name = relative.replace('\\', "/");
        match std::fs::read_to_string(&path) {
            Ok(source) => report.merge(scan_source(&source, &name)),
            Err(error) => report.unreadable.push((name, error.to_string())),
        }
    }
    report
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_string_lossy().into_owned());
        }
    }
}

// =============================================================================
// The lexer
// =============================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Punct(char),
    /// A string literal, unescaped.
    Str(String),
    /// A number, a char literal — anything else the walk never reads.
    Other,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: u32,
    column: u32,
}

/// Breaks `source` into the tokens the walk reads, with comments and
/// whitespace gone and strings whole — so an attribute inside a comment
/// or a literal is never seen.
fn lex(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let (mut i, mut line, mut column) = (0usize, 1u32, 1u32);
    let advance = |i: &mut usize, line: &mut u32, column: &mut u32| {
        if chars[*i] == '\n' {
            *line += 1;
            *column = 1;
        } else {
            *column += 1;
        }
        *i += 1;
    };
    while i < chars.len() {
        let c = chars[i];
        let (start_line, start_column) = (line, column);
        // whitespace
        if c.is_whitespace() {
            advance(&mut i, &mut line, &mut column);
            continue;
        }
        // comments
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                advance(&mut i, &mut line, &mut column);
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let mut depth = 0u32;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    advance(&mut i, &mut line, &mut column);
                    advance(&mut i, &mut line, &mut column);
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    advance(&mut i, &mut line, &mut column);
                    advance(&mut i, &mut line, &mut column);
                    if depth == 0 {
                        break;
                    }
                } else {
                    advance(&mut i, &mut line, &mut column);
                }
            }
            continue;
        }
        // raw strings: r"…", r#"…"#, br"…"
        let raw_at = if c == 'r' { Some(i + 1) } else if c == 'b' && chars.get(i + 1) == Some(&'r') { Some(i + 2) } else { None };
        if let Some(mut j) = raw_at {
            let mut hashes = 0usize;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) == Some(&'"') {
                // consume up to the quote
                while i <= j {
                    advance(&mut i, &mut line, &mut column);
                }
                let mut text = String::new();
                loop {
                    if i >= chars.len() {
                        break;
                    }
                    if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
                        for _ in 0..=hashes {
                            advance(&mut i, &mut line, &mut column);
                        }
                        break;
                    }
                    text.push(chars[i]);
                    advance(&mut i, &mut line, &mut column);
                }
                tokens.push(Token { tok: Tok::Str(text), line: start_line, column: start_column });
                continue;
            }
        }
        // plain and byte strings
        if c == '"' || (c == 'b' && chars.get(i + 1) == Some(&'"')) {
            if c == 'b' {
                advance(&mut i, &mut line, &mut column);
            }
            advance(&mut i, &mut line, &mut column);
            let mut text = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    advance(&mut i, &mut line, &mut column);
                    let escaped = chars[i];
                    text.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        other => other,
                    });
                    advance(&mut i, &mut line, &mut column);
                    continue;
                }
                text.push(chars[i]);
                advance(&mut i, &mut line, &mut column);
            }
            if i < chars.len() {
                advance(&mut i, &mut line, &mut column);
            }
            tokens.push(Token { tok: Tok::Str(text), line: start_line, column: start_column });
            continue;
        }
        // char literals and lifetimes
        if c == '\'' {
            let is_char = chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\'');
            if is_char {
                advance(&mut i, &mut line, &mut column);
                if chars.get(i) == Some(&'\\') {
                    advance(&mut i, &mut line, &mut column);
                }
                while i < chars.len() && chars[i] != '\'' {
                    advance(&mut i, &mut line, &mut column);
                }
                if i < chars.len() {
                    advance(&mut i, &mut line, &mut column);
                }
                tokens.push(Token { tok: Tok::Other, line: start_line, column: start_column });
            } else {
                advance(&mut i, &mut line, &mut column);
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    advance(&mut i, &mut line, &mut column);
                }
                tokens.push(Token { tok: Tok::Other, line: start_line, column: start_column });
            }
            continue;
        }
        // identifiers and keywords (raw idents keep their name)
        if c.is_alphabetic() || c == '_' {
            let mut word = String::new();
            if c == 'r' && chars.get(i + 1) == Some(&'#') && chars.get(i + 2).is_some_and(|c| c.is_alphabetic() || *c == '_') {
                advance(&mut i, &mut line, &mut column);
                advance(&mut i, &mut line, &mut column);
            }
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                word.push(chars[i]);
                advance(&mut i, &mut line, &mut column);
            }
            tokens.push(Token { tok: Tok::Ident(word), line: start_line, column: start_column });
            continue;
        }
        // numbers
        if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                advance(&mut i, &mut line, &mut column);
            }
            tokens.push(Token { tok: Tok::Other, line: start_line, column: start_column });
            continue;
        }
        tokens.push(Token { tok: Tok::Punct(c), line: start_line, column: start_column });
        advance(&mut i, &mut line, &mut column);
    }
    tokens
}

// =============================================================================
// The walk
// =============================================================================

/// What a brace opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    /// A module body, a function body, anything else: items here are free.
    Free,
    /// An `impl` or `trait` body: functions here are associated.
    Associated,
}

/// Scans one file's source. `file` is how the report names it.
pub fn scan_source(source: &str, file: &str) -> ScanReport {
    let tokens = lex(source);
    let mut report = ScanReport::default();
    let mut blocks: Vec<Block> = Vec::new();
    // the item keyword seen since the last `;`, `{` or `}` at this level —
    // what the next `{` opens
    let mut opening = Block::Free;
    let mut i = 0usize;
    while i < tokens.len() {
        match &tokens[i].tok {
            Tok::Punct('{') => {
                blocks.push(opening);
                opening = Block::Free;
                i += 1;
            }
            Tok::Punct('}') => {
                blocks.pop();
                opening = Block::Free;
                i += 1;
            }
            Tok::Punct(';') => {
                opening = Block::Free;
                i += 1;
            }
            Tok::Ident(word) if word == "impl" || word == "trait" => {
                opening = Block::Associated;
                i += 1;
            }
            Tok::Punct('#') if tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('[')) => {
                let associated = blocks.last() == Some(&Block::Associated);
                i = attribute(&tokens, i, file, associated, &mut report);
            }
            Tok::Ident(word)
                if word == "app"
                    && blocks.is_empty()
                    && tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('!')) =>
            {
                let start = path_start(&tokens, i);
                if let Some(end) = macro_end(&tokens, i + 2) {
                    report.record_app(Site { file: file.to_string(), line: tokens[start].line, column: tokens[start].column });
                    i = end;
                } else {
                    i += 2;
                }
            }
            Tok::Ident(word) if word == "declare_preview" && tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('!')) => {
                // `bunny_ui::declare_preview!` sites at the path's start
                let start = path_start(&tokens, i);
                i = declared(&tokens, i, start, file, &mut report);
            }
            Tok::Ident(word) if tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('!')) => {
                let open = i + if word == "macro_rules" { 3 } else { 2 };
                i = macro_end(&tokens, open).unwrap_or(i + 2);
            }
            _ => i += 1,
        }
    }
    report
}

fn path_start(tokens: &[Token], mut at: usize) -> usize {
    while at >= 2 && tokens[at - 1].tok == Tok::Punct(':') && tokens[at - 2].tok == Tok::Punct(':')
    {
        at = if at >= 3 && matches!(tokens[at - 3].tok, Tok::Ident(_)) {
            at - 3
        } else {
            at - 2
        };
    }
    at
}

fn macro_end(tokens: &[Token], open: usize) -> Option<usize> {
    let (opener, closer) = match tokens.get(open)?.tok {
        Tok::Punct('(') => ('(', ')'),
        Tok::Punct('[') => ('[', ']'),
        Tok::Punct('{') => ('{', '}'),
        _ => return None,
    };
    Some(matching(tokens, open, opener, closer) + 1)
}

/// Reads an attribute at `at` (`#`), records what it means, and answers
/// where the walk continues.
fn attribute(tokens: &[Token], at: usize, file: &str, associated: bool, report: &mut ScanReport) -> usize {
    let site = Site { file: file.to_string(), line: tokens[at].line, column: tokens[at].column };
    // the attribute's path, up to `(` or `]`
    let mut i = at + 2;
    let mut path: Vec<String> = Vec::new();
    while let Some(token) = tokens.get(i) {
        match &token.tok {
            Tok::Ident(word) => path.push(word.clone()),
            Tok::Punct(':') => {}
            _ => break,
        }
        i += 1;
    }
    let is_preview = path.last().is_some_and(|last| last == "preview") && path.first().is_some_and(|first| first != "cfg_attr");
    // the arguments, when any
    let mut args: Vec<(String, Option<String>)> = Vec::new();
    let mut argument_error: Option<Reason> = None;
    if tokens.get(i).is_some_and(|t| t.tok == Tok::Punct('(')) {
        let close = matching(tokens, i, '(', ')');
        if is_preview {
            argument_error = arguments(&tokens[i + 1..close], &mut args);
        }
    }
    // to the closing bracket of the attribute
    let close = matching(tokens, at + 1, '[', ']');
    let mut i = close + 1;
    if !is_preview {
        return i;
    }
    if let Some(reason) = argument_error {
        report.refused.push(Refusal { site, ident: None, reason });
        return i;
    }
    // the item: past the other attributes, through the visibility and the qualifiers
    while tokens.get(i).is_some_and(|t| t.tok == Tok::Punct('#')) {
        let open = i + 1 + usize::from(tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('!')));
        if !tokens.get(open).is_some_and(|t| t.tok == Tok::Punct('[')) {
            break;
        }
        i = matching(tokens, open, '[', ']') + 1;
    }
    let item_start = i;
    if tokens.get(i).is_some_and(|t| t.tok == Tok::Ident(String::from("pub"))) {
        i += 1;
        if tokens.get(i).is_some_and(|t| t.tok == Tok::Punct('(')) {
            i = matching(tokens, i, '(', ')') + 1;
        }
    }
    let mut qualifier: Option<String> = None;
    while let Some(Tok::Ident(word)) = tokens.get(i).map(|t| &t.tok) {
        match word.as_str() {
            "const" => i += 1,
            "async" | "unsafe" | "extern" => {
                qualifier.get_or_insert_with(|| word.clone());
                i += 1;
                if word == "extern" && matches!(tokens.get(i).map(|t| &t.tok), Some(Tok::Str(_))) {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    if !tokens.get(i).is_some_and(|t| t.tok == Tok::Ident(String::from("fn"))) {
        report.refused.push(Refusal { site, ident: None, reason: Reason::NotAFunction });
        return close + 1;
    }
    let Some(Tok::Ident(ident)) = tokens.get(i + 1).map(|t| &t.tok) else {
        report.refused.push(Refusal { site, ident: None, reason: Reason::NotAFunction });
        return close + 1;
    };
    let ident = ident.clone();
    let item_site = Site { file: file.to_string(), line: tokens[item_start].line, column: tokens[item_start].column };
    let refuse = |reason: Reason, report: &mut ScanReport| {
        report.refused.push(Refusal { site: item_site.clone(), ident: Some(ident.clone()), reason });
    };
    if ident == crate::wire::APP_IDENT {
        refuse(Reason::Reserved, report);
        return i + 2;
    }
    if associated {
        refuse(Reason::Associated, report);
        return i + 2;
    }
    if let Some(word) = qualifier {
        refuse(Reason::Qualifier(word), report);
        return i + 2;
    }
    let mut j = i + 2;
    if tokens.get(j).is_some_and(|t| t.tok == Tok::Punct('<')) {
        refuse(Reason::Generic, report);
        return j;
    }
    if !tokens.get(j).is_some_and(|t| t.tok == Tok::Punct('(')) {
        refuse(Reason::NotAFunction, report);
        return j;
    }
    let params_close = matching(tokens, j, '(', ')');
    if params_close > j + 1 {
        refuse(Reason::Parameters, report);
        return params_close + 1;
    }
    j = params_close + 1;
    let mut name = None;
    let mut scope = None;
    let mut example = None;
    for (key, value) in args {
        match key.as_str() {
            "name" => name = value,
            "scope" => scope = value,
            "example" => example = value,
            _ => {}
        }
    }
    report.previews.push(PreviewRef {
        symbol: symbol(&ident),
        name: name.unwrap_or_else(|| ident.clone()),
        scope: scope.unwrap_or_default(),
        example: example.unwrap_or_else(|| ident.clone()),
        ident,
        file: file.to_string(),
        line: item_site.line,
        column: item_site.column,
    });
    j
}

/// Reads `key = "…"` pairs; `crate = path` is accepted and dropped.
fn arguments(tokens: &[Token], out: &mut Vec<(String, Option<String>)>) -> Option<Reason> {
    let mut i = 0;
    while i < tokens.len() {
        let Tok::Ident(key) = &tokens[i].tok else {
            return Some(Reason::BadArgument(describe(&tokens[i].tok)));
        };
        if !matches!(key.as_str(), "name" | "scope" | "example" | "crate") {
            return Some(Reason::UnknownArgument(key.clone()));
        }
        if !tokens.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('=')) {
            return Some(Reason::BadArgument(key.clone()));
        }
        i += 2;
        if key == "crate" {
            while i < tokens.len() && tokens[i].tok != Tok::Punct(',') {
                i += 1;
            }
            out.push((key.clone(), None));
        } else {
            let Some(Tok::Str(value)) = tokens.get(i).map(|t| &t.tok) else {
                return Some(Reason::BadArgument(key.clone()));
            };
            out.push((key.clone(), Some(value.clone())));
            i += 1;
        }
        match tokens.get(i).map(|t| &t.tok) {
            None => break,
            Some(Tok::Punct(',')) => i += 1,
            Some(other) => return Some(Reason::BadArgument(describe(other))),
        }
    }
    None
}

fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Ident(word) => word.clone(),
        Tok::Punct(c) => c.to_string(),
        Tok::Str(s) => format!("\"{s}\""),
        Tok::Other => String::from("a literal"),
    }
}

/// `declare_preview!(ident, symbol, "name", "scope", "example")` at `at`.
fn declared(tokens: &[Token], at: usize, start: usize, file: &str, report: &mut ScanReport) -> usize {
    let site = Site { file: file.to_string(), line: tokens[start].line, column: tokens[start].column };
    let open = at + 2;
    if !tokens.get(open).is_some_and(|t| t.tok == Tok::Punct('(')) {
        return at + 2;
    }
    let close = matching(tokens, open, '(', ')');
    let inner: Vec<&Tok> = tokens[open + 1..close].iter().map(|t| &t.tok).filter(|t| **t != Tok::Punct(',')).collect();
    match inner.as_slice() {
        [Tok::Ident(ident), ..] if ident == crate::wire::APP_IDENT => {
            report.refused.push(Refusal { site, ident: Some(ident.clone()), reason: Reason::Reserved });
        }
        [Tok::Ident(ident), Tok::Ident(symbol_name), Tok::Str(name), Tok::Str(scope), Tok::Str(example)] => {
            report.previews.push(PreviewRef {
                ident: ident.clone(),
                symbol: symbol_name.clone(),
                name: name.clone(),
                scope: scope.clone(),
                example: example.clone(),
                file: file.to_string(),
                line: site.line,
                column: site.column,
            });
        }
        _ => report.refused.push(Refusal { site, ident: None, reason: Reason::BadArgument(String::from("declare_preview")) }),
    }
    close + 1
}

/// The index of the bracket closing the one at `open`, or the end.
fn matching(tokens: &[Token], open: usize, opener: char, closer: char) -> usize {
    let mut depth = 0u32;
    let mut i = open;
    while i < tokens.len() {
        match tokens[i].tok {
            Tok::Punct(c) if c == opener => depth += 1,
            Tok::Punct(c) if c == closer => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    tokens.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lexer_keeps_strings_whole_and_drops_comments() {
        let tokens = lex("const S: &str = \"#[preview] fn x() {}\"; // #[preview] fn y() {}\n/* #[preview] */ fn z() {}");
        let words: Vec<String> = tokens.iter().map(|t| describe(&t.tok)).collect();
        assert_eq!(words, ["const", "S", ":", "&", "str", "=", "\"#[preview] fn x() {}\"", ";", "fn", "z", "(", ")", "{", "}"]);
        assert_eq!((tokens[8].line, tokens[8].column), (2, 18), "the fn after the block comment");
    }

    #[test]
    fn raw_strings_chars_and_lifetimes_do_not_confuse_the_lexer() {
        let tokens = lex("let a = r#\"x \" y\"#; let b = '\"'; fn f<'a>() {} #[preview] fn g() {}");
        let idents: Vec<&str> = tokens.iter().filter_map(|t| if let Tok::Ident(w) = &t.tok { Some(w.as_str()) } else { None }).collect();
        assert_eq!(idents, ["let", "a", "let", "b", "fn", "f", "preview", "fn", "g"], "a lifetime is not an ident, a char is not a string");
    }

    #[test]
    fn a_free_function_under_the_attribute_is_a_preview_at_its_own_site() {
        let report = scan_source("mod m {\n    #[preview(name = \"Card\")]\n    pub(crate) fn card() -> impl View { text(\"\") }\n}\n", "src/a.rs");
        assert_eq!(report.refused, Vec::new());
        assert_eq!(report.previews.len(), 1);
        let preview = &report.previews[0];
        assert_eq!((preview.line, preview.column), (3, 5));
        assert_eq!((preview.name.as_str(), preview.example.as_str(), preview.scope.as_str()), ("Card", "card", ""));
        assert_eq!(preview.symbol, "bunny_preview__card");
    }

    #[test]
    fn an_impl_block_and_a_trait_make_their_functions_associated() {
        let report = scan_source("impl Foo {\n    #[preview]\n    fn inside() -> impl View { text(\"\") }\n}\ntrait T { #[preview] fn also(); }\n#[preview]\nfn free() -> impl View { text(\"\") }\n", "src/a.rs");
        assert_eq!(report.previews.iter().map(|p| p.ident.as_str()).collect::<Vec<_>>(), ["free"]);
        assert_eq!(report.refused.iter().map(|r| (r.ident.as_deref(), r.reason.clone())).collect::<Vec<_>>(), [(Some("inside"), Reason::Associated), (Some("also"), Reason::Associated)]);
    }

    #[test]
    fn the_declarative_form_is_a_preview_at_the_macros_site() {
        let report = scan_source("fn home() -> impl View { text(\"\") }\nbunny_ui::declare_preview!(home, bunny_preview__home, \"Home\", \"Screens\", \"home\");\n", "src/lib.rs");
        assert_eq!(report.previews.len(), 1);
        assert_eq!((report.previews[0].line, report.previews[0].column), (2, 1));
        assert_eq!(report.previews[0].scope, "Screens");
    }

    #[test]
    fn qualifiers_generics_and_bad_arguments_are_refused_by_name() {
        let report = scan_source("#[preview]\nasync fn a() {}\n#[preview]\nfn g<T>() {}\n#[preview(name = 7)]\nfn n() {}\n#[preview(name)]\nfn m() {}\n", "src/a.rs");
        let reasons: Vec<String> = report.refused.iter().map(|r| r.reason.tag()).collect();
        assert_eq!(reasons, ["qualifier:async", "generic", "bad_argument:name", "bad_argument:name"]);
        assert!(report.previews.is_empty());
    }

    #[test]
    fn merging_names_the_second_claim_of_an_ident_a_duplicate() {
        let mut a = scan_source("#[preview]\nfn twin() {}\n", "a.rs");
        let b = scan_source("#[preview(name = \"Twin\")]\nfn twin() {}\n", "b.rs");
        a.merge(b);
        assert_eq!(a.previews.len(), 1);
        assert_eq!(a.duplicates.len(), 1);
        assert_eq!(a.duplicates[0].kept.to_string(), "a.rs:2:1");
        assert_eq!(a.duplicates[0].dropped.to_string(), "b.rs:2:1");
        assert!(a.json().contains("\"duplicates\": [\n    {\"ident\": \"twin\""));
    }
}
