//! The rules: what changed in bunny-ui's API from one version to the
//! next, written as rewrites of the call sites that a person would make
//! by hand — and, where no rewrite is sure, a gap named with its line and
//! what to do about it.
//!
//! A rule reads tokens, never text: a comment, a string, or a function of
//! the same name being defined is not a call site.

use super::Version;
use super::lex::{self, Kind, Token};

/// One version's changes.
pub struct Step {
    pub from: Version,
    pub to: Version,
    pub rules: &'static [Rule],
}

/// Where a call is written.
#[derive(Clone, Copy, Debug)]
pub enum Call {
    /// `name(…)` or `path::name(…)`, not `.name(…)` and not `fn name(…)`.
    Free(&'static str),
    /// `.name(…)`.
    Method(&'static str),
}

/// What an argument of a reordered call becomes.
#[derive(Clone, Copy, Debug)]
pub enum Arg {
    /// The argument as written.
    Same(usize),
    /// The argument as a closure that makes it: `move || …`.
    Made(usize),
}

/// One change of the API.
#[derive(Clone, Copy, Debug)]
pub enum Rule {
    /// A name changed: every identifier `from` becomes `to`.
    Rename { from: &'static str, to: &'static str },
    /// A call's arguments change places, and maybe its name: `args[i]`
    /// says what the i-th argument is now. A call of another arity is
    /// someone else's function and is left alone.
    Reorder { call: Call, rename: Option<&'static str>, args: &'static [Arg] },
    /// The first argument is an `async` closure now: a closure gains
    /// `async`, anything else is a gap.
    Async { call: Call, note: &'static str },
    /// The argument is wrapped in `with(…)`, or each element of a tuple
    /// is, unless it already is.
    Wrap { call: Call, with: &'static str },
    /// The argument at `index` is one view now: a one-element tuple
    /// `(x,)` becomes `x`; a longer tuple is a gap.
    Unwrap { call: Call, index: usize, note: &'static str },
    /// The call answers a struct now: `field` is read after it, unless a
    /// field of `unless` already is.
    Field { call: Call, field: &'static str, unless: &'static [&'static str] },
    /// No rewrite is sure: every identifier `name` is a gap.
    GapName { name: &'static str, note: &'static str },
    /// No rewrite is sure: every call is a gap.
    GapCall { call: Call, note: &'static str },
    /// No rewrite is sure: every path `a::b` is a gap.
    GapPath { path: &'static [&'static str], note: &'static str },
}

/// Every step this `bunny` knows, oldest first.
pub const STEPS: &[Step] = &[
    Step {
        from: Version::new(0, 4),
        to: Version::new(0, 5),
        rules: &[
            Rule::Field { call: Call::Method("modifiers_changed"), field: "present", unless: &["present", "tap_taken"] },
            Rule::GapPath {
                path: &["KeyMatch", "None"],
                note: "`KeyMatch` has a new answer, `KeyMatch::Disabled` (a binding that means nothing here): a `match` that names every answer needs its arm, and spends the stroke as it does `Pending`.",
            },
        ],
    },
    Step {
        from: Version::new(0, 5),
        to: Version::new(0, 6),
        rules: &[
            Rule::Reorder { call: Call::Free("nav_link_value"), rename: None, args: &[Arg::Same(1), Arg::Same(0)] },
            Rule::Reorder { call: Call::Free("navigation_link"), rename: Some("nav_link"), args: &[Arg::Same(1), Arg::Made(0)] },
            Rule::Rename { from: "NavigationLink", to: "NavLink" },
            Rule::Wrap { call: Call::Method("toolbar"), with: "toolbar_item" },
            Rule::Async {
                call: Call::Method("refreshable"),
                note: "`.refreshable` takes an `async` closure now: `.refreshable(async move || model.reload().await)`.",
            },
            Rule::Unwrap {
                call: Call::Free("navigation_stack"),
                index: 1,
                note: "`navigation_stack(path, root)` takes ONE root view now: put these views in a stack (`vstack!`, `zstack!`) or pick the page.",
            },
            Rule::Unwrap {
                call: Call::Free("navigation_stack_content"),
                index: 0,
                note: "`navigation_stack_content(root)` takes ONE view now: put these views in a stack (`vstack!`, `zstack!`) or pick the page.",
            },
            Rule::GapName {
                name: "NavigationPath",
                note: "`NavigationPath` is gone: the path is the app's own list of routes, a `State<Vec<Route>>` or a `Binding<Vec<Route>>` of a type of yours, and `.destination(|route| …)` makes a route's page.",
            },
            Rule::GapCall {
                call: Call::Method("navigation_destination"),
                note: "`.navigation_destination()` is gone: the stack makes a route's page, `navigation_stack(path, root).destination(|route| …)`.",
            },
        ],
    },
];

/// A change of the source: the bytes `start..end` become `pieces`.
#[derive(Clone, Debug)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub pieces: Vec<Piece>,
}

/// Part of what an edit writes.
#[derive(Clone, Debug)]
pub enum Piece {
    Text(&'static str),
    /// Source bytes, with the edits inside them made too.
    Source(usize, usize),
}

/// A place no rule rewrites: the byte it starts at, the word found
/// there, and what to do.
#[derive(Clone, Debug)]
pub struct Found {
    pub at: usize,
    pub word: String,
    pub note: &'static str,
}

/// A call site: the name's token, its parentheses, its arguments.
struct Site {
    name: usize,
    close: usize,
    /// The token ranges of the arguments, a trailing comma's empty one
    /// left out.
    args: Vec<(usize, usize)>,
}

impl Rule {
    /// The edits this rule makes to `source`.
    pub fn edits(&self, source: &str, tokens: &[Token]) -> Vec<Edit> {
        let span = |(first, last): (usize, usize)| (tokens[first].start, tokens[last].end);
        let mut out = Vec::new();
        match *self {
            Rule::Rename { from, to } => {
                for token in tokens.iter().filter(|token| token.is_ident(source, from)) {
                    out.push(Edit { start: token.start, end: token.end, pieces: vec![Piece::Text(to)] });
                }
            }
            Rule::Reorder { call, rename, args } => {
                for site in sites(source, tokens, call) {
                    if site.args.len() != args.len() || args.is_empty() {
                        continue;
                    }
                    let spans: Vec<(usize, usize)> = site.args.iter().map(|arg| span(*arg)).collect();
                    let mut pieces = Vec::new();
                    let start = match rename {
                        Some(name) => {
                            pieces.push(Piece::Text(name));
                            pieces.push(Piece::Source(tokens[site.name].end, spans[0].0));
                            tokens[site.name].start
                        }
                        None => spans[0].0,
                    };
                    for (position, arg) in args.iter().enumerate() {
                        if position > 0 {
                            pieces.push(Piece::Source(spans[position - 1].1, spans[position].0));
                        }
                        let index = match *arg {
                            Arg::Same(index) => index,
                            Arg::Made(index) => {
                                pieces.push(Piece::Text("move || "));
                                index
                            }
                        };
                        pieces.push(Piece::Source(spans[index].0, spans[index].1));
                    }
                    out.push(Edit { start, end: spans[spans.len() - 1].1, pieces });
                }
            }
            Rule::Async { call, .. } => {
                for site in sites(source, tokens, call) {
                    if let [first, ..] = site.args[..]
                        && closure(source, tokens, first.0) == Some(false)
                    {
                        let at = tokens[first.0].start;
                        out.push(Edit { start: at, end: at, pieces: vec![Piece::Text("async ")] });
                    }
                }
            }
            Rule::Wrap { call, with } => {
                for site in sites(source, tokens, call) {
                    let [arg] = site.args[..] else { continue };
                    let elements = tuple(source, tokens, arg).unwrap_or_else(|| vec![arg]);
                    for element in elements {
                        if tokens[element.0].is_ident(source, with) && tokens.get(element.0 + 1).is_some_and(|t| t.is_punct('(')) {
                            continue;
                        }
                        let (start, end) = span(element);
                        out.push(Edit { start, end: start, pieces: vec![Piece::Text(with), Piece::Text("(")] });
                        out.push(Edit { start: end, end, pieces: vec![Piece::Text(")")] });
                    }
                }
            }
            Rule::Unwrap { call, index, .. } => {
                for site in sites(source, tokens, call) {
                    let Some(&arg) = site.args.get(index) else { continue };
                    if let Some(elements) = tuple(source, tokens, arg)
                        && let [element] = elements[..]
                    {
                        // `(` goes, and `,)` after the element
                        out.push(Edit { start: tokens[arg.0].start, end: tokens[element.0].start, pieces: Vec::new() });
                        out.push(Edit { start: tokens[element.1].end, end: tokens[arg.1].end, pieces: Vec::new() });
                    }
                }
            }
            Rule::Field { call, field, unless } => {
                for site in sites(source, tokens, call) {
                    let after = tokens.get(site.close + 1);
                    let next = tokens.get(site.close + 2);
                    let read = after.is_some_and(|t| t.is_punct('.')) && next.is_some_and(|t| unless.iter().any(|name| t.is_ident(source, name)));
                    if !read {
                        let at = tokens[site.close].end;
                        out.push(Edit { start: at, end: at, pieces: vec![Piece::Text("."), Piece::Text(field)] });
                    }
                }
            }
            Rule::GapName { .. } | Rule::GapCall { .. } | Rule::GapPath { .. } => {}
        }
        out
    }

    /// The places in `source` this rule cannot carry.
    pub fn gaps(&self, source: &str, tokens: &[Token]) -> Vec<Found> {
        let found = |token: &Token, note| Found { at: token.start, word: token.text(source).to_string(), note };
        let mut out = Vec::new();
        match *self {
            Rule::GapName { name, note } => {
                out.extend(tokens.iter().filter(|token| token.is_ident(source, name)).map(|token| found(token, note)));
            }
            Rule::GapCall { call, note } => {
                out.extend(sites(source, tokens, call).iter().map(|site| found(&tokens[site.name], note)));
            }
            Rule::GapPath { path, note } => {
                for (index, token) in tokens.iter().enumerate() {
                    if is_path(source, &tokens[index..], path) {
                        out.push(found(token, note));
                    }
                }
            }
            Rule::Async { call, note } => {
                for site in sites(source, tokens, call) {
                    if let [first, ..] = site.args[..]
                        && closure(source, tokens, first.0).is_none()
                    {
                        out.push(found(&tokens[site.name], note));
                    }
                }
            }
            Rule::Unwrap { call, index, note } => {
                for site in sites(source, tokens, call) {
                    if let Some(&arg) = site.args.get(index)
                        && tuple(source, tokens, arg).is_some_and(|elements| elements.len() > 1)
                    {
                        out.push(found(&tokens[site.name], note));
                    }
                }
            }
            Rule::Rename { .. } | Rule::Reorder { .. } | Rule::Wrap { .. } | Rule::Field { .. } => {}
        }
        out
    }
}

/// Whether `tokens` start with `path`'s names joined by `::`.
fn is_path(source: &str, tokens: &[Token], path: &[&str]) -> bool {
    let mut at = 0;
    for (index, name) in path.iter().enumerate() {
        if index > 0 {
            if !(tokens.get(at).is_some_and(|t| t.is_punct(':')) && tokens.get(at + 1).is_some_and(|t| t.is_punct(':'))) {
                return false;
            }
            at += 2;
        }
        if !tokens.get(at).is_some_and(|t| t.is_ident(source, name)) {
            return false;
        }
        at += 1;
    }
    true
}

/// Whether the argument starting at token `first` is a closure: `Some(true)`
/// for an `async` one, `Some(false)` for a plain one, `None` for anything
/// else.
fn closure(source: &str, tokens: &[Token], first: usize) -> Option<bool> {
    let mut at = first;
    let is_async = tokens[at].is_ident(source, "async");
    if is_async {
        at += 1;
    }
    if tokens.get(at).is_some_and(|t| t.is_ident(source, "move")) {
        at += 1;
    }
    tokens.get(at).is_some_and(|t| t.is_punct('|')).then_some(is_async)
}

/// The elements of an argument that is a tuple — `(` at its first token,
/// its matching `)` at its last, a comma inside — or `None`.
fn tuple(source: &str, tokens: &[Token], (first, last): (usize, usize)) -> Option<Vec<(usize, usize)>> {
    if !tokens[first].is_punct('(') || matching(tokens, first) != Some(last) {
        return None;
    }
    let (elements, commas) = split(source, tokens, first, last);
    (commas > 0).then_some(elements)
}

/// Every call of `call` in `tokens`.
fn sites(source: &str, tokens: &[Token], call: Call) -> Vec<Site> {
    let mut out = Vec::new();
    let name = match call {
        Call::Free(name) | Call::Method(name) => name,
    };
    for (index, token) in tokens.iter().enumerate() {
        if !token.is_ident(source, name) || !tokens.get(index + 1).is_some_and(|t| t.is_punct('(')) {
            continue;
        }
        let before = index.checked_sub(1).map(|i| &tokens[i]);
        let method = before.is_some_and(|t| t.is_punct('.'));
        let defined = before.is_some_and(|t| t.is_ident(source, "fn"));
        let wanted = match call {
            Call::Free(_) => !method && !defined,
            Call::Method(_) => method,
        };
        if !wanted {
            continue;
        }
        let open = index + 1;
        let Some(close) = matching(tokens, open) else { continue };
        let (args, _) = split(source, tokens, open, close);
        out.push(Site { name: index, close, args });
    }
    out
}

/// The token that closes the bracket opened at `open`.
fn matching(tokens: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            Kind::Punct('(' | '[' | '{') => depth += 1,
            Kind::Punct(')' | ']' | '}') => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The arguments between the brackets at `open` and `close`, as token
/// ranges, and how many top-level commas there were. A comma counts at
/// the top level only: not inside brackets, a turbofish's `<…>`, or a
/// closure's `|…|`.
fn split(source: &str, tokens: &[Token], open: usize, close: usize) -> (Vec<(usize, usize)>, usize) {
    let mut args = Vec::new();
    let mut commas = 0;
    let mut depth = 0usize;
    let mut angle = 0usize;
    let mut start = open + 1;
    let mut index = open + 1;
    while index < close {
        let token = tokens[index];
        // a closure's parameters at the start of an argument: `|a, b|`
        if index == start && depth == 0 && closure(source, tokens, index).is_some() {
            let open_bar = (index..close).find(|&i| tokens[i].is_punct('|'));
            let close_bar = open_bar.and_then(|bar| (bar + 1..close).find(|&i| tokens[i].is_punct('|')));
            if let Some(bar) = close_bar {
                index = bar + 1;
                continue;
            }
        }
        match token.kind {
            Kind::Punct('(' | '[' | '{') => depth += 1,
            Kind::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Kind::Punct('<') if angle > 0 || (index >= 2 && tokens[index - 1].is_punct(':') && tokens[index - 2].is_punct(':')) => angle += 1,
            // `->` inside a turbofish is no closing angle
            Kind::Punct('>') if angle > 0 && !(tokens[index - 1].is_punct('-') && tokens[index - 1].end == token.start) => angle -= 1,
            Kind::Punct(',') if depth == 0 && angle == 0 => {
                commas += 1;
                if start < index {
                    args.push((start, index - 1));
                }
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start < close {
        args.push((start, close - 1));
    }
    (args, commas)
}

/// `source` with `edits` made. Edits nest or stand apart; an edit inside
/// another's `Source` piece is made there.
pub fn apply(source: &str, mut edits: Vec<Edit>) -> String {
    // by start; at one start an insertion first, then the widest
    edits.sort_by_key(|edit| (edit.start, edit.end != edit.start, std::cmp::Reverse(edit.end)));
    let mut out = String::with_capacity(source.len() + 64);
    emit(source, &edits, 0, source.len(), &mut out);
    out
}

fn emit(source: &str, edits: &[Edit], start: usize, end: usize, out: &mut String) {
    let mut cursor = start;
    for edit in edits {
        if edit.start > end {
            break;
        }
        // before the range, inside an edit already made, or past the end
        if edit.start < cursor || edit.end > end {
            continue;
        }
        out.push_str(&source[cursor..edit.start]);
        for piece in &edit.pieces {
            match *piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Source(from, to) => emit(source, edits, from, to, out),
            }
        }
        cursor = edit.end;
    }
    out.push_str(&source[cursor..end]);
}

/// The tokens of `source`, for the rules.
pub fn tokens(source: &str) -> Vec<Token> {
    lex::tokens(source)
}
