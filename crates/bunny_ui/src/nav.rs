//! Navigation: a stack of pages, one on top of the other.
//!
//! ```ignore
//! #[derive(Clone, PartialEq)]
//! enum Route { Country(Country), About }
//!
//! navigation_stack(self.path, self.list())            // path: State<Vec<Route>>
//!     .destination(|route| match route {
//!         Route::Country(country) => Either::First(CountryDetails::new(country)),
//!         Route::About => Either::Second(About),
//!     })
//! ```
//!
//! The path is the app's: a list of routes, in a `State` or behind a
//! `Binding`. The stack shows the root while the list is empty, and the
//! page of the last route while it is not. `destination` makes the page
//! of a route. Nothing else is kept anywhere: what the list holds is what
//! the reader sees.
//!
//! ```ignore
//! self.path.push(Route::About);                   // a page goes on top
//! self.path.pop();                                // it leaves
//! self.path.pop_to_root();
//! self.path.set(vec![Route::Country(usa)]);       // a deep link is one write
//! ```
//!
//! A link pushes a route on the stack it stands in, with no path in hand:
//!
//! ```ignore
//! nav_link_value(CountryCell::new(country.clone()), Route::Country(country.clone()))
//! ```
//!
//! ## What a page keeps
//!
//! A page under another one is still mounted. Its state, its scroll
//! offset and its running tasks are as the reader left them, and they are
//! there when the page comes back. It is not on the screen: it takes no
//! press, no key and no place in the Tab order, and assistive technology
//! does not hear it. The keys and the actions it declared answer nothing
//! while it is covered.
//!
//! A page is the page of its route AT ITS PLACE. A path that changes from
//! `[a, b]` to `[a, c]` keeps the page of `a` and makes a new one for
//! `c`; the page of `b` leaves with its state.
//!
//! ## The bar
//!
//! ```ignore
//! CountryDetails::new(country).navigation_title("Details")
//! ```
//!
//! The stack draws a bar over the page on top: the button that goes
//! back, where there is a page to leave, and the title the page gave
//! itself with `.navigation_title(…)`. The title is the page's, so it
//! may read the page's state and it changes with it. The button is named
//! "Back" in the reader's language, and it is the first Tab stop of a
//! page.
//!
//! ```ignore
//! page.toolbar((toolbar_item(edit_button()).leading(), toolbar_item(add_button())))
//! ```
//!
//! A page puts views of its own in the bar with `.toolbar(…)`. An item
//! stands at the trailing edge, or after the button that goes back when
//! it is `.leading()`. The items are the page's too: a page says them
//! each time its body runs, and the bar shows the last ones it said.
//!
//! A root that says no title and no item has no bar.
//!
//! ## Back
//!
//! The platform's "back" takes the page on top away
//! ([`Runtime::navigate_back`](crate::runtime::Runtime::navigate_back)),
//! after the menus and the overlays had their turn. With two stacks on
//! the screen, the inner one goes back while it has a page to leave.
//! The keyboard returns to the link that opened the page, when the page
//! leaves holding it.

use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context, LayoutDirection, Locale, Navigator, State};
use motor::view::RenderNode;
use motor::views::Edge;

use crate::action::NAV_BACK;
use crate::ext::ViewExt;
use crate::icon::house as symbol;
use crate::layout::{ControlKind, LayoutNode, Truncation};
use crate::provenance::{NodeKind, Site};
use crate::state_ext::StateExt;
use crate::view::{Component, NodeList, Single, View, print_label};
use crate::views::{Alignment, ControlSpec, EmptyView, button, control, empty, hstack, icon, spacer, text, vstack, wrap_layout};
use crate::words::{Word, Words};

/// What a stack's path is given as: the app's `State<Vec<R>>`, or a
/// `Binding<Vec<R>>` to routes that live somewhere else.
pub trait IntoRoutes<R> {
    /// The binding the stack reads and writes.
    fn into_routes(self) -> Binding<Vec<R>>;
}

impl<R: Clone + 'static> IntoRoutes<R> for State<Vec<R>> {
    fn into_routes(self) -> Binding<Vec<R>> {
        self.binding()
    }
}

impl<R> IntoRoutes<R> for Binding<Vec<R>> {
    fn into_routes(self) -> Binding<Vec<R>> {
        self
    }
}

