//! Keyboard loans: the keys a field's BEAT took go back where they came
//! from.
//!
//! `.auto_focus_beat(n)` is an app's intent — "this field takes the
//! keyboard now, from whoever holds it". The fields that ask are the
//! transient ones: a picker's query over an editor, a find bar, a filter
//! a chord summons. They leave the scene still holding the keys (Escape
//! closes the popup; a pick runs and the popup goes), and with no road
//! back the keyboard lands on nobody: the next keystroke goes nowhere
//! until the reader clicks where they already were.
//!
//! So a beat BORROWS. The runtime writes down who held the keyboard when
//! the beat took it, and when the borrower leaves the scene still holding
//! it, the keyboard goes home:
//!
//! - **Only a holder returns it.** A borrower the keyboard has already
//!   left (a click elsewhere, another beat) gives nothing back — the
//!   reader's own move stands.
//! - **The first live lender wins.** A borrower can lend in turn (a
//!   popup's query opens another popup's), so the road home walks the
//!   chain to the first input still on screen.
//! - **A lender that left is not revived.** When nothing on the chain
//!   lives, the keyboard lands on nobody, as it always did.
//! - **A first appearance borrows nothing.** `.auto_focus()` takes only a
//!   keyboard nobody holds, so nobody is owed it.
//!
//! The book holds identity paths, like the focus itself: an input that
//! moves house (a named one re-parented by a split) takes its loans with
//! it.

use motor::hash::FxHashMap as HashMap;

/// Who lent the keyboard to whom — borrower path → lender path.
#[derive(Default)]
pub(crate) struct Loans {
    lent: HashMap<String, String>,
}

impl Loans {
    /// `borrower` took the keyboard on a beat from `lender`.
    pub(crate) fn lend(&mut self, borrower: &str, lender: &str) {
        self.lent.insert(borrower.to_owned(), lender.to_owned());
    }

    /// Where the keyboard goes when `borrower` leaves holding it: the
    /// first lender on its chain that `lives` — `None` when it borrowed
    /// nothing, or when every lender has left.
    pub(crate) fn heir(&self, borrower: &str, lives: impl Fn(&str) -> bool) -> Option<&str> {
        let mut at = self.lent.get(borrower)?;
        // bounded by the book: no chain is longer than the loans written,
        // and a cycle ends here instead of spinning
        for _ in 0..self.lent.len() {
            if lives(at) {
                return Some(at);
            }
            at = self.lent.get(at)?;
        }
        None
    }

    /// Forgets the borrowers that left. One that lent in turn hands its
    /// own lender down, so a chain survives its middle leaving first.
    pub(crate) fn settle(&mut self, lives: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self.lent.keys().filter(|borrower| !lives(borrower)).cloned().collect();
        for borrower in gone {
            // read at removal, not at collection: an earlier removal in
            // this walk may have re-pointed this very entry
            let Some(lender) = self.lent.remove(&borrower) else {
                continue;
            };
            for next in self.lent.values_mut() {
                if *next == borrower {
                    next.clone_from(&lender);
                }
            }
        }
        self.lent.retain(|borrower, lender| borrower != lender);
    }

    /// An input moved house: its loans follow it, both ways.
    pub(crate) fn follow(&mut self, from: &str, to: &str) {
        if let Some(lender) = self.lent.remove(from) {
            self.lent.insert(to.to_owned(), lender);
        }
        for lender in self.lent.values_mut() {
            if lender == from {
                to.clone_into(lender);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Loans;

    fn alive<'a>(paths: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |path| paths.contains(&path)
    }

    #[test]
    fn the_heir_is_the_first_lender_still_on_screen() {
        let mut book = Loans::default();
        book.lend("inner", "outer");
        book.lend("outer", "editor");
        assert_eq!(book.heir("inner", alive(&["outer", "editor"])), Some("outer"));
        assert_eq!(book.heir("inner", alive(&["editor"])), Some("editor"), "a lender that left is walked past");
        assert_eq!(book.heir("inner", alive(&[])), None, "nothing on the chain lives: nothing is revived");
        assert_eq!(book.heir("editor", alive(&["editor"])), None, "an input that borrowed nothing is owed nothing");
    }

    #[test]
    fn a_borrower_that_left_hands_its_lender_down() {
        let mut book = Loans::default();
        book.lend("inner", "middle");
        book.lend("middle", "outer");
        book.lend("outer", "editor");
        // two links leave at once, in whatever order the map walks them
        book.settle(alive(&["inner", "editor"]));
        assert_eq!(book.heir("inner", alive(&["editor"])), Some("editor"));
        assert_eq!(book.heir("middle", alive(&["editor"])), None, "a borrower that left is forgotten");
    }

    #[test]
    fn a_cycle_ends_the_walk() {
        let mut book = Loans::default();
        book.lend("a", "b");
        book.lend("b", "a");
        assert_eq!(book.heir("a", alive(&[])), None);
    }

    #[test]
    fn an_input_that_moves_house_takes_its_loans_both_ways() {
        let mut book = Loans::default();
        book.lend("query", "editor");
        book.lend("inner", "query");
        book.follow("query", "split/query");
        assert_eq!(book.heir("split/query", alive(&["editor"])), Some("editor"));
        assert_eq!(book.heir("inner", alive(&["split/query"])), Some("split/query"));
        assert_eq!(book.heir("query", alive(&["editor"])), None, "the old house owes nothing");
    }
}
