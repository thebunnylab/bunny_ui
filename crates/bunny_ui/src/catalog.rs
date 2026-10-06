//! A table per language, picked once by the locale — the mechanism
//! behind the framework's own words ([`crate::words`]), offered to an
//! app for its own.
//!
//! The DATA is plain: a [`Table`] is a BCP-47 tag and a fixed array of
//! entries in the order of the keys; the first table is the source
//! language and the fallback; an empty entry means "not yet" and falls
//! to the source. A generator can write it, a reviewer can diff it, and
//! the compiler checks every table's length against the keys — a table
//! one word short does not build. The DECISION happens once:
//! [`Catalog::pick`] chooses a table by [`Locale::pick_in`], and from
//! then on [`Strings::get`] is an index into it.
//!
//! ```ignore
//! #[derive(Clone, Copy)]
//! enum Label { Save, Cancel }
//! impl catalog::Key for Label {
//!     const COUNT: usize = 2;
//!     fn index(self) -> usize { self as usize }
//! }
//! static LABELS: Catalog<Label, 2> = Catalog::new(&[
//!     catalog::Table { tag: "en", entries: ["Save", "Cancel"] },
//!     catalog::Table { tag: "pt", entries: ["Salvar", "Cancelar"] },
//! ]);
//!
//! // in a body — or once, at the root, injected for every row below
//! let strings = LABELS.pick(&ctx.environment::<Locale>());
//! button(text(strings.get(Label::Save)), save)
//! ```
//!
//! Nothing here reads a file: the tables are the app's to embed, in
//! the shape its tools write them.

use std::marker::PhantomData;

use motor::state::Locale;

/// What indexes a table: an enum of the app's keys, numbered in the
/// order its tables list them.
pub trait Key: Copy + 'static {
    /// How many keys there are — the length of every table.
    const COUNT: usize;
    /// The key's row.
    fn index(self) -> usize;
}

/// One language's entries, in the keys' order. `tag` is BCP-47; an
/// empty entry has no translation yet and reads as the source's.
pub struct Table<const N: usize> {
    pub tag: &'static str,
    pub entries: [&'static str; N],
}

/// The tables of one set of keys — one per language, the source first.
pub struct Catalog<K: Key, const N: usize> {
    tables: &'static [Table<N>],
    _key: PhantomData<fn() -> K>,
}

impl<K: Key, const N: usize> Catalog<K, N> {
    /// `tables`, the source language first. A table whose length is not
    /// the keys' count, or a catalog with no table at all, does not
    /// build.
    pub const fn new(tables: &'static [Table<N>]) -> Self {
        const {
            assert!(N == K::COUNT, "a catalog's tables hold one entry per key");
        }
        assert!(!tables.is_empty(), "a catalog holds at least its source language");
        Catalog { tables, _key: PhantomData }
    }

    /// The languages spoken, in the tables' order.
    pub fn tags(&self) -> impl Iterator<Item = &'static str> + Clone + '_ {
        self.tables.iter().map(|table| table.tag)
    }

    /// The tables themselves, for a tool that lists or checks them.
    pub fn tables(&self) -> &'static [Table<N>] {
        self.tables
    }

    /// The table that serves `locale` best ([`Locale::pick_in`]), or the
    /// source when none speaks any of its languages. Decided once;
    /// every [`Strings::get`] after it is an index.
    pub fn pick(&self, locale: &Locale) -> Strings<K, N> {
        let index = locale.pick_in(self.tags()).unwrap_or(0);
        Strings { table: &self.tables[index], source: &self.tables[0], _key: PhantomData }
    }
}

/// A catalog's table, picked: what a body reads from.
#[derive(Clone, Copy)]
pub struct Strings<K: Key, const N: usize> {
    table: &'static Table<N>,
    source: &'static Table<N>,
    _key: PhantomData<fn() -> K>,
}

impl<K: Key, const N: usize> Strings<K, N> {
    /// The tag of the table that answered.
    pub fn tag(&self) -> &'static str {
        self.table.tag
    }

    /// The entry for `key` — the source's where this table has none yet.
    pub fn get(&self, key: K) -> &'static str {
        let index = key.index();
        let entry = self.table.entries[index];
        if entry.is_empty() { self.source.entries[index] } else { entry }
    }
}

/// `{name}` in `template` becomes its value from `values`; a name not
/// given stays as written, braces and all. One pass over the template,
/// the result sized up front.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let grown: usize = values.iter().map(|(_, value)| value.len()).sum();
    let mut out = String::with_capacity(template.len() + grown);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let name = &after[..close];
        match values.iter().find(|(known, _)| *known == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Label {
        Save,
        Cancel,
    }

    impl Key for Label {
        const COUNT: usize = 2;
        fn index(self) -> usize {
            self as usize
        }
    }

    static LABELS: Catalog<Label, 2> = Catalog::new(&[
        Table { tag: "en", entries: ["Save", "Cancel"] },
        Table { tag: "pt", entries: ["Salvar", "Cancelar"] },
        Table { tag: "pt-PT", entries: ["Guardar", ""] },
    ]);

    #[test]
    fn a_catalog_picks_the_table_the_locale_asks_for() {
        let strings = LABELS.pick(&Locale::new("pt-BR"));
        assert_eq!(strings.tag(), "pt");
        assert_eq!(strings.get(Label::Save), "Salvar");
        assert_eq!(LABELS.pick(&Locale::new("pt-PT")).get(Label::Save), "Guardar");
        assert_eq!(LABELS.pick(&Locale::parse("fr,pt")).tag(), "pt", "the second preference speaks");
        assert_eq!(LABELS.tags().collect::<Vec<_>>(), ["en", "pt", "pt-PT"]);
        assert_eq!(LABELS.tables().len(), 3);
    }

    #[test]
    fn an_unknown_language_falls_to_the_first_table() {
        let strings = LABELS.pick(&Locale::new("ja"));
        assert_eq!(strings.tag(), "en");
        assert_eq!(strings.get(Label::Cancel), "Cancel");
    }

    #[test]
    fn an_empty_entry_falls_to_the_first_tables_word() {
        let strings = LABELS.pick(&Locale::new("pt-PT"));
        assert_eq!(strings.get(Label::Cancel), "Cancel", "not yet translated: the source speaks");
        assert_eq!(strings.get(Label::Save), "Guardar");
    }

    #[test]
    fn fill_puts_the_name_where_the_template_says() {
        assert_eq!(fill("{app} beenden", &[("app", "Bunny")]), "Bunny beenden");
        assert_eq!(fill("Quit {app}", &[("app", "Bunny")]), "Quit Bunny");
        assert_eq!(fill("{app}を終了", &[("app", "Bunny")]), "Bunnyを終了");
        assert_eq!(fill("{a} and {b}", &[("a", "1"), ("b", "2")]), "1 and 2");
        assert_eq!(fill("plain", &[("app", "Bunny")]), "plain");
    }

    #[test]
    fn fill_keeps_a_name_it_was_not_given() {
        assert_eq!(fill("Quit {app}", &[]), "Quit {app}");
        assert_eq!(fill("{x} {app}", &[("app", "Bunny")]), "{x} Bunny");
        assert_eq!(fill("a { b", &[("b", "2")]), "a { b", "an open brace alone is text");
    }
}