/// The words a path is written with: `self.path.push(route)`.
pub trait NavPathExt<R> {
    /// Puts the page of `route` on top.
    fn push(&self, route: R);
    /// Takes the page on top away. `false` on the root, which stays.
    fn pop(&self) -> bool;
    /// Leaves only the root. `false` when it was alone already.
    fn pop_to_root(&self) -> bool;
}

impl<R: Clone + 'static> NavPathExt<R> for Binding<Vec<R>> {
    fn push(&self, route: R) {
        self.update(|routes| routes.push(route));
    }

    fn pop(&self) -> bool {
        self.modify(|routes| routes.pop().is_some())
    }

    fn pop_to_root(&self) -> bool {
        self.modify(|routes| {
            let had = !routes.is_empty();
            routes.clear();
            had
        })
    }
}

impl<R: Clone + 'static> NavPathExt<R> for State<Vec<R>> {
    fn push(&self, route: R) {
        self.update(|routes| routes.push(route));
    }

    fn pop(&self) -> bool {
        self.update_if(|routes| {
            let popped = routes.pop().is_some();
            (popped, popped)
        })
    }

    fn pop_to_root(&self) -> bool {
        self.update_if(|routes| {
            let had = !routes.is_empty();
            routes.clear();
            (had, had)
        })
    }
}

/// A stack of pages ([`navigation_stack`]).
pub struct NavigationStack<R: 'static, Root, V> {
    path: Binding<Vec<R>>,
    root: Root,
    destination: Rc<dyn Fn(R) -> V>,
    site: Site,
}

impl<R: Clone + 'static, Root: Clone, V> Clone for NavigationStack<R, Root, V> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            root: self.root.clone(),
            destination: Rc::clone(&self.destination),
            site: self.site,
        }
    }
}

impl<R: 'static, Root, V> NavigationStack<R, Root, V> {
    /// The page of a route. A `match` over the routes answers one view
    /// type with [`Either`](crate::view::Either), or `OneOf3` to `OneOf8`
    /// for more arms.
    #[must_use]
    pub fn destination<W: View, F: Fn(R) -> W + 'static>(self, destination: F) -> NavigationStack<R, Root, W> {
        NavigationStack { path: self.path, root: self.root, destination: Rc::new(destination), site: self.site }
    }
}

impl<R, Root, V> View for NavigationStack<R, Root, V>
where
    R: Clone + PartialEq + 'static,
    Root: View + Clone + 'static,
    V: View + 'static,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        Pages { path: self.path.clone(), root: self.root.clone(), destination: Rc::clone(&self.destination) }
            .render_into(ctx, out);
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::NavigationStack, None);
    }
}

/// The page of each route the path held when the stack last ran.
struct Kept<R> {
    pages: Vec<(R, u64)>,
    made: u64,
}

impl<R: Clone + PartialEq> Kept<R> {
    /// The number of the page of each route. The routes the path still
    /// starts with keep their pages; from the first route that changed,
    /// every page is a new one.
    fn numbers(&mut self, routes: &[R]) -> Vec<u64> {
        let same = self.pages.iter().zip(routes).take_while(|((was, _), now)| was == *now).count();
        self.pages.truncate(same);
        for route in &routes[same..] {
            self.made += 1;
            self.pages.push((route.clone(), self.made));
        }
        self.pages.iter().map(|(_, number)| *number).collect()
    }
}

/// The stack's memory, shared by the runs of its body. Reading it never
/// makes the body run: it is written inside the body, and only there.
struct Shelf<R> {
    kept: Rc<RefCell<Kept<R>>>,
    /// True while the stack makes its pages. A page that speaks then is
    /// read by the stack in the same run; one that speaks at another time
    /// has to make the stack run.
    making: Rc<Cell<bool>>,
}

impl<R> Clone for Shelf<R> {
    fn clone(&self) -> Self {
        Self { kept: Rc::clone(&self.kept), making: Rc::clone(&self.making) }
    }
}

/// What a view of a page tells the stack the page stands in. It travels
/// through [`Navigator::tell`], which knows no type of this crate.
pub(crate) enum PageWord {
    /// `.navigation_title(…)`.
    Title(Arc<str>),
    /// `.toolbar(…)`.
    Items(BarItems),
}

/// One view of a bar, made when the bar is.
type BarItem = Rc<dyn Fn(&Context, &mut NodeList)>;

/// The views a page puts in its bar, by the side they stand on.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct BarItems {
    leading: Vec<BarItem>,
    trailing: Vec<BarItem>,
}

