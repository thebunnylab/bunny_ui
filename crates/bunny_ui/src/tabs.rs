//! Tabs: a bar that chooses one page, and the page it chooses.
//!
//! ```ignore
//! tab_view(self.pane.binding(), (
//!     tab(Pane::General, "General", general()),
//!     tab(Pane::Advanced, "Advanced", advanced()),
//! ))
//! ```
//!
//! The binding holds the value of the tab that is shown. A press on a tab
//! writes its value, and the page of that tab takes the place under the
//! bar. A value that is no tab shows no page, and the binding is never
//! written to repair it.
//!
//! Only the page that is shown is made. A page that is left is unmounted:
//! the state its views declared goes with it, as with any view that
//! leaves the scene. Keep what must outlive a change of tab above the
//! tab view.
//!
//! [`tabs`] is the bar alone, for an app that places the pages itself:
//!
//! ```ignore
//! tabs(self.pane.binding(), [(Pane::General, "General"), (Pane::Advanced, "Advanced")])
//! ```
//!
//! ## Keys
//!
//! The bar is ONE Tab stop, on the tab that is shown. The arrows move to
//! the next tab and show its page, and go round at the ends. Home and End
//! jump to the first and the last. In a right-to-left scene the
//! horizontal arrows swap. Tab then goes on into the page.
//!
//! ## Assistive technology
//!
//! Each tab is a [`Role::Tab`](crate::accessibility::Role) named by its
//! words, with `checked` true on the one that is shown. The activate
//! action shows its page.
//!
//! ## How it is made
//!
//! The bar is the row a segmented control is made of, with a line under
//! the selected tab where a segment has a thumb behind it. The tabs share
//! the width they are given; give the bar a width to keep it short.

use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context};

use crate::choice::{Bar, Choice, IntoChoices};
use crate::provenance::{NodeKind, Site};
use crate::view::{NodeList, Single, View, position_segment};
use crate::views::vstack;

/// A bar of tabs that chooses one value ([`tabs`]).
#[derive(Clone)]
pub struct Tabs<T: 'static> {
    selection: Binding<T>,
    choices: Rc<[Choice<T>]>,
    disabled: bool,
    site: Site,
}

impl<T: 'static> Tabs<T> {
    /// A disabled bar shows its selection and cannot change.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl<T: Clone + PartialEq + 'static> View for Tabs<T> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        crate::provenance::quiet(|| {
            Bar {
                selection: &self.selection,
                choices: &self.choices,
                disabled: self.disabled,
                tabs: true,
                site: self.site,
            }
            .render(ctx, out);
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Tabs, None);
    }
}

/// A bar of tabs alone:
/// `tabs(self.pane.binding(), [(Pane::General, "General"), (Pane::Advanced, "Advanced")])`.
///
/// The app shows the page for the value it reads. [`tab_view`] is the bar
/// and the pages together.
#[track_caller]
pub fn tabs<T, O>(selection: Binding<T>, options: O) -> Tabs<T>
where
    T: Clone + PartialEq + 'static,
    O: IntoChoices<T>,
{
    Tabs { selection, choices: options.into_choices(), disabled: false, site: crate::provenance::here() }
}

/// One tab of a [`tab_view`]: its value, its words and its page ([`tab`]).
#[derive(Clone)]
pub struct TabItem<T, C> {
    value: T,
    label: Arc<str>,
    content: C,
}

/// One tab of a [`tab_view`]: `tab(Pane::General, "General", general())`.
pub fn tab<T, C: View>(value: T, label: impl Into<Arc<str>>, content: C) -> TabItem<T, C> {
    TabItem { value, label: label.into(), content }
}

/// The tabs of a [`tab_view`]: a tuple of one to eight [`tab`]s.
pub trait TabPages<T>: Clone + 'static {
    /// The value and the words of each tab, in order.
    fn choices(&self) -> Rc<[Choice<T>]>;
    /// Renders the page of the tab at `index`.
    fn render_page(&self, index: usize, ctx: &Context, out: &mut NodeList);
}

