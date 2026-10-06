//! Property-wrapper machinery: `@State`, `@Binding`, `@Environment`,
//! plus the `Context` / `EnvironmentValues` they resolve against.

use crate::combine::Store;
use std::any::{Any, TypeId};
use std::cell::{Cell, RefCell};
use crate::hash::FxHashMap;
use std::marker::PhantomData;
use std::rc::Rc;

/// A side effect collected while rendering (drained by `Runtime::pump`).
/// Returns whether it observed a state change (i.e. a re-render is due).
pub type EffectFn = Rc<dyn Fn(&Context) -> bool>;

/// `\.locale` — the languages the reader asked for, best first, as one
/// normalized BCP-47 list: `"pt-BR,en-US,en"`.
///
/// A list, not a word. A person who reads Portuguese and English is
/// served Portuguese where there is some and English where there is
/// not, and an app that speaks neither falls to its own source
/// language — [`Locale::pick`] is that decision, made once per table.
/// The shell reports the system's list, the app may pin its own, and a
/// body reads whichever stands with `ctx.environment::<Locale>()`.
///
/// Shared, not copied: the tags sit behind an `Rc`, so the clone every
/// read hands out is a count. A thousand rows asking cost a thousand
/// increments and no bytes, and every accessor is a slice of the one
/// string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Locale {
    tags: Rc<str>,
}

impl Locale {
    /// One tag — `Locale::new("pt-BR")`. Spelled however the platform
    /// spells it: `pt_BR.UTF-8` and `PT-br` both land on `pt-BR`. An
    /// empty tag is the default.
    pub fn new(tag: impl AsRef<str>) -> Self {
        Self::parse(tag.as_ref())
    }

    /// Several tags, best first, each normalized and kept once.
    pub fn preferred<I, S>(tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut list = String::new();
        for tag in tags {
            push_tags(&mut list, tag.as_ref());
        }
        Self::finish(list)
    }

    /// The ONE normalizer, which every shell's report goes through, so
    /// a spelling has one home: tags separated by commas, semicolons,
    /// colons or spaces (a browser's `navigator.languages` joined, a
    /// POSIX `LANGUAGE`, an `Accept-Language` header with its `q=`
    /// weights, which are dropped); `_` for `-`; a POSIX encoding and
    /// modifier (`.UTF-8@latin`) cut away; `C` and `POSIX` ignored;
    /// the language lowercased with its legacy codes brought forward
    /// (`iw` → `he`, `ji` → `yi`, `in` → `id`), a script in Title case,
    /// a region in upper; a tag repeated kept once. Nothing left is the
    /// default.
    pub fn parse(list: &str) -> Self {
        let mut out = String::with_capacity(list.len());
        push_tags(&mut out, list);
        Self::finish(out)
    }

    fn finish(list: String) -> Self {
        if list.is_empty() { Self::default() } else { Locale { tags: Rc::from(list) } }
    }

    /// The first tag — the locale the reader prefers above all others.
    pub fn identifier(&self) -> &str {
        self.tags().next().unwrap_or("en")
    }

    /// The first tag's language: `"pt"` of `pt-BR`.
    pub fn language(&self) -> &str {
        Tag::of(self.identifier()).language
    }

    /// The first tag's script, when it names one: `"Hant"` of `zh-Hant-TW`.
    pub fn script(&self) -> Option<&str> {
        Tag::of(self.identifier()).script
    }

    /// The first tag's region, when it names one: `"BR"` of `pt-BR`,
    /// `"419"` of `es-419`.
    pub fn region(&self) -> Option<&str> {
        Tag::of(self.identifier()).region
    }

    /// Every tag, best first.
    pub fn tags(&self) -> impl Iterator<Item = &str> + '_ {
        self.tags.split(',')
    }

    /// The whole list as it is kept — what a wire or a log wants.
    pub fn as_str(&self) -> &str {
        &self.tags
    }

    /// Which of `supported` serves this reader best: the index of the
    /// tag chosen, or `None` when none speaks any of the languages
    /// asked for. See [`Locale::pick_in`].
    pub fn pick(&self, supported: &[&str]) -> Option<usize> {
        self.pick_in(supported.iter().copied())
    }

    /// [`Locale::pick`] over any list of tags, without building a slice.
    ///
    /// For each preferred tag, best first: the exact tag; then the tag
    /// cut from the right (`pt-Latn-BR`, `pt-Latn`, `pt`); then any tag
    /// of the same language whose script agrees — a region implies its
    /// script where that matters (`zh-TW` is `Hant`, `zh` and `zh-CN`
    /// are `Hans`). Only then the next preference: a reader of `pt-BR`
    /// is better served by `pt-PT` than by the English they listed
    /// second. Case never matters, legacy codes meet their modern
    /// names, and nothing allocates.
    pub fn pick_in<'s, I>(&self, supported: I) -> Option<usize>
    where
        I: Iterator<Item = &'s str> + Clone,
    {
        for preferred in self.tags() {
            let mut range = preferred;
            loop {
                if let Some(index) =
                    supported.clone().position(|tag| tag.eq_ignore_ascii_case(range))
                {
                    return Some(index);
                }
                match range.rfind('-') {
                    Some(cut) => range = &range[..cut],
                    None => break,
                }
            }
            let want = Tag::of(preferred);
            let want_script = want.script_or_implied();
            if let Some(index) = supported.clone().position(|tag| {
                let have = Tag::of(tag);
                same_language(have.language, want.language)
                    && match (want_script, have.script_or_implied()) {
                        (Some(wanted), Some(had)) => wanted.eq_ignore_ascii_case(had),
                        _ => true,
                    }
            }) {
                return Some(index);
            }
        }
        None
    }

    /// Which way the first tag reads. A script decides when the tag
    /// names one (`az-Arab` reads right to left, `ar-Latn` does not);
    /// the language decides otherwise.
    pub fn direction(&self) -> LayoutDirection {
        let tag = Tag::of(self.identifier());
        let rtl = match tag.script {
            Some(script) => RTL_SCRIPTS.iter().any(|known| known.eq_ignore_ascii_case(script)),
            None => RTL_LANGUAGES.iter().any(|known| known.eq_ignore_ascii_case(tag.language)),
        };
        if rtl { LayoutDirection::RightToLeft } else { LayoutDirection::LeftToRight }
    }
}