impl BarItems {
    fn is_empty(&self) -> bool {
        self.leading.is_empty() && self.trailing.is_empty()
    }

    fn take(&mut self, more: BarItems) {
        self.leading.extend(more.leading);
        self.trailing.extend(more.trailing);
    }
}

/// What a page told its stack, kept in the page's own scope.
#[derive(Clone, Default)]
struct Said {
    title: Option<Arc<str>>,
    items: BarItems,
}

#[derive(Clone, Default)]
struct Told(Rc<RefCell<Said>>);

impl Told {
    /// Keeps the word. `true` when what the bar shows is different now.
    fn hear(&self, word: &PageWord) -> bool {
        let mut said = self.0.borrow_mut();
        match word {
            PageWord::Title(title) if said.title.as_ref() == Some(title) => false,
            PageWord::Title(title) => {
                said.title = Some(Arc::clone(title));
                true
            }
            // two views are not compared: new items are different items
            PageWord::Items(items) => {
                said.items = items.clone();
                true
            }
        }
    }
}

/// One view of a page's bar ([`toolbar_item`]).
#[derive(Clone)]
pub struct ToolbarItem<C> {
    content: C,
    leading: bool,
}

impl<C> ToolbarItem<C> {
    /// The item stands at the leading side of the bar, after the button
    /// that goes back. An item stands at the trailing side when this is
    /// not said.
    #[must_use]
    pub fn leading(mut self) -> Self {
        self.leading = true;
        self
    }
}

/// One view of a page's bar: `toolbar_item(button(text("Add"), add))`.
///
/// The view is any view. `None` is no item, so
/// `toolbar_item(self.can_edit.then(|| edit_button()))` is an item that
/// is there only while the page allows it.
pub fn toolbar_item<C: View>(content: C) -> ToolbarItem<C> {
    ToolbarItem { content, leading: false }
}

/// What `.toolbar(…)` takes: one [`toolbar_item`], or a tuple of up to
/// eight.
pub trait ToolbarItems {
    #[doc(hidden)]
    fn into_bar(self) -> BarItems;
}

impl<C: View> ToolbarItems for ToolbarItem<C> {
    fn into_bar(self) -> BarItems {
        let content = self.content;
        let item: BarItem = Rc::new(move |ctx, out| content.render_into(ctx, out));
        if self.leading {
            BarItems { leading: vec![item], trailing: Vec::new() }
        } else {
            BarItems { leading: Vec::new(), trailing: vec![item] }
        }
    }
}

macro_rules! toolbar_items {
    ($($item:ident),+) => {
        impl<$($item: ToolbarItems),+> ToolbarItems for ($($item,)+) {
            fn into_bar(self) -> BarItems {
                #[allow(non_snake_case)]
                let ($($item,)+) = self;
                let mut all = BarItems::default();
                $(all.take($item.into_bar());)+
                all
            }
        }
    };
}
toolbar_items!(A);
toolbar_items!(A, B);
toolbar_items!(A, B, C);
toolbar_items!(A, B, C, D);
toolbar_items!(A, B, C, D, E);
toolbar_items!(A, B, C, D, E, F);
toolbar_items!(A, B, C, D, E, F, G);
toolbar_items!(A, B, C, D, E, F, G, H);

/// The items of one side of a bar, each under its own key.
#[derive(Clone)]
struct Items(Vec<BarItem>, &'static str);

impl View for Items {
    type Arity = crate::view::Many;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        for (place, item) in self.0.iter().enumerate() {
            let _frame = motor::identity::enter_key(&format!("{}{place}", self.1));
            item(ctx, out);
        }
    }
}

/// Says that the stack is making its pages, for as long as it lives.
struct Making(Rc<Cell<bool>>);

impl Making {
    fn start(flag: &Rc<Cell<bool>>) -> Self {
        flag.set(true);
        Self(Rc::clone(flag))
    }
}

impl Drop for Making {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// The stack's own body. It reads the path, so a push or a pop runs this
/// body and nothing of the app's. It is a keyed list to the reconciler:
/// under it a page that is clean is kept, however often the path changes.
struct Pages<R: 'static, Root, V> {
    path: Binding<Vec<R>>,
    root: Root,
    destination: Rc<dyn Fn(R) -> V>,
}