macro_rules! tab_pages {
    ($($name:ident $index:tt),+) => {
        impl<T: Clone + 'static, $($name: View + Clone + 'static),+> TabPages<T> for ($(TabItem<T, $name>,)+) {
            fn choices(&self) -> Rc<[Choice<T>]> {
                Rc::from([$(Choice::new(self.$index.value.clone(), Arc::clone(&self.$index.label))),+])
            }

            fn render_page(&self, index: usize, ctx: &Context, out: &mut NodeList) {
                match index {
                    $($index => self.$index.content.render_into(ctx, out),)+
                    _ => {}
                }
            }
        }
    };
}

tab_pages!(A 0);
tab_pages!(A 0, B 1);
tab_pages!(A 0, B 1, C 2);
tab_pages!(A 0, B 1, C 2, D 3);
tab_pages!(A 0, B 1, C 2, D 3, E 4);
tab_pages!(A 0, B 1, C 2, D 3, E 4, F 5);
tab_pages!(A 0, B 1, C 2, D 3, E 4, F 5, G 6);
tab_pages!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7);

/// A bar of tabs over the page of the selected one ([`tab_view`]).
#[derive(Clone)]
pub struct TabView<T: 'static, P> {
    selection: Binding<T>,
    pages: P,
    disabled: bool,
    site: Site,
}

impl<T: 'static, P> TabView<T, P> {
    /// A disabled tab view keeps the page it shows and cannot change tab.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The bar of a tab view, as a view of its own inside the column.
#[derive(Clone)]
struct BarOf<T: 'static> {
    selection: Binding<T>,
    choices: Rc<[Choice<T>]>,
    disabled: bool,
    site: Site,
}

impl<T: Clone + PartialEq + 'static> View for BarOf<T> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        Bar { selection: &self.selection, choices: &self.choices, disabled: self.disabled, tabs: true, site: self.site }
            .render(ctx, out);
    }
}

/// The page of the selected tab. Each page has an identity of its own by
/// its place, so the state of one page is never the state of another.
#[derive(Clone)]
struct PageOf<T, P> {
    pages: P,
    selected: Option<usize>,
    of: std::marker::PhantomData<T>,
}

impl<T: Clone + 'static, P: TabPages<T>> View for PageOf<T, P> {
    type Arity = crate::view::Many;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let Some(index) = self.selected else {
            return;
        };
        let _page = motor::identity::enter(position_segment(index));
        // the page is the app's own view inside the tab view's quiet scope
        crate::provenance::written(|| self.pages.render_page(index, ctx, out));
    }
}

impl<T: Clone + PartialEq + 'static, P: TabPages<T>> View for TabView<T, P> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        crate::provenance::quiet(|| {
            let choices = self.pages.choices();
            let current = self.selection.wrappedValue();
            let selected = choices.iter().position(|choice| *choice.value() == current);
            let bar = BarOf { selection: self.selection.clone(), choices, disabled: self.disabled, site: self.site };
            let page = PageOf { pages: self.pages.clone(), selected, of: std::marker::PhantomData };
            vstack((bar, page)).spacing(0.0).render_into(ctx, out);
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::TabView, None);
    }
}

/// A bar of tabs over the page of the selected one:
///
/// ```ignore
/// tab_view(self.pane.binding(), (
///     tab(Pane::General, "General", general()),
///     tab(Pane::Advanced, "Advanced", advanced()),
/// ))
/// ```
///
/// [The module](self) has the keys, the roles and what happens to a page
/// that is left.
#[track_caller]
pub fn tab_view<T, P>(selection: Binding<T>, pages: P) -> TabView<T, P>
where
    T: Clone + PartialEq + 'static,
    P: TabPages<T>,
{
    TabView { selection, pages, disabled: false, site: crate::provenance::here() }
}
