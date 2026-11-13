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
//! A page the app never opens by itself needs no route. A link makes it:
//!
//! ```ignore
//! nav_link("About", || About)
//! ```
//!
//! Such a page is not in the path. It stands over the pages the stack
//! shows when it opens; a route pushed later goes over it, and "back"
//! returns to it. The button of the bar, the platform's "back" and its
//! keys leave it. The app's `path.pop()` does not see it, and a write of
//! the path that changes a route under it takes it away. A place the app
//! must open by itself, restore or link to from outside takes a route.
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
//! ```ignore
//! page.searchable(self.query.binding())
//!     .refreshable(async move || model.reload().await)
//! ```
//!
//! `.searchable(…)` puts a search field in the bar, under the title. The
//! binding holds the query and the page filters by it; the accelerator
//! with `F` takes the keyboard to the field.
//!
//! `.refreshable(…)` gives the page work that loads its content again.
//! The bar gets a button for it, named "Refresh" in the reader's
//! language, and the accelerator with `R` starts it too. One run at a
//! time: the button is disabled until the work is done. The work is the
//! page's, as its tasks are: it continues under another page, and it is
//! cancelled when the page leaves the stack.
//!
//! A root that says none of these has no bar.
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
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use motor::state::{Binding, Context, LayoutDirection, Locale, Navigator, State};
use motor::task::Spawned;
use motor::view::RenderNode;
use motor::views::Edge;

use crate::action::NAV_BACK;
use crate::beat::Beat;
use crate::erased::Erased;
use crate::ext::ViewExt;
use crate::icon::house as symbol;
use crate::layout::{ControlKind, LayoutNode, Truncation};
use crate::provenance::{NodeKind, Site};
use crate::search_field::search_field;
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
    /// The number of the page of each route, and how many routes the path
    /// still starts with. Those keep their pages; from the first route
    /// that changed, every page is a new one.
    fn numbers(&mut self, routes: &[R]) -> (Vec<u64>, usize) {
        let same = self.pages.iter().zip(routes).take_while(|((was, _), now)| was == *now).count();
        self.pages.truncate(same);
        for route in &routes[same..] {
            self.made += 1;
            self.pages.push((route.clone(), self.made));
        }
        (self.pages.iter().map(|(_, number)| *number).collect(), same)
    }

    /// A number for a page that has no route.
    fn number(&mut self) -> u64 {
        self.made += 1;
        self.made
    }
}

/// A page with no route: the view a link made ([`nav_link`]).
///
/// It is not in the path. It stands over the routes that were in the path
/// when it was opened, and it stays for as long as the path still starts
/// with them: a route pushed later goes over it, and a write that changes
/// a route under it takes it away.
#[derive(Clone)]
struct Loose {
    number: u64,
    /// How many routes were under it.
    over: usize,
    make: Rc<dyn Fn() -> Erased>,
}

/// What a link to a view hands its stack, through `Navigator::push`.
struct Opens(Rc<dyn Fn() -> Erased>);

/// A page over the root, in the order the pages stand.
#[derive(Clone)]
enum Entry<R> {
    Route(R, u64),
    Loose(Loose),
}

impl<R> Entry<R> {
    fn number(&self) -> u64 {
        match self {
            Entry::Route(_, number) => *number,
            Entry::Loose(loose) => loose.number,
        }
    }
}

/// The pages over the root: each route's page, and after it the pages
/// with no route that were opened over it.
fn entries<R: Clone>(routes: &[R], numbers: &[u64], loose: &[Loose]) -> Vec<Entry<R>> {
    let mut all = Vec::with_capacity(routes.len() + loose.len());
    all.extend(loose.iter().filter(|page| page.over == 0).cloned().map(Entry::Loose));
    for (place, (route, number)) in routes.iter().zip(numbers).enumerate() {
        all.push(Entry::Route(route.clone(), *number));
        all.extend(loose.iter().filter(|page| page.over == place + 1).cloned().map(Entry::Loose));
    }
    all
}

/// The doors of a stack that its bar, its keys and its pages all use.
struct Doors<R: 'static> {
    path: Binding<Vec<R>>,
    kept: Rc<RefCell<Kept<R>>>,
    loose: Rc<RefCell<Vec<Loose>>>,
    /// Moved when a page with no route comes or goes: the stack reads it.
    moved: State<u64>,
}