impl<R: Clone + 'static, Root: Clone, V> Clone for Pages<R, Root, V> {
    fn clone(&self) -> Self {
        Self { path: self.path.clone(), root: self.root.clone(), destination: Rc::clone(&self.destination) }
    }
}

impl<R, Root, V> Component for Pages<R, Root, V>
where
    R: Clone + PartialEq + 'static,
    Root: View + Clone + 'static,
    V: View + 'static,
{
    const KEYED_LIST: bool = true;

    fn body(self) -> impl View {
        let shelf = State::new(Shelf {
            kept: Rc::new(RefCell::new(Kept { pages: Vec::new(), made: 0 })),
            making: Rc::new(Cell::new(false)),
        })
        .get();
        let routes = self.path.wrappedValue();
        let numbers = shelf.kept.borrow_mut().numbers(&routes);
        Laid { path: self.path, root: self.root, destination: self.destination, routes, numbers, making: shelf.making }
    }
}

/// The pages of one run: every page is made, and the one on top is laid.
struct Laid<R: 'static, Root, V> {
    path: Binding<Vec<R>>,
    root: Root,
    destination: Rc<dyn Fn(R) -> V>,
    routes: Vec<R>,
    numbers: Vec<u64>,
    making: Rc<Cell<bool>>,
}

impl<R: Clone + 'static, Root: Clone, V> Clone for Laid<R, Root, V> {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            root: self.root.clone(),
            destination: Rc::clone(&self.destination),
            routes: self.routes.clone(),
            numbers: self.numbers.clone(),
            making: Rc::clone(&self.making),
        }
    }
}

impl<R, Root, V> View for Laid<R, Root, V>
where
    R: Clone + PartialEq + 'static,
    Root: View + Clone + 'static,
    V: View + 'static,
{
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let depth = self.routes.len();
        let stack = navigator(&self.path);
        let _making = Making::start(&self.making);

        let mut prints = Vec::with_capacity(depth + 2);
        let mut top = Vec::new();
        let mut bar = Said::default();
        for place in 0..=depth {
            let covered = place < depth;
            let key: Cow<'static, str> =
                if place == 0 { Cow::Borrowed("root") } else { Cow::Owned(format!("p{}", self.numbers[place - 1])) };
            let mut page = NodeList::new();
            {
                // a page is its own scope: its state lives under its key,
                // and leaves with it when the route leaves the path
                let _frame = motor::identity::enter_key(&key);
                // what the page told the stack, for the bar. It is kept
                // in the page's scope: a page whose body is kept from an
                // earlier run says nothing in this one, and what it said
                // is still here
                let told = State::new(Told::default()).get();
                // moved when the page speaks while the stack is not making
                // its pages: the stack reads it, and so runs after the page
                let spoke = State::new(0u64);
                let hear = {
                    let (told, making) = (told.clone(), Rc::clone(&self.making));
                    move |word: &dyn Any| {
                        let heard = word.downcast_ref::<PageWord>().is_some_and(|word| told.hear(word));
                        if heard && !making.get() {
                            spoke.update(|times| *times += 1);
                        }
                    }
                };
                // the pages read their stack from the environment; the
                // app's other values pass through as they are
                let mut inside = ctx.clone();
                Rc::make_mut(&mut inside.values).navigation = Some(stack.clone().for_page(hear));
                if let Some(scope) = motor::identity::cursor_scope() {
                    if covered {
                        crate::reconciler::attribute_context(scope.clone(), crate::reconciler::NAV_COVERED, false);
                    }
                    if place > 0 {
                        // the link that opened the page gets the keyboard
                        // back when the page leaves holding it
                        crate::reconciler::attribute_context(scope, crate::reconciler::FOCUS_RETURN, false);
                    }
                }
                if place == 0 {
                    self.root.render_into(&inside, &mut page);
                } else {
                    (self.destination)(self.routes[place - 1].clone()).render_into(&inside, &mut page);
                }
                if !covered {
                    // read AFTER the page was made: a page that ran in
                    // this pass has just spoken. The count is read so
                    // that a page that speaks later runs the stack
                    spoke.get();
                    bar = told.0.borrow().clone();
                }
            }
            // EVERY page is made, so a covered one stays mounted with its
            // state. Only the page on top is laid: what is not laid takes
            // no press, no key and no place among the elements
            let (print, layout) = page.into_parts();
            prints.push(RenderNode::branch(print_label(if covered { "Page (covered)" } else { "Page" }), print));
            if !covered {
                top = layout;
            }
        }
        if depth > 0
            && let Some(scope) = motor::identity::cursor_scope()
        {
            // only a stack with a page to leave answers "back": the one
            // around it answers when this one is at its root
            let path = self.path.clone();
            // the keys of "back" answer while this stands, and only then
            crate::reconciler::attribute_context(scope.clone(), crate::action::NAV_CONTEXT, false);
            crate::reconciler::attribute_handler(scope, NAV_BACK, Rc::new(move || {
                path.pop();
            }));
        }
        let page = wrap_layout(top);
        // a root that says nothing has no bar: a stack that only holds
        // one untitled view adds nothing to it
        let laid = if depth > 0 || bar.title.is_some() || !bar.items.is_empty() {
            let mut whole = NodeList::new();
            // the bar is the stack's own: under its own key, beside the pages
            let _frame = motor::identity::enter_key("bar");
            self.under_a_bar(ctx, bar, page).render_into(ctx, &mut whole);
            let (print, layout) = whole.into_parts();
            prints.push(RenderNode::branch(print_label("Bar"), print));
            wrap_layout(layout)
        } else {
            page
        };
        out.push(RenderNode::branch(crate::view::print_line(|| format!("NavigationStack (path: {depth})")), prints));
        out.push_layout(laid);
    }
}

