//! What an edit reaches: the inside of functions only, or the types.
//!
//! A hot reload builds the app's code again and loads it next to the
//! old code. The state the old code made stays, and the new code reads
//! it — safe while each type keeps its shape. The compiler names a type
//! by its path and the build's salt, not by its fields: a field that
//! changes type keeps the name, and the new code would read the old
//! value as the new shape. So an edit that reaches a type gets a new
//! salt, and the types of the new build are new types, whose state
//! starts over; an edit inside function bodies keeps the salt, and all
//! the state stays.
//!
//! The decision is lexical. The file becomes its tokens, without
//! comments, and the inside of each function body comes out — unless
//! the body declares an item of its own (a `struct`, an `impl`…), which
//! is then part of what must not move. Two versions whose remains are
//! equal changed only inside bodies. A wrong answer is safe one way
//! only, so every doubt answers "types": a new salt costs the state of
//! the edited types, a kept salt on a moved type costs memory read
//! wrong.

/// What an edit to a source file reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Only the inside of function bodies: every type is as it was.
    Bodies,
    /// Something else — a type, a field, a signature, an item added or
    /// removed. The new build needs a new salt.
    Types,
}

/// What the edit from `old` to `new` reaches.
pub fn classify(old: &str, new: &str) -> Reach {
    if outline(old) == outline(new) { Reach::Bodies } else { Reach::Types }
}

/// The words that declare an item. A body that holds one keeps all its
/// tokens: the items inside a function are types too.
const ITEM_WORDS: &[&str] = &["struct", "enum", "union", "trait", "impl", "type", "static", "mod", "macro_rules"];

/// The file's tokens with the inside of every function body taken out.
pub fn outline(source: &str) -> Vec<String> {
    let tokens = tokens(source);
    let mut out = Vec::with_capacity(tokens.len());
    let mut at = 0;
    while at < tokens.len() {
        let token = &tokens[at];
        let is_item_fn = token == "fn" && tokens.get(at + 1).is_some_and(|next| is_word(next));
        if !is_item_fn {
            out.push(token.clone());
            at += 1;
            continue;
        }
        // the signature runs to the body's `{`, or to the `;` of a
        // declaration without one — outside parentheses and brackets
        let mut end = at + 1;
        let mut depth = 0i32;
        while end < tokens.len() {
            match tokens[end].as_str() {
                "(" | "[" => depth += 1,
                ")" | "]" => depth -= 1,
                "{" | ";" if depth <= 0 => break,
                _ => {}
            }
            end += 1;
        }
        out.extend(tokens[at..end].iter().cloned());
        if end >= tokens.len() || tokens[end] == ";" {
            at = end;
            continue;
        }
        let close = matching_brace(&tokens, end);
        let body = &tokens[end + 1..close.min(tokens.len())];
        if body.iter().any(|token| ITEM_WORDS.contains(&token.as_str())) {
            out.extend(tokens[end..(close + 1).min(tokens.len())].iter().cloned());
        } else {
            out.push(String::from("{"));
            out.push(String::from("}"));
        }
        at = close + 1;
    }
    out
}

/// The index of the `}` that closes the `{` at `open` — the end of the
/// tokens when it is never closed.
fn matching_brace(tokens: &[String], open: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.as_str() {
            "{" => depth += 1,
            "}" => {
                depth -= 1;
                if depth == 0 {
                    return index;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

fn is_word(token: &str) -> bool {
    let first = token.trim_start_matches("r#").chars().next();
    first.is_some_and(|c| c == '_' || c.is_alphabetic())
}

/// The tokens of Rust source, comments left out: words, literals, and
/// each punctuation character on its own.
pub fn tokens(source: &str) -> Vec<String> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        if c.is_whitespace() {
            at += 1;
        } else if c == '/' && next == Some('/') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '/' && next == Some('*') {
            at = block_comment_end(&chars, at);
        } else if let Some(end) = string_end(&chars, at) {
            tokens.push(chars[at..end].iter().collect());
            at = end;
        } else if c == '\'' {
            let end = quote_end(&chars, at);
            tokens.push(chars[at..end].iter().collect());
            at = end;
        } else if c == '_' || c.is_alphanumeric() {
            let mut end = at;
            while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                end += 1;
            }
            // a number's decimal point, never a range's `..`
            if c.is_ascii_digit()
                && chars.get(end) == Some(&'.')
                && chars.get(end + 1).is_some_and(char::is_ascii_digit)
            {
                end += 1;
                while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                    end += 1;
                }
            }
            // a raw identifier: `r#type` is one word
            if end == at + 1 && c == 'r' && chars.get(end) == Some(&'#') && chars.get(end + 1).is_some_and(|c| c.is_alphabetic() || *c == '_') {
                end += 1;
                while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
                    end += 1;
                }
            }
            tokens.push(chars[at..end].iter().collect());
            at = end;
        } else {
            tokens.push(c.to_string());
            at += 1;
        }
    }
    tokens
}