impl<R: 'static> Clone for Doors<R>
where
    Binding<Vec<R>>: Clone,
{
    fn clone(&self) -> Self {
        Self { path: self.path.clone(), kept: Rc::clone(&self.kept), loose: Rc::clone(&self.loose), moved: self.moved }
    }
}

impl<R: Clone + PartialEq + 'static> Doors<R> {
    /// The page on top leaves: a page with no route that stands over the
    /// whole path, else the page of the last route. `false` on the root.
    fn leave(&self) -> bool {
        let routes = self.path.with(Vec::len);
        let top = self.loose.borrow().iter().rposition(|page| page.over == routes);
        match top {
            Some(place) => {
                self.loose.borrow_mut().remove(place);
                self.moved.update(|times| *times += 1);
                true
            }
            None => self.path.pop(),
        }
    }

    /// A view opens as a page over everything the stack holds now.
    fn open(&self, make: Rc<dyn Fn() -> Erased>) {
        let number = self.kept.borrow_mut().number();
        let over = self.path.with(Vec::len);
        self.loose.borrow_mut().push(Loose { number, over, make });
        self.moved.update(|times| *times += 1);
    }

    fn depth(&self) -> usize {
        self.path.with(Vec::len) + self.loose.borrow().len()
    }
}

/// The stack's memory, shared by the runs of its body. Reading it never
/// makes the body run: it is written inside the body, and only there.
struct Shelf<R> {
    kept: Rc<RefCell<Kept<R>>>,
    /// The pages with no route, in the order they were opened.
    loose: Rc<RefCell<Vec<Loose>>>,
    /// True while the stack makes its pages. A page that speaks then is
    /// read by the stack in the same run; one that speaks at another time
    /// has to make the stack run.
    making: Rc<Cell<bool>>,
}

impl<R> Clone for Shelf<R> {
    fn clone(&self) -> Self {
        Self { kept: Rc::clone(&self.kept), loose: Rc::clone(&self.loose), making: Rc::clone(&self.making) }
    }
}

/// What a view of a page tells the stack the page stands in. It travels
/// through [`Navigator::tell`], which knows no type of this crate.
pub(crate) enum PageWord {
    /// `.navigation_title(…)`.
    Title(Arc<str>),
    /// `.toolbar(…)`.
    Items(BarItems),
    /// `.searchable(…)`.
    Search(Binding<String>),
    /// `.refreshable(…)`.
    Refresh(Refresh),
}

/// One run of the work a page gave to `.refreshable(…)`.
type Run = Pin<Box<dyn Future<Output = ()>>>;

/// The work of `.refreshable(…)`: each call starts one run.
pub(crate) type Refresh = Rc<dyn Fn() -> Run>;

/// Keeps `action` so that it can run more than one time. Each run holds
/// the action while it borrows from it.
pub(crate) fn refresh<F: AsyncFn() + 'static>(action: F) -> Refresh {
    let action = Rc::new(action);
    Rc::new(move || {
        let action = Rc::clone(&action);
        Box::pin(async move { action().await })
    })
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
    search: Option<Binding<String>>,
    refresh: Option<Refresh>,
}

impl Said {
    /// A page that said nothing has no bar over it.
    fn is_nothing(&self) -> bool {
        self.title.is_none() && self.items.is_empty() && self.search.is_none() && self.refresh.is_none()
    }
}

/// What the stack keeps of one page, in the page's own scope: it is
/// there while the page is, covered or not, and it leaves with the page.
#[derive(Clone, Default)]
struct Told {
    said: Rc<RefCell<Said>>,
    /// `true` while a run of the page's refresh is not done.
    busy: Rc<Cell<bool>>,
    /// That run. To drop it is to cancel it, so a page that leaves the
    /// stack stops the work it started.
    running: Rc<RefCell<Option<Spawned>>>,
}

impl Told {
    /// Keeps the word. `true` when what the bar shows is different now.
    fn hear(&self, word: &PageWord) -> bool {
        let mut said = self.said.borrow_mut();
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
            // the binding a page gives is the same door on every run:
            // only the first one changes what the bar shows
            PageWord::Search(query) => said.search.replace(query.clone()).is_none(),
            // the last work is kept, because it holds what the page holds
            // now. The bar shows one button for any of them
            PageWord::Refresh(work) => said.refresh.replace(Rc::clone(work)).is_none(),
        }
    }
}

/// The refresh of the page on top, for the button of the bar and the key.
#[derive(Clone)]
struct Reload {
    told: Told,
    /// The page's count of words: a move of it runs the stack again.
    spoke: State<u64>,
}

