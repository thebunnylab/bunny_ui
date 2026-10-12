//! `#[preview]` — marks a function whose view Studio and `bunny preview`
//! show, the way SwiftUI's `#Preview` marks a block. (`bunny_ui::declare_preview!`
//! writes the same entry by hand, for a crate that would rather not expand
//! an attribute.)
//!
//! ```ignore
//! #[bunny_ui::preview(name = "Button", scope = "Input", example = "Primary")]
//! fn button_primary() -> impl View {
//!     button(text("Save"), || {})
//! }
//! ```
//!
//! The function stays exactly as written. After it, the attribute writes
//! one invocation of `bunny_ui::__bunny_preview_entry!`, which — in a
//! build with the facade's `preview` feature — exports a symbol named
//! `bunny_preview__<ident>` that answers the preview's entry (its
//! metadata, and a function that builds the view with its traits); in
//! any other build it expands to nothing but a reference that keeps the
//! function from reading as dead code. Discovery is a scan of the source
//! for the attribute and a `dlsym` of that symbol: no registry, no
//! linker section, nothing an app that forbids `unsafe` has to allow.
//!
//! The arguments, each optional: `name = "…"` (the component the preview
//! belongs to; the function's name otherwise), `scope = "…"` (the gallery
//! section; none otherwise), `example = "…"` (this preview's label on the
//! component's card; the function's name otherwise) and `crate = path`
//! (where `bunny_ui` is reachable, for a crate that renamed it; `::bunny_ui`
//! otherwise). A function with parameters, type parameters, or marked
//! `async`, `unsafe` or `extern`, and anything that is not a free function,
//! is refused with a `compile_error!` that says why.
//!
//! No dependencies: the tokens are scanned by hand (`scan`), and the scan
//! is unit-tested on its own token model, since a proc-macro's own types
//! exist only inside an expansion.

use proc_macro::{Delimiter, TokenStream, TokenTree};

/// Marks a free function with no parameters whose return value is a view
/// or a `Preview` — see the crate docs.
#[proc_macro_attribute]
pub fn preview(args: TokenStream, item: TokenStream) -> TokenStream {
    let tokens = scan::tokens(args);
    let item_tokens = scan::tokens(item.clone());
    let mut out = item;
    let tail = match scan::arguments(&tokens).and_then(|args| scan::signature(&item_tokens).map(|ident| (args, ident))) {
        Ok((args, ident)) => scan::invocation(&args, &ident),
        Err(why) => format!("::core::compile_error!({why:?});"),
    };
    out.extend(tail.parse::<TokenStream>().expect("the expansion is well-formed Rust"));
    out
}

mod scan {
    use super::{Delimiter, TokenStream, TokenTree};

    /// A token as the scan sees it — enough of each tree to read a
    /// signature and an argument list, and nothing a test cannot build.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Tok {
        /// An identifier or a keyword.
        Ident(String),
        /// One punctuation character.
        Punct(char),
        /// A delimited group, with whether it holds anything.
        Group { delimiter: Delim, empty: bool },
        /// A literal, as written.
        Literal(String),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Delim {
        Paren,
        Brace,
        Bracket,
        None,
    }

