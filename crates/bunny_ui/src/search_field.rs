//! The search field: a text field with a magnifier and a button that
//! empties it.
//!
//! ```ignore
//! search_field(self.query.binding())
//! ```
//!
//! The binding holds the query. The app reads the same binding to filter
//! what it shows; the field filters nothing by itself.
//!
//! ```ignore
//! let shown = items.iter().filter(|item| item.name.contains(&*self.query.get()));
//! ```
//!
//! The field shows a prompt while it is empty: the word "Search" in the
//! reader's language, or the one [`SearchField::prompt`] names. While it
//! holds text, a button at the trailing edge empties it.
//!
//! ## Keys
//!
//! The field is a text field: every editing key is its own. Enter runs
//! [`SearchField::on_submit`] when the app gave one. Tab reaches the
//! field, and then the button while the field holds text. A press on the
//! button empties the field and leaves the keyboard where it was.
//!
//! ## Assistive technology
//!
//! A text field named by its prompt, and a button named "Clear" in the
//! reader's language.
//!
//! ## How it is made
//!
//! A bare text field between an icon and a button, inside a box that
//! wears the field's own colours and the focus colour on its border
//! while the field has the keyboard. On the web the field is a real
//! `<input>`, as every text field is.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context, Locale, State};

use crate::beat::Beat;
use crate::ext::ViewExt;
use crate::icon::house as symbol;
use crate::layout::{Color, Edges};
use crate::provenance::{NodeKind, Quiet, Site};
use crate::state_ext::StateExt;
use crate::text_input::KeyboardType;
use crate::view::{Component, NodeList, Single, View};
use crate::views::{button, hstack, icon, text_field};
use crate::words::{Word, Words};

const RADIUS: f64 = 6.0;
/// Between the box's edge and what it holds.
const INSET: f64 = 6.0;
/// Between the icon, the text and the button.
const GAP: f64 = 6.0;

/// A text field for a query ([`search_field`]).
#[derive(Clone)]
pub struct SearchField {
    text: Binding<String>,
    prompt: Option<Arc<str>>,
    submit: Option<Rc<dyn Fn()>>,
    /// The beat the field answers, and whether a ring selects its text.
    summon: Option<(Beat, bool)>,
    site: Site,
}

impl SearchField {
    /// The field takes the keyboard on each new ring of `beat`
    /// ([`TextField::focus_on`](crate::views::TextField::focus_on)).
    #[must_use]
    pub fn focus_on(mut self, beat: Beat) -> Self {
        self.summon = Some((beat, false));
        self
    }

    /// [`SearchField::focus_on`], and each ring also selects the whole
    /// query, so the next keys replace it: what a "find" shortcut wants.
    #[must_use]
    pub fn select_on(mut self, beat: Beat) -> Self {
        self.summon = Some((beat, true));
        self
    }

    /// The words shown while the field is empty, in place of "Search".
    /// They are also the field's name for assistive technology.
    #[must_use]
    pub fn prompt(mut self, prompt: impl Into<Arc<str>>) -> Self {
        self.prompt = Some(prompt.into());
        self
    }

    /// Runs when the reader presses Enter in the field.
    #[must_use]
    pub fn on_submit(mut self, submit: impl Fn() + 'static) -> Self {
        self.submit = Some(Rc::new(submit));
        self
    }
}

impl View for SearchField {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        let words = Words::for_locale(&ctx.environment::<Locale>());
        Body {
            text: self.text.clone(),
            prompt: self.prompt.clone().unwrap_or_else(|| Arc::from(&*words.get(Word::Search))),
            clear: Arc::from(&*words.get(Word::Clear)),
            submit: self.submit.clone(),
            summon: self.summon,
        }
        .render_into(ctx, out);
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::SearchField, None);
    }
}

/// The field's own body: it holds whether the field has the keyboard, so
/// the border changes without a body of the app running.
#[derive(Clone)]
struct Body {
    text: Binding<String>,
    prompt: Arc<str>,
    clear: Arc<str>,
    submit: Option<Rc<dyn Fn()>>,
    summon: Option<(Beat, bool)>,
}

impl Component for Body {
    fn body(self) -> impl View {
        let focused = State::new(false);
        let theme = crate::theme::current();
        let field = text_field(&*self.prompt, self.text.clone())
            .bare()
            .content_inset(Edges { top: 0.0, bottom: 0.0, leading: 0.0, trailing: 0.0 })
            .keyboard_type(KeyboardType::Search)
            .on_focus(move |has| focused.set(has));
        let field = match self.submit {
            Some(submit) => field.on_submit(move || submit()),
            None => field,
        };
        let field = match self.summon {
            Some((beat, true)) => field.select_on(beat),
            Some((beat, false)) => field.focus_on(beat),
            None => field,
        };
        // the button exists only while there is something to empty
        let filled = self.text.with(|text| !text.is_empty());
        let empty = self.text.clone();
        let clear = filled.then(|| {
            button(icon(symbol::CLOSE).foreground_color(theme.fg_secondary), move || empty.set(String::new()))
                .plain()
                .accessibility_label(&*self.clear)
                .leaves_keyboard()
        });
        let border: Color = if focused.get() { theme.focus } else { theme.field_border };
        Quiet(
            hstack((icon(symbol::SEARCH).foreground_color(theme.fg_secondary), field, clear))
                .spacing(GAP)
                .padding_length(INSET)
                .background_color(theme.field)
                .border(border, 1.0)
                .corner_radius(RADIUS),
        )
    }
}

/// A text field for a query: `search_field(self.query.binding())`.
///
/// [The module](self) has the prompt, the keys and how it is made.
#[track_caller]
pub fn search_field(text: Binding<String>) -> SearchField {
    SearchField { text, prompt: None, submit: None, summon: None, site: crate::provenance::here() }
}
