//! Rust source as the rules see it: identifiers, punctuation and
//! literals, each with the bytes it spans. Comments and whitespace are
//! not tokens, so a rule never reads a word inside a comment or a
//! string, and a rewrite keeps every byte it did not mean to change.

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A name or a keyword (`nav_link_value`, `fn`, `r#type`).
    Ident,
    /// One character of punctuation: `::` is two tokens.
    Punct(char),
    /// A string, a character, a byte string or a number.
    Literal,
    /// `'a`.
    Lifetime,
}

/// One token and where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// The token's text in `source`.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    pub fn is_punct(&self, c: char) -> bool {
        self.kind == Kind::Punct(c)
    }

    pub fn is_ident(&self, source: &str, name: &str) -> bool {
        self.kind == Kind::Ident && self.text(source) == name
    }
}

/// The tokens of `source`. Text the lexer cannot read — an unterminated
/// string, a stray byte — ends the tokens there: a rule then sees less,
/// and changes less, never more.
pub fn tokens(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        let start = at;
        if c.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if source[at..].starts_with("//") {
            at = source[at..].find('\n').map_or(bytes.len(), |n| at + n);
            continue;
        }
        if source[at..].starts_with("/*") {
            match block_comment(bytes, at) {
                Some(end) => at = end,
                None => break,
            }
            continue;
        }
        // raw strings and byte/C strings before identifiers: `r"…"`,
        // `r#"…"#`, `b"…"`, `br"…"`, `c"…"`, `b'x'`
        if let Some(end) = prefixed_literal(bytes, at) {
            out.push(Token { kind: Kind::Literal, start, end });
            at = end;
            continue;
        }
        if c == b'"' {
            let Some(end) = quoted(bytes, at + 1, b'"') else { break };
            out.push(Token { kind: Kind::Literal, start, end });
            at = end;
            continue;
        }
        if c == b'\'' {
            // a character literal, or a lifetime
            if let Some(end) = char_literal(source, at) {
                out.push(Token { kind: Kind::Literal, start, end });
                at = end;
            } else {
                at += 1;
                while at < bytes.len() && is_ident_byte(bytes[at]) {
                    at += 1;
                }
                out.push(Token { kind: Kind::Lifetime, start, end: at });
            }
            continue;
        }
        if c.is_ascii_digit() {
            at += 1;
            while at < bytes.len() && (is_ident_byte(bytes[at]) || (bytes[at] == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))) {
                at += 1;
            }
            out.push(Token { kind: Kind::Literal, start, end: at });
            continue;
        }
        if is_ident_start(source, at) {
            if source[at..].starts_with("r#") {
                at += 2;
            }
            while at < bytes.len() && is_ident_continue(source, at) {
                at += source[at..].chars().next().map_or(1, char::len_utf8);
            }
            out.push(Token { kind: Kind::Ident, start, end: at });
            continue;
        }
        let ch = source[at..].chars().next().unwrap_or('\0');
        at += ch.len_utf8();
        out.push(Token { kind: Kind::Punct(ch), start, end: at });
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_start(source: &str, at: usize) -> bool {
    source[at..].chars().next().is_some_and(|c| c == '_' || c.is_alphabetic())
}

fn is_ident_continue(source: &str, at: usize) -> bool {
    source[at..].chars().next().is_some_and(|c| c == '_' || c.is_alphanumeric())
}

/// The end of the (nested) block comment that starts at `at`.
fn block_comment(bytes: &[u8], mut at: usize) -> Option<usize> {
    let mut depth = 0usize;
    while at + 1 < bytes.len() {
        match (bytes[at], bytes[at + 1]) {
            (b'/', b'*') => {
                depth += 1;
                at += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                at += 2;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// The end of a quoted run whose body starts at `at`, escapes honoured.
fn quoted(bytes: &[u8], mut at: usize, quote: u8) -> Option<usize> {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b if b == quote => return Some(at + 1),
            _ => at += 1,
        }
    }
    None
}

/// The end of a raw string's body: `hashes` `#`s after the closing `"`.
fn raw(bytes: &[u8], mut at: usize, hashes: usize) -> Option<usize> {
    while at < bytes.len() {
        if bytes[at] == b'"' && bytes.len() >= at + 1 + hashes && bytes[at + 1..at + 1 + hashes].iter().all(|b| *b == b'#') {
            return Some(at + 1 + hashes);
        }
        at += 1;
    }
    None
}

/// A literal that opens with a letter: `r"…"`, `r#"…"#`, `b"…"`,
/// `br#"…"#`, `c"…"`, `cr"…"`, `b'x'`.
fn prefixed_literal(bytes: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    if matches!(bytes.get(i), Some(b'b' | b'c')) {
        i += 1;
    }
    let is_raw = bytes.get(i) == Some(&b'r');
    if is_raw {
        i += 1;
    }
    if i == at {
        return None;
    }
    // the prefix must not be the start of a longer name (`bar"` is not one)
    if at > 0 && is_ident_byte(bytes[at - 1]) {
        return None;
    }
    if is_raw {
        let hashes = bytes[i..].iter().take_while(|b| **b == b'#').count();
        if bytes.get(i + hashes) != Some(&b'"') {
            return None;
        }
        return raw(bytes, i + hashes + 1, hashes);
    }
    match bytes.get(i) {
        Some(b'"') => quoted(bytes, i + 1, b'"'),
        Some(b'\'') if bytes[at] == b'b' => quoted(bytes, i + 1, b'\''),
        _ => None,
    }
}

/// `'x'`, `'\n'`, `'\u{1F430}'`, `'''`-free: the end of a character
/// literal at `at`, or `None` for a lifetime.
fn char_literal(source: &str, at: usize) -> Option<usize> {
    let rest = &source[at + 1..];
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        let close = rest.find('\'')?;
        // `'\''` holds the quote it escapes
        let close = if close == 1 { rest[2..].find('\'')? + 2 } else { close };
        return Some(at + 1 + close + 1);
    }
    let (next, after) = chars.next()?;
    (after == '\'').then_some(at + 1 + next + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(source: &str) -> Vec<&str> {
        tokens(source).iter().map(|token| token.text(source)).collect()
    }

    #[test]
    fn comments_and_whitespace_are_not_tokens() {
        assert_eq!(words("a /* b /* c */ d */ e // f\ng"), ["a", "e", "g"]);
    }

    #[test]
    fn literals_are_one_token_each() {
        let source = r##"x("a(b", r#"c"d"#, b"e", br"f", '(', '\'', b'\n', 'é', 1.5e3, 0x1f)"##;
        assert_eq!(
            words(source),
            ["x", "(", "\"a(b\"", ",", "r#\"c\"d\"#", ",", "b\"e\"", ",", "br\"f\"", ",", "'('", ",", "'\\''", ",", "b'\\n'", ",", "'é'", ",", "1.5e3", ",", "0x1f", ")"]
        );
    }

    #[test]
    fn lifetimes_are_not_characters() {
        let source = "fn f<'a>(x: &'a str) -> &'static str";
        let lifetimes: Vec<&str> = tokens(source).iter().filter(|t| t.kind == Kind::Lifetime).map(|t| t.text(source)).collect();
        assert_eq!(lifetimes, ["'a", "'a", "'static"]);
    }

    #[test]
    fn names_with_a_literal_prefix_stay_names() {
        assert_eq!(words("bar(r, br, c)"), ["bar", "(", "r", ",", "br", ",", "c", ")"]);
        assert_eq!(words("r#type"), ["r#type"]);
    }

    #[test]
    fn an_unterminated_string_ends_the_tokens() {
        assert_eq!(words("a \"b"), ["a"]);
    }
}