impl Reload {
    /// Starts a run of the page's work. One run at a time: while one is
    /// not done, this does nothing.
    fn start(&self) {
        // read when the reader asks, not when the bar was made: a page
        // gives its work again each time its body runs
        let Some(work) = self.told.said.borrow().refresh.clone() else { return };
        if self.told.busy.replace(true) {
            return;
        }
        let run = work();
        let (busy, spoke) = (Rc::clone(&self.told.busy), self.spoke);
        let task = motor::task::spawn(async move {
            run.await;
            busy.set(false);
            spoke.update(|times| *times += 1);
        });
        // out of the cell before it drops: a cancel runs the future's
        // own drop, which must not find the cell borrowed
        let before = self.told.running.borrow_mut().replace(task);
        drop(before);
        self.spoke.update(|times| *times += 1);
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
            loose: Rc::new(RefCell::new(Vec::new())),
            making: Rc::new(Cell::new(false)),
        })
        .get();
        // read, so that a page with no route that comes or goes runs this body
        let moved = State::new(0u64);
        moved.get();
        let routes = self.path.wrappedValue();
        let (numbers, same) = shelf.kept.borrow_mut().numbers(&routes);
        // a page with no route stays while the routes under it are the same
        shelf.loose.borrow_mut().retain(|page| page.over <= same);
        let pages = entries(&routes, &numbers, &shelf.loose.borrow());
        let doors = Doors { path: self.path, kept: shelf.kept, loose: shelf.loose, moved };
        Laid { doors, root: self.root, destination: self.destination, pages, making: shelf.making }
    }
}

/// The pages of one run: every page is made, and the one on top is laid.
struct Laid<R: 'static, Root, V> {
    doors: Doors<R>,
    root: Root,
    destination: Rc<dyn Fn(R) -> V>,
    /// The pages over the root, the one on top last.
    pages: Vec<Entry<R>>,
    making: Rc<Cell<bool>>,
}