impl Default for Locale {
    /// English — the language the framework's own words are written in.
    fn default() -> Self {
        Locale { tags: Rc::from("en") }
    }
}

impl std::fmt::Display for Locale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.tags)
    }
}

/// The scripts written right to left.
const RTL_SCRIPTS: [&str; 9] =
    ["Arab", "Hebr", "Thaa", "Syrc", "Nkoo", "Adlm", "Rohg", "Mand", "Samr"];

/// The languages written right to left when their tag names no script.
const RTL_LANGUAGES: [&str; 17] = [
    "ar", "he", "fa", "ur", "ps", "sd", "ug", "yi", "dv", "ckb", "ks", "syr", "nqo", "arc", "prs",
    "iw", "ji",
];

/// Writes every tag of `list` that names a language into `out`,
/// normalized and separated by commas, skipping one already there.
fn push_tags(out: &mut String, list: &str) {
    let separators = |c: char| c == ',' || c == ';' || c == ':' || c.is_ascii_whitespace();
    for token in list.split(separators) {
        // a POSIX locale carries its encoding and modifier behind the tag
        let token = token.split(['.', '@']).next().unwrap_or("");
        let language = token.split(['-', '_']).next().unwrap_or("");
        let names_a_language = (2..=8).contains(&language.len())
            && language.bytes().all(|b| b.is_ascii_alphabetic())
            && !language.eq_ignore_ascii_case("POSIX");
        if !names_a_language || token.contains('=') {
            continue;
        }
        let start = out.len();
        if start > 0 {
            out.push(',');
        }
        let tag_start = out.len();
        push_canonical(out, token);
        let repeated = {
            let written = &out[tag_start..];
            out[..start].split(',').any(|earlier| earlier == written)
        };
        if repeated {
            out.truncate(start);
        }
    }
}

/// One tag in canonical spelling: subtags by their shape, not their
/// position — the language lowercased, a four-letter script in Title
/// case, a two-letter or three-digit region in upper, the rest lower.
fn push_canonical(out: &mut String, tag: &str) {
    for (index, sub) in tag.split(['-', '_']).filter(|sub| !sub.is_empty()).enumerate() {
        if index > 0 {
            out.push('-');
        }
        let is_alpha = sub.bytes().all(|b| b.is_ascii_alphabetic());
        if index == 0 {
            let at = out.len();
            out.extend(sub.chars().map(|c| c.to_ascii_lowercase()));
            if let Some(modern) = modern_name(&out[at..]) {
                out.truncate(at);
                out.push_str(modern);
            }
        } else if sub.len() == 4 && is_alpha {
            let mut chars = sub.chars();
            out.extend(chars.next().map(|c| c.to_ascii_uppercase()));
            out.extend(chars.map(|c| c.to_ascii_lowercase()));
        } else if (sub.len() == 2 && is_alpha)
            || (sub.len() == 3 && sub.bytes().all(|b| b.is_ascii_digit()))
        {
            out.extend(sub.chars().map(|c| c.to_ascii_uppercase()));
        } else {
            out.extend(sub.chars().map(|c| c.to_ascii_lowercase()));
        }
    }
}

/// The modern name of one of the three legacy codes the platforms still
/// hand out; `None` for every language already called by its name.
fn modern_name(language: &str) -> Option<&'static str> {
    if language.eq_ignore_ascii_case("iw") {
        Some("he")
    } else if language.eq_ignore_ascii_case("ji") {
        Some("yi")
    } else if language.eq_ignore_ascii_case("in") {
        Some("id")
    } else {
        None
    }
}

fn same_language(a: &str, b: &str) -> bool {
    modern_name(a).unwrap_or(a).eq_ignore_ascii_case(modern_name(b).unwrap_or(b))
}

/// One tag's subtags, read in place — the reading twin of
/// [`push_canonical`], which works on tags an app wrote by hand too.
#[derive(Clone, Copy)]
struct Tag<'a> {
    language: &'a str,
    script: Option<&'a str>,
    region: Option<&'a str>,
}

impl<'a> Tag<'a> {
    fn of(tag: &'a str) -> Self {
        let mut parts = tag.split(['-', '_']);
        let language = parts.next().unwrap_or("");
        let mut script = None;
        let mut region = None;
        for part in parts {
            let is_alpha = part.bytes().all(|b| b.is_ascii_alphabetic());
            if script.is_none() && region.is_none() && part.len() == 4 && is_alpha {
                script = Some(part);
            } else if region.is_none()
                && ((part.len() == 2 && is_alpha)
                    || (part.len() == 3 && part.bytes().all(|b| b.is_ascii_digit())))
            {
                region = Some(part);
            } else {
                // variants and extensions: nothing a match reads
                break;
            }
        }
        Tag { language, script, region }
    }

    /// The script named, or the one the region implies where a language
    /// is written two ways: Chinese is traditional in Taiwan, Hong Kong
    /// and Macau and simplified everywhere else, bare `zh` included.
    fn script_or_implied(&self) -> Option<&'a str> {
        if self.script.is_some() {
            return self.script;
        }
        if self.language.eq_ignore_ascii_case("zh") {
            let traditional = self.region.is_some_and(|region| {
                ["TW", "HK", "MO"].iter().any(|known| known.eq_ignore_ascii_case(region))
            });
            return Some(if traditional { "Hant" } else { "Hans" });
        }
        None
    }
}