/// The bar, from edge to edge.
const BAR_HEIGHT: f64 = 44.0;
/// Between the bar's edge and what it holds.
const BAR_INSET: f64 = 8.0;

impl<R, Root, V> Laid<R, Root, V>
where
    R: Clone + PartialEq + 'static,
    Root: View + Clone + 'static,
    V: View + 'static,
{
    /// The page on top with the stack's bar over it: the button that goes
    /// back, where there is a page to leave, the page's leading items,
    /// its title, and its other items at the trailing edge.
    fn under_a_bar(&self, ctx: &Context, said: Said, page: LayoutNode) -> impl View<Arity = Single> + use<R, Root, V> {
        let theme = crate::theme::current();
        let back = (!self.routes.is_empty()).then(|| {
            let words = Words::for_locale(&ctx.environment::<Locale>());
            // the arrow points where the page under this one is
            let glyph = if ctx.environment::<LayoutDirection>().is_rtl() { symbol::CHEVRON_RIGHT } else { symbol::CHEVRON_LEFT };
            let path = self.path.clone();
            button(icon(glyph).frame(28.0, 28.0), move || {
                path.pop();
            })
            .plain()
            .accessibility_label(&*words.get(Word::Back))
        });
        let line = empty()
            .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center)
            .frame_height(1.0)
            .background_color(theme.divider);
        let title = text(said.title.as_deref().unwrap_or("")).bold().truncation_mode(Truncation::End);
        let bar = hstack((back, Items(said.items.leading, "l"), title, spacer(), Items(said.items.trailing, "t")))
            .spacing(4.0)
            .padding_edge(Edge::Leading, BAR_INSET)
            .padding_edge(Edge::Trailing, BAR_INSET)
            .frame_height(BAR_HEIGHT);
        // the page takes all the room under the bar, and starts at its top
        let page = Made(Rc::new(RefCell::new(Some(page)))).frame_max(f64::INFINITY, f64::INFINITY, Alignment::TopLeading);
        crate::provenance::Quiet(vstack((bar, line, page)).spacing(0.0))
    }
}

/// Layout that is made already, put where a view is expected. The stack
/// makes a page first, to hear its title, and lays it under the bar after.
#[derive(Clone)]
struct Made(Rc<RefCell<Option<LayoutNode>>>);

impl View for Made {
    type Arity = Single;

    fn render_into(&self, _ctx: &Context, out: &mut NodeList) {
        // its lines are printed with the stack's pages
        out.push(RenderNode::leaf(print_label("Page")));
        if let Some(node) = self.0.borrow_mut().take() {
            out.push_layout(node);
        }
    }
}

/// The stack's three doors, for the views inside its pages. They hold the
/// path and no page, so a page that was kept from an earlier run pushes
/// on the path as it is now.
fn navigator<R: Clone + 'static>(path: &Binding<Vec<R>>) -> Navigator {
    let (push, pop, depth) = (path.clone(), path.clone(), path.clone());
    Navigator::new(
        move |route: Box<dyn Any>| match route.downcast::<R>() {
            Ok(route) => {
                push.push(*route);
                true
            }
            // a route of another stack's type: this stack has no page for it
            Err(_) => false,
        },
        move || pop.pop(),
        move || depth.with(Vec::len),
    )
}