/// Past a block comment that starts at `at` — they nest in Rust.
fn block_comment_end(chars: &[char], mut at: usize) -> usize {
    let mut depth = 0usize;
    while at < chars.len() {
        if chars[at] == '/' && chars.get(at + 1) == Some(&'*') {
            depth += 1;
            at += 2;
        } else if chars[at] == '*' && chars.get(at + 1) == Some(&'/') {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    at
}

/// The end of a string literal that starts at `at`, when one does:
/// `"…"`, raw `r#"…"#`, and the byte and C forms of both.
fn string_end(chars: &[char], at: usize) -> Option<usize> {
    let mut start = at;
    if matches!(chars[start], 'b' | 'c') {
        start += 1;
    }
    let raw = chars.get(start) == Some(&'r');
    if raw {
        start += 1;
    }
    let mut hashes = 0;
    while raw && chars.get(start + hashes) == Some(&'#') {
        hashes += 1;
    }
    if chars.get(start + hashes) != Some(&'"') {
        return None;
    }
    // a plain word that only looks like a prefix (`b`, `cr`) is no string
    if start > at && chars[at..start].iter().any(|c| !matches!(c, 'b' | 'c' | 'r')) {
        return None;
    }
    let mut end = start + hashes + 1;
    while end < chars.len() {
        if !raw && chars[end] == '\\' {
            end += 2;
            continue;
        }
        if chars[end] == '"' && (0..hashes).all(|n| chars.get(end + 1 + n) == Some(&'#')) {
            return Some(end + 1 + hashes);
        }
        end += 1;
    }
    Some(chars.len())
}

/// The end of a character literal (`'a'`, `'\n'`) or a lifetime (`'a`)
/// that starts at `at`.
fn quote_end(chars: &[char], at: usize) -> usize {
    if chars.get(at + 1) == Some(&'\\') {
        let mut end = at + 2;
        while end < chars.len() && chars[end] != '\'' {
            end += 1;
        }
        return (end + 1).min(chars.len());
    }
    if chars.get(at + 2) == Some(&'\'') {
        return at + 3;
    }
    let mut end = at + 1;
    while end < chars.len() && (chars[end] == '_' || chars[end].is_alphanumeric()) {
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r#"
use bunny_ui::prelude::*;

/// A counter.
#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(self) -> impl View {
        vstack!(
            text("Hello"),
            text!("You tapped {} times", self.count),
            button(text("Tap me"), move || self.count.add(1)),
        )
    }
}

fn home() -> impl View {
    Counter { count: State::new(0) }
}

bunny_ui::app!(home, bunny_ui::AppConfig::new().size(420.0, 640.0));
"#;

    fn edit(from: &str, to: &str) -> String {
        assert!(APP.contains(from), "{from}");
        APP.replacen(from, to, 1)
    }

    #[test]
    fn an_edit_inside_a_body_keeps_the_types() {
        assert_eq!(classify(APP, APP), Reach::Bodies);
        assert_eq!(classify(APP, &edit("\"Hello\"", "\"Hello, { world }\"")), Reach::Bodies);
        assert_eq!(classify(APP, &edit("text(\"Hello\"),", "text(\"Hello\"),\n            spacer(),")), Reach::Bodies);
        assert_eq!(classify(APP, &edit("State::new(0)", "State::new(10)")), Reach::Bodies);
        assert_eq!(classify(APP, &edit("/// A counter.", "/// A counter that counts.\n// more")), Reach::Bodies);
        assert_eq!(classify(APP, &edit("move || self.count.add(1)", "move || { self.count.add(2) }")), Reach::Bodies);
    }

    #[test]
    fn an_edit_outside_the_bodies_reaches_the_types() {
        assert_eq!(classify(APP, &edit("count: State<i32>,", "count: State<i64>,")), Reach::Types);
        assert_eq!(classify(APP, &edit("count: State<i32>,", "count: State<i32>,\n    step: State<i32>,")), Reach::Types);
        assert_eq!(classify(APP, &edit("#[derive(Clone, Copy)]", "#[derive(Clone, Copy, Debug)]")), Reach::Types);
        assert_eq!(classify(APP, &edit("fn home() -> impl View", "fn home() -> impl View + Clone")), Reach::Types);
        assert_eq!(classify(APP, &edit("size(420.0, 640.0)", "size(400.0, 640.0)")), Reach::Types);
        assert_eq!(classify(APP, &edit("fn home()", "struct Extra;\n\nfn home()")), Reach::Types);
    }

    #[test]
    fn a_body_that_declares_an_item_keeps_its_tokens() {
        let with_item = edit("Counter { count: State::new(0) }", "struct Local(u8);\n    Counter { count: State::new(0) }");
        let edited = with_item.replace("struct Local(u8);", "struct Local(u16);");
        assert_eq!(classify(&with_item, &edited), Reach::Types);
        let body_edit = with_item.replace("State::new(0)", "State::new(1)");
        assert_eq!(classify(&with_item, &body_edit), Reach::Types, "every doubt answers types");
    }

    #[test]
    fn the_tokens_skip_comments_and_keep_literals_whole() {
        assert_eq!(
            tokens("a /* x /* nested */ y */ b // c\nr#\"q\"u\"# 'x' '\\n' 'life b\"\\\"\" 1.5 0..2 r#type"),
            [
                "a", "b", "r#\"q\"u\"#", "'x'", "'\\n'", "'life", "b\"\\\"\"", "1.5", "0", ".", ".", "2", "r#type"
            ]
        );
    }

    #[test]
    fn a_trait_declaration_without_a_body_is_kept() {
        let source = "trait Shape { fn area(&self) -> f64; fn name(&self) -> &str { \"shape\" } }";
        assert_eq!(classify(source, &source.replace("\"shape\"", "\"form\"")), Reach::Bodies);
        assert_eq!(classify(source, &source.replace("-> f64;", "-> f32;")), Reach::Types);
    }
}