/// `\.layoutDirection` — which way a row reads, and where leading is.
///
/// The runtime derives it from the locale in effect and an app may pin
/// it (`Runtime::set_layout_direction`); an island that reads the other
/// way turns it for its own subtree with
/// `.environment(|values| values.layoutDirection = …)`. The layout
/// engine mirrors every leading edge by it; pictures keep their face
/// unless a view says `flips_for_right_to_left_layout_direction(true)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum LayoutDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

impl LayoutDirection {
    pub const fn is_rtl(self) -> bool {
        matches!(self, LayoutDirection::RightToLeft)
    }
}

/// `\.horizontalSizeClass` — how wide the window is, in the two words
/// a layout adapts to: a phone in portrait is `Compact`, everything from
/// a tablet up is `Regular`. The shell reports it and a body that reads
/// it re-runs when it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SizeClass {
    Compact,
    #[default]
    Regular,
}

/// `\.safeAreaInsets` — what the WINDOW is covered by at each edge: the
/// notch and the status bar above, the home indicator below, a rounded
/// corner's bite at the sides.
///
/// The root is laid out inside these already; a body reads them when it has
/// to paint THROUGH them and hold its content clear by hand — an ambient
/// wash that stops dead at a horizontal line under the status bar reads as
/// exactly what it is, and a bottom sheet that stops above the home
/// indicator is a sheet with a gap under it.
///
/// Stored as the window sees them: `leading` is the LEFT band. Read
/// through the environment, leading follows the direction — a body in
/// a right-to-left scene gets the right band as its `leading`, which is
/// the side its content starts from.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct SafeAreaInsets {
    pub top: f64,
    pub trailing: f64,
    pub bottom: f64,
    pub leading: f64,
}

/// `\.keyboardInset` — the software keyboard's height over the window, `0`
/// when it hides.
///
/// Separate from [`SafeAreaInsets`] on purpose, because the two bands mean
/// opposite things to a surface that reaches the screen's edge: whatever the
/// keyboard covers is genuinely unusable and content must stand above it,
/// while the home indicator's band is a place a surface may paint and only
/// its CONTENT must keep clear of. A single merged number cannot say that,
/// and an app that only has the merger has to choose which of the two rules
/// to get wrong.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct KeyboardInset(pub f64);

/// `\.viewport` — the size the window lays its root out at, in layout
/// points: the whole window, the safe area included.
///
/// A body reads it to choose a SHAPE — a desktop chassis or a portrait
/// one, a sidebar that fits or a sheet that replaces it — which is the
/// question a product asks `Form::of(window)` in twenty places. It is a
/// dependency like a `State`: a body that reads it re-runs when the
/// window changes size, and only that body. A body that only needs a
/// threshold does best to read it in a small view of its own, so the
/// rest of the tree stays still while a resize crosses nothing.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
}

/// `\.windowState` — what the platform says about the window itself.
///
/// A scene that draws its own caption buttons draws the middle one as a
/// square or as the restore glyph, and only the platform knows which the
/// window is: the button's click was always right in both states, and
/// now its picture can be too. A dependency like [`Viewport`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct WindowState {
    /// The window fills the screen's work area, by the platform's own
    /// maximize (zoom on the mac) — not by being dragged that big.
    pub maximized: bool,
}

/// The values the runtime keeps for its window, readable from every body.
/// Shared handles, so a read is a dependency and a move re-runs only the
/// bodies that read — never the whole retention, which is what moving
/// any other environment value costs, and what a live resize could not
/// afford once a frame.
#[derive(Clone)]
pub struct WindowValues {
    viewport: Store<Viewport>,
    state: Store<WindowState>,
}

impl Default for WindowValues {
    fn default() -> Self {
        WindowValues { viewport: Store::new(Viewport::default()), state: Store::new(WindowState::default()) }
    }
}

impl WindowValues {
    /// The runtime's write: `true` when the viewport moved, and then the
    /// bodies that read it are due.
    pub fn set_viewport(&self, viewport: Viewport) -> bool {
        let moved = self.viewport.value() != viewport;
        if moved {
            self.viewport.send(viewport);
        }
        moved
    }

    /// The runtime's write for the platform's report: `true` when it moved.
    pub fn set_state(&self, state: WindowState) -> bool {
        let moved = self.state.value() != state;
        if moved {
            self.state.send(state);
        }
        moved
    }
}

/// Everything `@Environment(\.key)` can read. App-specific values (the DI
/// container, the SwiftData model container) ride along type-erased, exactly
/// like `@Entry` extensions do in real SwiftUI.
#[derive(Clone, Default)]
pub struct EnvironmentValues {
    /// `\.locale` — the shell's report or the app's pin, see [`Locale`].
    pub locale: Locale,
    /// `\.layoutDirection` — derived from the locale by the runtime
    /// unless pinned, see [`LayoutDirection`].
    pub layoutDirection: LayoutDirection,
    /// `\.horizontalSizeClass`.
    pub horizontalSizeClass: SizeClass,
    /// `\.safeAreaInsets` — the shell's, mirrored per layout.
    pub safeAreaInsets: SafeAreaInsets,
    /// `\.keyboardInset` — the shell's, mirrored per layout.
    pub keyboardInset: KeyboardInset,
    /// `\.viewport` and `\.windowState` — the runtime's, kept live.
    pub window: WindowValues,
    /// `\.injected` — `Rc<DIContainer>` in the app.
    pub injected: Option<Rc<dyn Any>>,
    /// `\.modelContext` stand-in: resolves `Query<T>` sources by type name.
    pub querySource: Option<Rc<dyn Fn(&'static str) -> Option<Rc<dyn Any>>>>,
}

/// Render-time context: environment values + collected effects.
///
/// The values are shared, and copied only when written: every body that
/// runs keeps a copy of the context it ran in, so a skipped view can
/// answer from cache — a thousand rows kept a thousand copies of the
/// same values, where a share is a count. A write goes through [`Rc::make_mut`], which copies
/// the values once when anything else still holds them.
#[derive(Clone, Default)]
pub struct Context {
    pub values: Rc<EnvironmentValues>,
    pub(crate) effects: Rc<RefCell<Vec<EffectFn>>>,
}

impl Context {
    pub fn environment<T: FromEnvironment>(&self) -> T {
        T::from_environment(&self.values)
    }
}

/// `@Environment(\.key) var x: T` — resolvable from `EnvironmentValues`.
pub trait FromEnvironment: Clone + 'static {
    fn from_environment(values: &EnvironmentValues) -> Self;
}

