//! A block written for bunny-ui 0.5: a list of places, a page bar with a
//! button of its own, and the work that loads it again. `bunny migrate`
//! carries it to 0.6 — `tests/migrate/0.6/places.rs`, byte for byte.

use bunny_ui::prelude::*;

/// Where a link goes. In 0.5 a link wrote its `Debug` text on the path;
/// in 0.6 the route is the value the stack pushes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Lisbon,
    Porto,
}

#[derive(Clone, Copy)]
pub struct Places {
    visits: State<i32>,
}

impl Component for Places {
    fn body(self) -> impl View {
        // 0.5 read nav_link_value(value, label) and .refreshable(|| …);
        // the comment and the string below keep their words
        let note = "nav_link_value(value, label)";
        navigation_stack_content(vstack!(
            text(note),
            nav_link_value(text("Lisbon"), Place::Lisbon),
            nav_link_value(
                hstack!(text("Porto"), text!("{} visits", self.visits)),
                Place::Porto,
            ),
        )
        .navigation_title("Places")
        .toolbar(toolbar_item(button(text("Visit"), move || self.visits.add(1))))
        .refreshable(async move || self.visits.set(0)))
    }
}

/// A fresh list of places.
pub fn places() -> Places {
    Places { visits: State::new(0) }
}