impl<R: Clone + 'static, Root: Clone, V> Clone for Laid<R, Root, V> {
    fn clone(&self) -> Self {
        Self {
            doors: self.doors.clone(),
            root: self.root.clone(),
            destination: Rc::clone(&self.destination),
            pages: self.pages.clone(),
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
        let depth = self.pages.len();
        let stack = navigator(&self.doors);
        let _making = Making::start(&self.making);

        let mut prints = Vec::with_capacity(depth + 2);
        let mut top = Vec::new();
        let mut bar = Said::default();
        let mut reload = None;
        for place in 0..=depth {
            let covered = place < depth;
            let key: Cow<'static, str> =
                if place == 0 { Cow::Borrowed("root") } else { Cow::Owned(format!("p{}", self.pages[place - 1].number())) };
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
                // its pages, and when its refresh starts or ends: the
                // stack reads it, and so runs after the page
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
                match place.checked_sub(1).map(|over_root| &self.pages[over_root]) {
                    None => self.root.render_into(&inside, &mut page),
                    Some(Entry::Route(route, _)) => (self.destination)(route.clone()).render_into(&inside, &mut page),
                    Some(Entry::Loose(loose)) => (loose.make)().render_into(&inside, &mut page),
                }
                if !covered {
                    // read AFTER the page was made: a page that ran in
                    // this pass has just spoken. The count is read so
                    // that a page that speaks later runs the stack
                    spoke.get();
                    bar = told.said.borrow().clone();
                    reload = Some(Reload { told, spoke });
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
            let doors = self.doors.clone();
            // the keys of "back" answer while this stands, and only then
            crate::reconciler::attribute_context(scope.clone(), crate::action::NAV_CONTEXT, false);
            crate::reconciler::attribute_handler(scope, NAV_BACK, Rc::new(move || {
                doors.leave();
            }));
        }
        let page = wrap_layout(top);
        // a root that says nothing has no bar: a stack that only holds
        // one untitled view adds nothing to it
        let laid = if depth > 0 || !bar.is_nothing() {
            let mut whole = NodeList::new();
            // the bar is the stack's own: under its own key, beside the pages
            let _frame = motor::identity::enter_key("bar");
            // the work is the page's, so only a page that gave some has
            // a button for it
            let reload = reload.filter(|_| bar.refresh.is_some());
            self.under_a_bar(ctx, bar, reload, page).render_into(ctx, &mut whole);
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
    /// its title, and at the trailing edge the button that refreshes the
    /// page and the page's other items.
    fn under_a_bar(
        &self,
        ctx: &Context,
        said: Said,
        reload: Option<Reload>,
        page: LayoutNode,
    ) -> impl View<Arity = Single> + use<R, Root, V> {
        let theme = crate::theme::current();
        let words = Words::for_locale(&ctx.environment::<Locale>());
        let back = (!self.pages.is_empty()).then(|| {
            // the arrow points where the page under this one is
            let glyph = if ctx.environment::<LayoutDirection>().is_rtl() { symbol::CHEVRON_RIGHT } else { symbol::CHEVRON_LEFT };
            let doors = self.doors.clone();
            button(icon(glyph).frame(28.0, 28.0), move || {
                doors.leave();
            })
            .plain()
            .accessibility_label(&*words.get(Word::Back))
        });
        let line = empty()
            .frame_max(f64::INFINITY, f64::INFINITY, Alignment::Center)
            .frame_height(1.0)
            .background_color(theme.divider);
        let title = text(said.title.as_deref().unwrap_or("")).bold().truncation_mode(Truncation::End);
        let again = reload.map(|reload| {
            if let Some(scope) = motor::identity::cursor_scope() {
                // the key is the stack's while the page on top can load
                // again, a run on its way or not
                let by_key = reload.clone();
                crate::reconciler::attribute_context(scope.clone(), crate::action::NAV_REFRESH_CONTEXT, false);
                crate::reconciler::attribute_handler(scope, crate::action::NAV_REFRESH, Rc::new(move || by_key.start()));
            }
            // one run at a time: the button waits for the one on its way
            let busy = reload.told.busy.get();
            button(icon(symbol::REFRESH).frame(28.0, 28.0), move || reload.start())
                .plain()
                .disabled(busy)
                .accessibility_label(&*words.get(Word::Refresh))
        });
        let bar = hstack((back, Items(said.items.leading, "l"), title, spacer(), again, Items(said.items.trailing, "t")))
            .spacing(4.0)
            .padding_edge(Edge::Leading, BAR_INSET)
            .padding_edge(Edge::Trailing, BAR_INSET)
            .frame_height(BAR_HEIGHT);
        let find = said.search.map(|query| {
            // the bar's own beat: "find" rings it and the field answers
            let beat = Beat::new();
            if let Some(scope) = motor::identity::cursor_scope() {
                crate::reconciler::attribute_context(scope.clone(), crate::action::NAV_FIND_CONTEXT, false);
                crate::reconciler::attribute_handler(scope, crate::action::NAV_FIND, Rc::new(move || {
                    beat.ring();
                }));
            }
            search_field(query)
                .select_on(beat)
                .padding_edge(Edge::Leading, BAR_INSET)
                .padding_edge(Edge::Trailing, BAR_INSET)
                .padding_edge(Edge::Bottom, BAR_INSET)
        });
        // the page takes all the room under the bar, and starts at its top
        let page = Made(Rc::new(RefCell::new(Some(page)))).frame_max(f64::INFINITY, f64::INFINITY, Alignment::TopLeading);
        crate::provenance::Quiet(vstack((bar, find, line, page)).spacing(0.0))
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
fn navigator<R: Clone + PartialEq + 'static>(doors: &Doors<R>) -> Navigator {
    let (push, pop, depth) = (doors.clone(), doors.clone(), doors.clone());
    Navigator::new(
        move |word: Box<dyn Any>| match word.downcast::<R>() {
            Ok(route) => {
                push.path.push(*route);
                true
            }
            Err(word) => match word.downcast::<Opens>() {
                Ok(opens) => {
                    push.open(opens.0);
                    true
                }
                // a route of another stack's type: this stack has no page for it
                Err(_) => false,
            },
        },
        move || pop.leave(),
        move || depth.depth(),
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

/// What a link shows: words, or any view.
///
/// `nav_link("About", …)` and `nav_link(CountryCell::new(country), …)`
/// are both a label.
pub trait IntoLabel {
    /// The view the link shows.
    type Label: View;
    /// The label as a view.
    fn into_label(self) -> Self::Label;
}

impl IntoLabel for &str {
    type Label = crate::views::Text;

    fn into_label(self) -> Self::Label {
        text(self)
    }
}

impl IntoLabel for String {
    type Label = crate::views::Text;

    fn into_label(self) -> Self::Label {
        text(self)
    }
}

impl<V: View> IntoLabel for V {
    type Label = V;

    fn into_label(self) -> V {
        self
    }
}

/// A view that opens a page when it is pressed ([`nav_link`],
/// [`nav_link_value`]).
#[derive(Clone)]
pub struct NavLink<L> {
    label: L,
    /// What a press tells the stack the link stands in.
    open: Rc<dyn Fn(&Navigator)>,
    kind: NodeKind,
    site: Site,
}

impl<L: View> View for NavLink<L> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        let stack = ctx.environment::<Option<Navigator>>();
        let outside = stack.is_none();
        let open = Rc::clone(&self.open);
        crate::provenance::quiet(|| {
            control(
                self.label.clone(),
                move || {
                    if let Some(stack) = &stack {
                        open(stack);
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
        crate::provenance::seal(ctx, out, mark, self.site, self.kind, None);
    }
}

/// A view that pushes `route` on the stack it stands in when it is
/// pressed: `nav_link_value(CountryCell::new(country), Route::Country(country))`.
///
/// The label is words or any view, and a view draws everything: the link
/// adds no padding and no colour. Space and Enter press it, and it is a
/// button to assistive technology. Outside every stack it is disabled.
/// On a stack whose routes are of another type it does nothing.
#[track_caller]
pub fn nav_link_value<L: IntoLabel, R: Clone + 'static>(label: L, route: R) -> NavLink<L::Label> {
    NavLink {
        label: label.into_label(),
        open: Rc::new(move |stack| {
            stack.push(route.clone());
        }),
        kind: NodeKind::NavLinkValue,
        site: crate::provenance::here(),
    }
}

/// A view that opens the page `destination` makes when it is pressed:
/// `nav_link("About", || About)`.
///
/// The page has no route and is not in the path. It stands over the
/// pages the stack shows when it opens. The button of the bar, the
/// platform's "back" and the keys of "back" leave it; the app's own
/// `path.pop()` does not see it. A route that is pushed later goes over
/// it, and a write of the path that changes a route under it takes it
/// away. A place the app must be able to open by itself, or restore,
/// takes a route and [`nav_link_value`].
#[track_caller]
pub fn nav_link<L, V, F>(label: L, destination: F) -> NavLink<L::Label>
where
    L: IntoLabel,
    V: View<Arity = Single>,
    F: Fn() -> V + 'static,
{
    let make: Rc<dyn Fn() -> Erased> = Rc::new(move || Erased::new(destination()));
    NavLink {
        label: label.into_label(),
        open: Rc::new(move |stack| {
            stack.push(Opens(Rc::clone(&make)));
        }),
        kind: NodeKind::NavLink,
        site: crate::provenance::here(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_keeps_its_number_while_the_path_still_starts_with_its_route() {
        let mut kept = Kept { pages: Vec::new(), made: 0 };
        assert_eq!(kept.numbers(&["a", "b"]), (vec![1, 2], 0));
        assert_eq!(kept.numbers(&["a", "b", "c"]), (vec![1, 2, 3], 2), "a push keeps the pages under it");
        assert_eq!(kept.numbers(&["a", "b"]), (vec![1, 2], 2), "a pop keeps them too");
        assert_eq!(kept.numbers(&["a", "c"]), (vec![1, 4], 1), "another route at the same place is another page");
        assert_eq!(kept.numbers(&["a", "c", "b"]), (vec![1, 4, 5], 2), "and the route that left comes back as a new page");
        assert_eq!(kept.numbers(&[]), (vec![], 0));
        assert_eq!(kept.number(), 6, "a page with no route takes a number of the same count");
        assert_eq!(kept.numbers(&["a"]), (vec![7], 0), "a number is never used twice");
    }

    #[test]
    fn a_page_with_no_route_stands_after_the_routes_it_was_opened_over() {
        let page = |number: u64, over: usize| Loose { number, over, make: Rc::new(|| Erased::new(EmptyView)) };
        let order = |routes: &[&str], numbers: &[u64], loose: &[Loose]| -> Vec<u64> {
            entries(routes, numbers, loose).iter().map(Entry::number).collect()
        };
        assert_eq!(order(&[], &[], &[page(9, 0)]), [9], "over the root");
        assert_eq!(order(&["a", "b"], &[1, 2], &[page(9, 2)]), [1, 2, 9], "over the whole path");
        // a route pushed later goes over it
        assert_eq!(order(&["a", "b", "c"], &[1, 2, 3], &[page(9, 2)]), [1, 2, 9, 3]);
        assert_eq!(order(&["a"], &[1], &[page(8, 0), page(9, 1), page(10, 1)]), [8, 1, 9, 10]);
    }
}