impl FromEnvironment for Locale {
    fn from_environment(values: &EnvironmentValues) -> Self {
        // a count, not a copy: the tags are shared
        values.locale.clone()
    }
}

impl FromEnvironment for LayoutDirection {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.layoutDirection
    }
}

impl FromEnvironment for SizeClass {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.horizontalSizeClass
    }
}

impl FromEnvironment for SafeAreaInsets {
    /// The window's bands in the BODY's direction: right to left, the
    /// right band is the leading one. The stored value stays the
    /// window's, so the shell writes it once however the scene reads.
    fn from_environment(values: &EnvironmentValues) -> Self {
        let insets = values.safeAreaInsets;
        if values.layoutDirection.is_rtl() {
            SafeAreaInsets { leading: insets.trailing, trailing: insets.leading, ..insets }
        } else {
            insets
        }
    }
}

impl FromEnvironment for KeyboardInset {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.keyboardInset
    }
}

impl FromEnvironment for Viewport {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.window.viewport.value()
    }
}

impl FromEnvironment for WindowState {
    fn from_environment(values: &EnvironmentValues) -> Self {
        values.window.state.value()
    }
}

/// `@Environment(\.key) private var x: T`
pub struct Environment<T: FromEnvironment> {
    _phantom: PhantomData<fn() -> T>,
}

impl<T: FromEnvironment> Environment<T> {
    pub fn new() -> Self {
        Environment { _phantom: PhantomData }
    }

    pub fn wrappedValue(&self, ctx: &Context) -> T {
        T::from_environment(&ctx.values)
    }
}

impl<T: FromEnvironment> Default for Environment<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: FromEnvironment> Clone for Environment<T> {
    fn clone(&self) -> Self {
        Self::new()
    }
}

/// `@State private var x = value` — value-type view, arena-backed storage.
///
/// The handle is `Copy` (index + generation + dependency id): closures
/// capture copies implicitly, like Swift structs — without the
/// `let this = self.clone()` ceremony per closure.
///
/// Ownership belongs to the runtime, anchored on structural identity
/// ([`crate::identity`]): `new` INSIDE a render pass is a declaration — if
/// the identity already mounted this state, the handle points to the live
/// slot and the initial value is discarded (the initial only seeds the
/// first mount, like Swift's `@State`). An identity that leaves the tree
/// takes the slot with it; the generation advances and a retained handle
/// fails loudly instead of reading a recycled slot. Outside render (roots
/// the app holds), the slot belongs to the app and lives forever.
///
/// Storage is a **per-type arena** (`Vec<Slot<T>>` with inline values):
/// the hot path indexes typed slots with no per-value downcast and no
/// per-slot box — erasure retreats to the cold edge (the arena registry and
/// the function pointer the sweep uses to free without knowing `T`).
pub struct State<T> {
    index: usize,
    generation: u32,
    /// Identity for the read graph — global, never recycled.
    dep: u64,
    _marker: PhantomData<fn() -> T>,
}

struct TypedSlot<T> {
    generation: u32,
    value: Option<T>,
}

struct TypedArena<T> {
    slots: Vec<TypedSlot<T>>,
    free: Vec<usize>,
}

impl<T> TypedArena<T> {
    fn new() -> Self {
        TypedArena { slots: Vec::new(), free: Vec::new() }
    }

    fn alloc(&mut self, value: T) -> (usize, u32) {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index];
            slot.value = Some(value);
            (index, slot.generation)
        } else {
            self.slots.push(TypedSlot { generation: 0, value: Some(value) });
            (self.slots.len() - 1, 0)
        }
    }

    /// Generation advances (a retained handle cannot see the recycled
    /// slot) and the value dies now.
    fn free(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            slot.generation += 1;
            slot.value = None;
            self.free.push(index);
        }
    }
}

thread_local! {
    /// One arena per `TypeId` — the `dyn Any` wraps the ARENA (cold edge,
    /// one downcast per access to the typed container), never the value.
    /// Every read and write of a state looks its arena up here, so the
    /// key is hashed the cheap way: a `TypeId` is the compiler's, never an
    /// outside caller's.
    static ARENAS: RefCell<FxHashMap<TypeId, Rc<dyn Any>>> = RefCell::new(FxHashMap::default());
    /// How the sweep frees without knowing `T`: one function pointer per
    /// type, registered when the arena is born.
    static FREERS: RefCell<FxHashMap<TypeId, fn(usize)>> = RefCell::new(FxHashMap::default());
    static NEXT_DEP: Cell<u64> = const { Cell::new(0) };
}

fn with_arena<T: 'static, R>(f: impl FnOnce(&mut TypedArena<T>) -> R) -> R {
    let cell = arena_cell::<T>();
    let result = f(&mut cell.borrow_mut());
    result
}