    /// The attribute's arguments, each as written or defaulted.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Args {
        /// The `name` literal, quotes included.
        pub name: Option<String>,
        /// The `scope` literal, quotes included.
        pub scope: Option<String>,
        /// The `example` literal, quotes included.
        pub example: Option<String>,
        /// The path to `bunny_ui`, as written.
        pub krate: Option<String>,
    }

    pub fn tokens(stream: TokenStream) -> Vec<Tok> {
        stream
            .into_iter()
            .map(|tree| match tree {
                TokenTree::Ident(ident) => Tok::Ident(ident.to_string()),
                TokenTree::Punct(punct) => Tok::Punct(punct.as_char()),
                TokenTree::Group(group) => Tok::Group {
                    delimiter: match group.delimiter() {
                        Delimiter::Parenthesis => Delim::Paren,
                        Delimiter::Brace => Delim::Brace,
                        Delimiter::Bracket => Delim::Bracket,
                        Delimiter::None => Delim::None,
                    },
                    empty: group.stream().is_empty(),
                },
                TokenTree::Literal(literal) => Tok::Literal(literal.to_string()),
            })
            .collect()
    }

    /// Reads `key = value` pairs separated by commas.
    pub fn arguments(tokens: &[Tok]) -> Result<Args, String> {
        let mut args = Args { name: None, scope: None, example: None, krate: None };
        let mut at = 0;
        while at < tokens.len() {
            let Tok::Ident(key) = &tokens[at] else {
                return Err(format!("`#[preview]` takes `name`, `scope`, `example` or `crate = path`, not {}", show(&tokens[at])));
            };
            if tokens.get(at + 1) != Some(&Tok::Punct('=')) {
                return Err(format!("`#[preview({key})]`: write `{key} = …`"));
            }
            at += 2;
            match key.as_str() {
                "crate" => {
                    let start = at;
                    while at < tokens.len() && tokens[at] != Tok::Punct(',') {
                        at += 1;
                    }
                    if start == at {
                        return Err(String::from("`#[preview(crate = …)]` names no path"));
                    }
                    let path: String = tokens[start..at].iter().map(path_piece).collect();
                    set(&mut args.krate, "crate", path)?;
                }
                "name" | "scope" | "example" => {
                    let Some(Tok::Literal(literal)) = tokens.get(at) else {
                        return Err(format!("`#[preview({key} = …)]` takes a string literal"));
                    };
                    if !literal.starts_with('"') {
                        return Err(format!("`#[preview({key} = {literal})]` takes a string literal"));
                    }
                    let slot = match key.as_str() {
                        "name" => &mut args.name,
                        "scope" => &mut args.scope,
                        _ => &mut args.example,
                    };
                    set(slot, key, literal.clone())?;
                    at += 1;
                }
                other => {
                    return Err(format!("`#[preview({other} = …)]`: unknown argument; the arguments are name, scope, example and crate"));
                }
            }
            match tokens.get(at) {
                None => break,
                Some(Tok::Punct(',')) => at += 1,
                Some(tok) => return Err(format!("`#[preview(…)]`: expected a comma, found {}", show(tok))),
            }
        }
        Ok(args)
    }

    fn set(slot: &mut Option<String>, key: &str, value: String) -> Result<(), String> {
        if slot.is_some() {
            return Err(format!("`#[preview]`: `{key}` is given twice"));
        }
        *slot = Some(value);
        Ok(())
    }

    fn path_piece(tok: &Tok) -> String {
        match tok {
            Tok::Ident(ident) => ident.clone(),
            Tok::Punct(punct) => punct.to_string(),
            Tok::Literal(literal) => literal.clone(),
            Tok::Group { .. } => String::from("<>"),
        }
    }

    fn show(tok: &Tok) -> String {
        match tok {
            Tok::Ident(ident) => format!("`{ident}`"),
            Tok::Punct(punct) => format!("`{punct}`"),
            Tok::Literal(literal) => literal.clone(),
            Tok::Group { .. } => String::from("a group"),
        }
    }

    /// The function's name, when the item is a free function with no
    /// parameters that a preview may call — or why it is not.
    pub fn signature(tokens: &[Tok]) -> Result<String, String> {
        let mut at = 0;
        // the attributes above it, doc comments included
        while tokens.get(at) == Some(&Tok::Punct('#')) {
            at += 1;
            if let Some(Tok::Punct('!')) = tokens.get(at) {
                at += 1;
            }
            match tokens.get(at) {
                Some(Tok::Group { delimiter: Delim::Bracket, .. }) => at += 1,
                _ => return Err(String::from("`#[preview]` must sit on a free function")),
            }
        }
        // the visibility
        if tokens.get(at) == Some(&Tok::Ident(String::from("pub"))) {
            at += 1;
            if let Some(Tok::Group { delimiter: Delim::Paren, .. }) = tokens.get(at) {
                at += 1;
            }
        }
        // the qualifiers a preview may or may not carry
        loop {
            match tokens.get(at) {
                Some(Tok::Ident(word)) if word == "const" => at += 1,
                Some(Tok::Ident(word)) if word == "async" => {
                    return Err(String::from("`#[preview]`: a preview is not `async` — it builds a view and returns"));
                }
                Some(Tok::Ident(word)) if word == "unsafe" => {
                    return Err(String::from("`#[preview]`: a preview is not `unsafe`"));
                }
                Some(Tok::Ident(word)) if word == "extern" => {
                    return Err(String::from("`#[preview]`: a preview is a Rust function, not `extern`"));
                }
                _ => break,
            }
        }
        if tokens.get(at) != Some(&Tok::Ident(String::from("fn"))) {
            return Err(String::from("`#[preview]` must sit on a free function"));
        }
        let Some(Tok::Ident(ident)) = tokens.get(at + 1) else {
            return Err(String::from("`#[preview]` must sit on a free function"));
        };
        at += 2;
        if tokens.get(at) == Some(&Tok::Punct('<')) {
            return Err(format!("`#[preview]`: `{ident}` takes type parameters; a preview takes none"));
        }
        match tokens.get(at) {
            Some(Tok::Group { delimiter: Delim::Paren, empty: true }) => {}
            Some(Tok::Group { delimiter: Delim::Paren, empty: false }) => {
                return Err(format!("`#[preview]`: `{ident}` takes parameters; a preview takes none"));
            }
            _ => return Err(String::from("`#[preview]` must sit on a free function")),
        }
        Ok(ident.clone())
    }

    /// The invocation the attribute writes after the function.
    pub fn invocation(args: &Args, ident: &str) -> String {
        let quoted = format!("{ident:?}");
        let name = args.name.clone().unwrap_or_else(|| quoted.clone());
        let example = args.example.clone().unwrap_or_else(|| quoted.clone());
        let scope = args.scope.clone().unwrap_or_else(|| String::from("\"\""));
        let krate = args.krate.clone().unwrap_or_else(|| String::from("::bunny_ui"));
        format!("{krate}::__bunny_preview_entry!({ident}, bunny_preview__{ident}, {name}, {scope}, {example});")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn ident(word: &str) -> Tok {
            Tok::Ident(word.to_string())
        }

        fn group(delimiter: Delim, empty: bool) -> Tok {
            Tok::Group { delimiter, empty }
        }

        fn plain_fn(name: &str) -> Vec<Tok> {
            vec![ident("fn"), ident(name), group(Delim::Paren, true), Tok::Punct('-'), Tok::Punct('>'), ident("impl"), ident("View"), group(Delim::Brace, false)]
        }

        #[test]
        fn a_plain_function_names_itself_everywhere() {
            let args = arguments(&[]).unwrap();
            assert_eq!(signature(&plain_fn("home")).unwrap(), "home");
            assert_eq!(
                invocation(&args, "home"),
                "::bunny_ui::__bunny_preview_entry!(home, bunny_preview__home, \"home\", \"\", \"home\");"
            );
        }

        #[test]
        fn attributes_doc_comments_and_visibility_are_walked_over() {
            let mut tokens = vec![
                Tok::Punct('#'),
                group(Delim::Bracket, false), // #[doc = "…"]
                Tok::Punct('#'),
                group(Delim::Bracket, false), // #[cfg(all())]
                ident("pub"),
                group(Delim::Paren, false), // pub(crate)
                ident("const"),
            ];
            tokens.extend(plain_fn("card"));
            assert_eq!(signature(&tokens).unwrap(), "card");
        }

        #[test]
        fn the_arguments_are_read_and_the_crate_path_kept() {
            let tokens = vec![
                ident("name"),
                Tok::Punct('='),
                Tok::Literal(String::from("\"Button\"")),
                Tok::Punct(','),
                ident("scope"),
                Tok::Punct('='),
                Tok::Literal(String::from("\"Input\"")),
                Tok::Punct(','),
                ident("example"),
                Tok::Punct('='),
                Tok::Literal(String::from("\"Primary\"")),
                Tok::Punct(','),
                ident("crate"),
                Tok::Punct('='),
                Tok::Punct(':'),
                Tok::Punct(':'),
                ident("ui"),
                Tok::Punct(','),
            ];
            let args = arguments(&tokens).unwrap();
            assert_eq!(args.name.as_deref(), Some("\"Button\""));
            assert_eq!(args.scope.as_deref(), Some("\"Input\""));
            assert_eq!(args.example.as_deref(), Some("\"Primary\""));
            assert_eq!(args.krate.as_deref(), Some("::ui"));
            assert_eq!(
                invocation(&args, "button_primary"),
                "::ui::__bunny_preview_entry!(button_primary, bunny_preview__button_primary, \"Button\", \"Input\", \"Primary\");"
            );
        }

        #[test]
        fn the_refusals_say_why() {
            let mut with_params = vec![ident("fn"), ident("f"), group(Delim::Paren, false)];
            assert!(signature(&with_params).unwrap_err().contains("takes parameters"));
            with_params[2] = Tok::Punct('<');
            assert!(signature(&with_params).unwrap_err().contains("type parameters"));
            assert!(signature(&[ident("struct"), ident("S"), Tok::Punct(';')]).unwrap_err().contains("free function"));
            assert!(signature(&[ident("async"), ident("fn"), ident("f"), group(Delim::Paren, true)]).unwrap_err().contains("async"));
            assert!(signature(&[ident("unsafe"), ident("fn"), ident("f"), group(Delim::Paren, true)]).unwrap_err().contains("unsafe"));
            assert!(signature(&[ident("extern"), ident("fn"), ident("f"), group(Delim::Paren, true)]).unwrap_err().contains("extern"));
            let unknown = [ident("colour"), Tok::Punct('='), Tok::Literal(String::from("\"x\""))];
            assert!(arguments(&unknown).unwrap_err().contains("unknown argument"));
            let not_a_string = [ident("name"), Tok::Punct('='), Tok::Literal(String::from("7"))];
            assert!(arguments(&not_a_string).unwrap_err().contains("string literal"));
            let twice = [
                ident("name"),
                Tok::Punct('='),
                Tok::Literal(String::from("\"a\"")),
                Tok::Punct(','),
                ident("name"),
                Tok::Punct('='),
                Tok::Literal(String::from("\"b\"")),
            ];
            assert!(arguments(&twice).unwrap_err().contains("twice"));
            let bare = [ident("name")];
            assert!(arguments(&bare).unwrap_err().contains("name = "));
            let no_path = [ident("crate"), Tok::Punct('=')];
            assert!(arguments(&no_path).unwrap_err().contains("names no path"));
        }
    }
}