/// A stack of pages: `navigation_stack(self.path, root).destination(|route| …)`.
///
/// [The module](self) has the path, the links, what a covered page keeps
/// and how "back" works.
#[track_caller]
pub fn navigation_stack<R, Root>(path: impl IntoRoutes<R>, root: Root) -> NavigationStack<R, Root, EmptyView>
where
    R: Clone + PartialEq + 'static,
    Root: View + Clone + 'static,
{
    NavigationStack {
        path: path.into_routes(),
        root,
        // a route with no destination has an empty page: the stack is
        // not complete until `.destination(…)` names one
        destination: Rc::new(|_| EmptyView),
        site: crate::provenance::here(),
    }
}

/// The route of a stack that takes none: no value has this type.
#[derive(Clone, PartialEq)]
enum NoRoute {}

/// A stack with no path of the app's ([`navigation_stack_content`]).
#[derive(Clone)]
pub struct NavigationStackContent<Root> {
    root: Root,
    site: Site,
}

impl<Root: View> Component for NavigationStackContent<Root> {
    fn body(self) -> impl View {
        // the stack keeps the path the app did not give it
        let path = State::new(Vec::<NoRoute>::new());
        NavigationStack {
            path: path.binding(),
            root: self.root,
            destination: Rc::new(|route: NoRoute| -> EmptyView { match route {} }),
            site: self.site,
        }
    }
}

/// A stack that shows `root` and has no path of the app's:
/// `navigation_stack_content(page)`.
///
/// Use it for a place that is one page with a title and a bar, such as
/// the content of a sheet. A place whose pages the app opens by a route
/// takes [`navigation_stack`].
#[track_caller]
pub fn navigation_stack_content<Root: View>(root: Root) -> NavigationStackContent<Root> {
    NavigationStackContent { root, site: crate::provenance::here() }
}

/// A view that pushes a route when it is pressed ([`nav_link_value`]).
#[derive(Clone)]
pub struct NavLink<L, R> {
    label: L,
    route: R,
    site: Site,
}

impl<L: View + Clone + 'static, R: Clone + 'static> View for NavLink<L, R> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        let stack = ctx.environment::<Option<Navigator>>();
        let outside = stack.is_none();
        let route = self.route.clone();
        crate::provenance::quiet(|| {
            control(
                self.label.clone(),
                move || {
                    if let Some(stack) = &stack {
                        stack.push(route.clone());
                    }
                },
                ControlSpec {
                    kind: ControlKind::Button,
                    checked: None,
                    // a link outside every stack has nowhere to go
                    disabled: outside,
                    plain: true,
                    site: self.site,
                    shows: None,
                },
            )
            .render_into(ctx, out);
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::NavLink, None);
    }
}

/// A view that pushes `route` on the stack it stands in when it is
/// pressed: `nav_link_value(CountryCell::new(country), Route::Country(country))`.
///
/// The label is any view, and it draws everything: the link adds no
/// padding and no colour. Space and Enter press it, and it is a button
/// to assistive technology. Outside every stack it is disabled. On a
/// stack whose routes are of another type it does nothing.
#[track_caller]
pub fn nav_link_value<L: View + Clone + 'static, R: Clone + 'static>(label: L, route: R) -> NavLink<L, R> {
    NavLink { label, route, site: crate::provenance::here() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_its_number_while_the_path_still_starts_with_its_route() {
        let mut kept = Kept { pages: Vec::new(), made: 0 };
        assert_eq!(kept.numbers(&["a", "b"]), [1, 2]);
        assert_eq!(kept.numbers(&["a", "b", "c"]), [1, 2, 3], "a push keeps the pages under it");
        assert_eq!(kept.numbers(&["a", "b"]), [1, 2], "a pop keeps them too");
        assert_eq!(kept.numbers(&["a", "c"]), [1, 4], "another route at the same place is another page");
        assert_eq!(kept.numbers(&["a", "c", "b"]), [1, 4, 5], "and the route that left comes back as a new page");
        assert_eq!(kept.numbers(&[]), [0u64; 0]);
        assert_eq!(kept.numbers(&["a"]), [6], "a number is never used twice");
    }
}