/// The arena of `T`, borrowed to READ: several reads of one type may
/// nest (a value's `Display` that reads another `State`); a write inside
/// one fails loudly, the rule `update` already keeps.
fn with_arena_ref<T: 'static, R>(f: impl FnOnce(&TypedArena<T>) -> R) -> R {
    let cell = arena_cell::<T>();
    let result = f(&cell.borrow());
    result
}

/// The arena of `T`, made on first use.
fn arena_cell<T: 'static>() -> Rc<RefCell<TypedArena<T>>> {
    ARENAS.with(|arenas| {
        let mut arenas = arenas.borrow_mut();
        arenas
            .entry(TypeId::of::<T>())
            .or_insert_with(|| {
                FREERS.with(|freers| {
                    freers.borrow_mut().insert(TypeId::of::<T>(), free_typed::<T>)
                });
                Rc::new(RefCell::new(TypedArena::<T>::new())) as Rc<dyn Any>
            })
            .clone()
            .downcast::<RefCell<TypedArena<T>>>()
            .expect("an arena registered by TypeId is always its own type")
    })
}

fn free_typed<T: 'static>(index: usize) {
    with_arena::<T, _>(|arena| arena.free(index));
}

/// The sweep of dead identities goes through here.
pub(crate) fn free_slot(type_id: TypeId, index: usize) {
    let Some(freer) = FREERS.with(|freers| freers.borrow().get(&type_id).copied()) else {
        return;
    };
    freer(index);
}

const DEAD_STATE: &str = "State of an unmounted identity (or from another thread) — \
                          the slot died together with the view";

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<T> {}

impl<T: Clone + 'static> State<T> {
    pub fn new(value: T) -> Self {
        match crate::identity::claim_anchor(TypeId::of::<T>()) {
            crate::identity::Claim::Existing { index, generation, dep } => {
                // identity already mounted: revive the slot, discard the initial
                State { index, generation, dep, _marker: PhantomData }
            }
            crate::identity::Claim::Fresh(token) => {
                let dep = NEXT_DEP.with(|next| {
                    let dep = next.get();
                    next.set(dep + 1);
                    dep
                });
                let (index, generation) = with_arena::<T, _>(|arena| arena.alloc(value));
                crate::identity::fulfill_anchor(token, index, generation, dep);
                State { index, generation, dep, _marker: PhantomData }
            }
        }
    }

    pub fn wrappedValue(&self) -> T {
        crate::identity::record_read(crate::identity::DepKey::State(self.dep));
        with_arena::<T, _>(|arena| {
            arena
                .slots
                .get(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.clone())
                .expect(DEAD_STATE)
        })
    }

    /// Reads the value in place: `f` sees it borrowed, and no clone is
    /// made — what a `text!` that only prints it needs. The read records
    /// itself as `get()`'s does. Reads of other `State`s may nest inside
    /// `f`; a write to a `State` of the same type inside it fails loudly,
    /// as `update`'s rule says.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        crate::identity::record_read(crate::identity::DepKey::State(self.dep));
        with_arena_ref::<T, _>(|arena| {
            let slot = arena
                .slots
                .get(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            f(slot.value.as_ref().expect(DEAD_STATE))
        })
    }

    /// Compound mutation that says whether it changed anything: `f`
    /// answers `(changed, result)`, and the write is recorded only when it
    /// did — a drain that found nothing leaves the scene as it was. The
    /// value leaves the arena while `f` runs, as in [`State::update`].
    pub fn update_if<R>(&self, f: impl FnOnce(&mut T) -> (bool, R)) -> R {
        let mut value = with_arena::<T, _>(|arena| {
            arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.take())
                .expect(DEAD_STATE)
        });
        let (changed, result) = f(&mut value);
        with_arena::<T, _>(|arena| {
            let slot = arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            slot.value = Some(value);
        });
        if changed {
            crate::identity::record_write(crate::identity::DepKey::State(self.dep));
        }
        result
    }

    pub fn set(&self, value: T) {
        with_arena::<T, _>(|arena| {
            let slot = arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            slot.value = Some(value);
        });
        crate::identity::record_write(crate::identity::DepKey::State(self.dep));
    }

    /// Compound mutation. The value leaves the arena while `f` runs (the
    /// arena is not left borrowed during user code — `f` may read OTHER
    /// `State`s of the same type); reentrant access to the SAME slot fails
    /// loudly.
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = with_arena::<T, _>(|arena| {
            arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.take())
                .expect(DEAD_STATE)
        });
        let result = f(&mut value);
        with_arena::<T, _>(|arena| {
            let slot = arena
                .slots
                .get_mut(self.index)
                .filter(|slot| slot.generation == self.generation)
                .expect(DEAD_STATE);
            slot.value = Some(value);
        });
        crate::identity::record_write(crate::identity::DepKey::State(self.dep));
        result
    }

    /// `$x` — the binding projection.
    pub fn binding(&self) -> Binding<T> {
        let for_set = *self;
        Binding {
            // a state's own binding lends its value in place: a field of
            // a megabyte reads it without a copy
            get: Rc::new(StateSource(*self)),
            set: Rc::new(move |value| for_set.set(value)),
        }
    }
}

impl<T: Clone + PartialEq + 'static> State<T> {
    /// Writes the value only when it differs from the one held — a poller
    /// that lands the same answer every tick wakes nobody, and a pump
    /// that bumps a counter by zero asks no frame. `true` when it wrote.
    /// The comparison records no read: a setter is not a reader.
    pub fn set_if_changed(&self, value: T) -> bool {
        let same = with_arena_ref::<T, _>(|arena| {
            arena
                .slots
                .get(self.index)
                .filter(|slot| slot.generation == self.generation)
                .and_then(|slot| slot.value.as_ref())
                .map(|held| *held == value)
                .expect(DEAD_STATE)
        });
        if same {
            return false;
        }
        self.set(value);
        true
    }

}

/// Displaying a `State` READS the value — the dependency records itself.
/// It is what makes `text!("count: {}", self.count)` react with no `.get()`
/// at all.
impl<T: Clone + std::fmt::Display + 'static> std::fmt::Display for State<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // in place: a label that prints a number clones nothing
        self.with(|value| value.fmt(f))
    }
}

/// `Binding<T>` — a get/set pair (`$x`, `@Binding`, `Binding.dispatched`).
pub struct Binding<T> {
    get: Rc<dyn Source<T>>,
    set: Rc<dyn Fn(T)>,
}

/// Where a binding's value comes from: a getter, which answers with a
/// copy, and a lend, which shows the value in place where the source can.
trait Source<T> {
    fn get(&self) -> T;
    fn lend(&self, visit: &mut dyn FnMut(&T));
    /// Edits the value where it lives and says whether it changed — `None`
    /// when the source cannot (its writes go through a setter).
    fn modify(&self, _edit: &mut dyn FnMut(&mut T) -> bool) -> Option<bool> {
        None
    }
}

/// A getter closure: it has nothing to lend, and lends the copy it makes.
struct Getter<F>(F);

impl<T, F: Fn() -> T> Source<T> for Getter<F> {
    fn get(&self) -> T {
        (self.0)()
    }

    fn lend(&self, visit: &mut dyn FnMut(&T)) {
        visit(&(self.0)())
    }
}

/// A state's own binding: it lends the value where it lives.
struct StateSource<T>(State<T>);

impl<T: Clone + 'static> Source<T> for StateSource<T> {
    fn get(&self) -> T {
        self.0.wrappedValue()
    }

    fn lend(&self, visit: &mut dyn FnMut(&T)) {
        self.0.with(|value| visit(value))
    }

    fn modify(&self, edit: &mut dyn FnMut(&mut T) -> bool) -> Option<bool> {
        Some(self.0.update_if(|value| {
            let changed = edit(value);
            (changed, changed)
        }))
    }
}

/// A source whose writes must reach a setter of their own (`onSet`): it
/// reads and lends as the source under it, and never edits in place.
struct Through<T>(Rc<dyn Source<T>>);

impl<T> Source<T> for Through<T> {
    fn get(&self) -> T {
        self.0.get()
    }

    fn lend(&self, visit: &mut dyn FnMut(&T)) {
        self.0.lend(visit)
    }
}

impl<T: Clone + 'static> Binding<T> {
    pub fn new(get: impl Fn() -> T + 'static, set: impl Fn(T) + 'static) -> Self {
        Binding { get: Rc::new(Getter(get)), set: Rc::new(set) }
    }

    pub fn wrappedValue(&self) -> T {
        self.get.get()
    }

    /// Reads the value in place: a state's own binding lends it without
    /// a clone, and records the read as `wrappedValue` does; a binding
    /// made of closures has nothing to lend, and `f` sees the value its
    /// getter returns.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        let mut f = Some(f);
        let mut answer = None;
        self.get.lend(&mut |value: &T| answer = f.take().map(|f| f(value)));
        answer.expect("a binding lends its value exactly once")
    }

    /// Edits the value and writes it back when `edit` says it changed. A
    /// state's own binding edits in place — no copy of the value, and no
    /// write at all when nothing changed; any other binding reads a copy,
    /// edits it, and hands it to its setter when it changed. True when it
    /// changed.
    pub fn modify(&self, mut edit: impl FnMut(&mut T) -> bool) -> bool {
        if let Some(changed) = self.get.modify(&mut edit) {
            return changed;
        }
        let mut value = self.get.get();
        let changed = edit(&mut value);
        if changed {
            (self.set)(value);
        }
        changed
    }

    pub fn set(&self, value: T) {
        (self.set)(value)
    }

    /// Compound mutation through the binding (`b.wrappedValue.field = v`).
    pub fn update<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.wrappedValue();
        let result = f(&mut value);
        self.set(value);
        result
    }

    /// `Binding.onSet { … }`
    pub fn onSet(self, perform: impl Fn(&T) + 'static) -> Self {
        let Binding { get, set } = self;
        let old_set = set;
        // the observer hears every write, so none may bypass the setter
        let get: Rc<dyn Source<T>> = Rc::new(Through(get));
        Binding { get, set: Rc::new(move |value| { old_set(value.clone()); perform(&value) }) }
    }

    /// `binding.field` — Swift's dynamic-member projection on `Binding<T>`
    /// (`routingBinding.detailsSheet`, a `Binding<Bool>` out of a
    /// `Binding<Routing>`).
    pub fn member<B: Clone + 'static>(
        &self,
        get: impl Fn(&T) -> B + 'static,
        set: impl Fn(&mut T, B) + 'static,
    ) -> Binding<B> {
        let old_get = self.get.clone();
        let old_get2 = self.get.clone();
        let old_set = self.set.clone();
        Binding::new(
            move || get(&old_get.get()),
            move |value| {
                let mut whole = old_get2.get();
                set(&mut whole, value);
                (old_set)(whole);
            },
        )
    }

    /// `Binding.dispatched(to: store, \.keyPath)`
    pub fn dispatched<S: Clone + 'static>(
        store: &Store<S>,
        get: impl Fn(&S) -> T + 'static,
        set: impl Fn(&mut S, T) + 'static,
    ) -> Self {
        let store_for_get = store.clone();
        let store_for_set = store.clone();
        Binding::new(
            move || get(&store_for_get.value()),
            move |value| store_for_set.update(|state| set(state, value)),
        )
    }
}

impl<T: Clone + 'static> Clone for Binding<T> {
    fn clone(&self) -> Self {
        Binding { get: self.get.clone(), set: self.set.clone() }
    }
}

/// What `.modelContainer(…)` needs to provide for `Query<T>` to fetch.
pub trait ProvidesQueries {
    fn querySource(&self) -> Rc<dyn Fn(&'static str) -> Option<Rc<dyn Any>>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_binding_lends_its_value_and_a_made_one_reads_it() {
        use std::cell::Cell;
        thread_local! { static CLONES: Cell<usize> = const { Cell::new(0) }; }
        #[derive(PartialEq, Debug)]
        struct Counted(usize);
        impl Clone for Counted {
            fn clone(&self) -> Self {
                CLONES.with(|n| n.set(n.get() + 1));
                Counted(self.0)
            }
        }
        let state = State::new(Counted(7));
        let binding = state.binding();
        CLONES.with(|n| n.set(0));
        assert_eq!(binding.with(|value| value.0), 7);
        assert_eq!(CLONES.with(Cell::get), 0, "a state's binding lends without a clone");
        binding.clone().onSet(|_| {}).with(|value| assert_eq!(value.0, 7));
        assert_eq!(CLONES.with(Cell::get), 0, "and keeps lending through onSet and clone");
        let made = Binding::new(move || state.wrappedValue(), move |value| state.set(value));
        assert_eq!(made.with(|value| value.0), 7);
        assert_eq!(CLONES.with(Cell::get), 1, "a made binding reads through its getter");
    }

    #[test]
    fn a_state_binding_edits_in_place_and_an_observed_one_writes_through() {
        let state = State::new(String::from("note"));
        let binding = state.binding();
        assert!(binding.modify(|text| {
            text.push('!');
            true
        }));
        assert_eq!(state.wrappedValue(), "note!");
        assert!(!binding.modify(|_| false), "an edit that changed nothing writes nothing");
        let heard = Rc::new(std::cell::Cell::new(0));
        let observed = {
            let heard = heard.clone();
            state.binding().onSet(move |_| heard.set(heard.get() + 1))
        };
        assert!(observed.modify(|text| {
            text.push('?');
            true
        }));
        assert_eq!(state.wrappedValue(), "note!?");
        assert_eq!(heard.get(), 1, "the observer hears an edit made through its binding");
        let made = Binding::new(move || state.wrappedValue(), move |value| state.set(value));
        assert!(made.modify(|text| {
            text.clear();
            true
        }));
        assert_eq!(state.wrappedValue(), "");
    }

    #[test]
    fn state_clones_share_storage() {
        let a = State::new(1);
        let b = a.clone();
        b.set(2);
        assert_eq!(a.wrappedValue(), 2);
    }

    #[test]
    fn binding_on_set_hooks() {
        let state = State::new(1);
        let fired = Rc::new(RefCell::new(false));
        let fired2 = fired.clone();
        let binding = state.binding().onSet(move |_| *fired2.borrow_mut() = true);
        binding.set(5);
        assert!( *fired.borrow());
        assert_eq!(state.wrappedValue(), 5);
    }

    /// A copy of a context shares its values — what every retained body
    /// keeps costs a count, not a copy of the locale — and a write copies
    /// them once, leaving every other holder with the values it had.
    #[test]
    fn a_context_copy_shares_its_values_until_one_is_written() {
        let mut ctx = Context::default();
        ctx.values = Rc::new(EnvironmentValues { locale: Locale::new("pt-BR"), ..EnvironmentValues::default() });
        let kept = ctx.clone();
        assert!(Rc::ptr_eq(&kept.values, &ctx.values), "a copy shares the values");
        Rc::make_mut(&mut ctx.values).locale = Locale::new("en");
        assert!(!Rc::ptr_eq(&kept.values, &ctx.values), "the write copied them");
        assert_eq!(kept.values.locale.identifier(), "pt-BR", "and the copy kept what it had");
        assert_eq!(ctx.environment::<Locale>().identifier(), "en");
    }

    // MARK: - The locale is a list

    /// However a platform spells a tag, the list keeps one spelling:
    /// the language lower, a script in Title case, a region upper, the
    /// POSIX encoding and modifier cut away.
    #[test]
    fn a_locale_is_canonical_however_it_was_spelled() {
        assert_eq!(Locale::new("PT_br.UTF-8@latin").as_str(), "pt-BR");
        assert_eq!(Locale::new("ZH-hant-tw").as_str(), "zh-Hant-TW");
        assert_eq!(Locale::new("sr-latn-rs").as_str(), "sr-Latn-RS");
        let latin_america = Locale::new("es-419");
        assert_eq!(latin_america.region(), Some("419"));
        assert_eq!(latin_america.script(), None);
        assert_eq!(latin_america.language(), "es");
        assert_eq!(Locale::new("en").to_string(), "en");
    }

    /// A `LANGUAGE` variable, an `Accept-Language` header and a joined
    /// `navigator.languages` all become one list, best first; `C`,
    /// `POSIX`, a wildcard and a weight are not languages.
    #[test]
    fn a_posix_list_becomes_a_preference_list() {
        assert_eq!(Locale::parse("pt_BR:en_US:C").as_str(), "pt-BR,en-US");
        assert_eq!(Locale::parse("pt-BR,pt;q=0.9,en;q=0.8,*").as_str(), "pt-BR,pt,en");
        assert_eq!(Locale::parse("C"), Locale::default());
        assert_eq!(Locale::parse("POSIX").as_str(), "en");
        assert_eq!(Locale::parse(""), Locale::default());
        assert_eq!(Locale::preferred(["fr-CH", "de_CH.UTF-8"]).as_str(), "fr-CH,de-CH");
        assert_eq!(Locale::new("").as_str(), "en");
    }

    #[test]
    fn a_repeated_tag_is_kept_once() {
        assert_eq!(Locale::parse("en-US,en-us,EN_us,en").as_str(), "en-US,en");
        assert_eq!(Locale::preferred(["pt", "pt"]).tags().count(), 1);
    }

    /// Every read of the environment clones the locale: the clone has to
    /// be a count, or a thousand rows would allocate a thousand strings.
    #[test]
    fn a_locale_copy_is_a_count_not_a_string() {
        let locale = Locale::parse("pt-BR,en");
        let copy = locale.clone();
        assert_eq!(locale.as_str().as_ptr(), copy.as_str().as_ptr(), "one string, two holders");
        assert_eq!(std::mem::size_of::<Locale>(), std::mem::size_of::<Rc<str>>());
        assert_eq!(locale, copy);
    }

    #[test]
    fn a_locale_falls_from_its_region_to_its_language() {
        assert_eq!(Locale::new("pt-BR").pick(&["en", "pt"]), Some(1));
        assert_eq!(Locale::new("pt-Latn-BR").pick(&["en", "pt-Latn"]), Some(1));
        // the same language in another region beats the next preference
        assert_eq!(Locale::parse("pt-BR,en").pick(&["en", "pt-PT"]), Some(1));
    }

    #[test]
    fn a_locale_prefers_an_exact_tag_over_its_language() {
        assert_eq!(Locale::new("pt-BR").pick(&["pt", "pt-BR"]), Some(1));
        assert_eq!(Locale::new("pt").pick(&["pt-BR", "pt"]), Some(1));
        // a bare language with no exact table takes the region's
        assert_eq!(Locale::new("pt").pick(&["en", "pt-BR"]), Some(1));
    }

    #[test]
    fn a_second_preference_speaks_when_the_first_is_unknown() {
        assert_eq!(Locale::parse("gsw,fr-CH").pick(&["en", "fr"]), Some(1));
        assert_eq!(Locale::parse("ja,pt-BR,en").pick(&["en", "pt"]), Some(1));
    }

    #[test]
    fn a_tag_matches_regardless_of_case() {
        assert_eq!(Locale::new("pt-br").pick(&["PT-BR"]), Some(0));
        assert_eq!(Locale::new("ZH-TW").pick(&["zh-hant"]), Some(0));
    }

    /// Chinese is written two ways, and a region says which: a reader
    /// in Taiwan is served the traditional table, one in Singapore the
    /// simplified, and bare `zh` is simplified.
    #[test]
    fn a_chinese_region_implies_its_script() {
        assert_eq!(Locale::new("zh-TW").pick(&["zh-Hans", "zh-Hant"]), Some(1));
        assert_eq!(Locale::new("zh-HK").pick(&["zh-Hans", "zh-Hant"]), Some(1));
        assert_eq!(Locale::new("zh-SG").pick(&["zh-Hant", "zh-Hans"]), Some(1));
        assert_eq!(Locale::new("zh").pick(&["zh-TW", "zh-CN"]), Some(1));
        assert_eq!(Locale::new("zh-Hant").pick(&["zh-CN", "zh-TW"]), Some(1));
        // no table of the right script: the language alone does not serve
        assert_eq!(Locale::new("zh-TW").pick(&["zh-Hans"]), None);
    }

    #[test]
    fn a_legacy_language_code_meets_its_modern_name() {
        assert_eq!(Locale::new("iw").as_str(), "he");
        assert_eq!(Locale::new("iw-IL").pick(&["en", "he"]), Some(1));
        assert_eq!(Locale::new("he").pick(&["en", "iw"]), Some(1));
        assert_eq!(Locale::new("in").language(), "id");
        assert_eq!(Locale::new("ji").language(), "yi");
    }

    #[test]
    fn a_locale_nobody_speaks_picks_nothing() {
        assert_eq!(Locale::new("ja").pick(&["en", "pt"]), None);
        assert_eq!(Locale::new("ja").pick(&[]), None);
        assert_eq!(Locale::new("ja").pick_in(["en", "pt"].into_iter()), None);
    }

    #[test]
    fn a_script_decides_the_direction_before_the_language() {
        use LayoutDirection::{LeftToRight, RightToLeft};
        assert_eq!(Locale::new("ar").direction(), RightToLeft);
        assert_eq!(Locale::new("he-IL").direction(), RightToLeft);
        assert_eq!(Locale::new("fa").direction(), RightToLeft);
        assert_eq!(Locale::new("ur-PK").direction(), RightToLeft);
        assert_eq!(Locale::new("ckb").direction(), RightToLeft);
        assert_eq!(Locale::new("iw").direction(), RightToLeft, "the legacy code reads the same");
        assert_eq!(Locale::new("ar-Latn").direction(), LeftToRight, "romanized Arabic reads left to right");
        assert_eq!(Locale::new("az-Arab").direction(), RightToLeft, "Azerbaijani in Arabic script does not");
        assert_eq!(Locale::new("pa-Arab-PK").direction(), RightToLeft);
        assert_eq!(Locale::new("pa").direction(), LeftToRight);
        assert_eq!(Locale::parse("ar,en").direction(), RightToLeft, "the first tag decides");
        assert_eq!(Locale::parse("en,ar").direction(), LeftToRight);
    }

    #[test]
    fn the_default_locale_is_english_and_reads_left_to_right() {
        let locale = Locale::default();
        assert_eq!(locale.identifier(), "en");
        assert_eq!(locale.language(), "en");
        assert_eq!(locale.region(), None);
        assert_eq!(locale.direction(), LayoutDirection::LeftToRight);
        assert_eq!(LayoutDirection::default(), LayoutDirection::LeftToRight);
        assert!(!LayoutDirection::LeftToRight.is_rtl() && LayoutDirection::RightToLeft.is_rtl());
        let values = EnvironmentValues::default();
        assert_eq!(values.layoutDirection, LayoutDirection::LeftToRight);
        assert_eq!(Context::default().environment::<LayoutDirection>(), LayoutDirection::LeftToRight);
    }
}
