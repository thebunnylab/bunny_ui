//! View modifiers — the idiomatic `ViewExt`.
//!
//! Each method returns a typed [`Modified<Self>`]: chaining `.font(…)`
//! allocates nothing and erases no type — the whole chain becomes a
//! monomorphic value known at compile time.
//!
//! The trait only exists for `Arity = Single` (one node): a modifier on
//! a raw tuple does not compile — see [`Modified`].
//!
//! The `site`s of `on_change`/`on_receive` — the identity the
//! change-detection slot needs across renders — come from
//! `#[track_caller]`: each call site is its own site, with no manual
//! string to invent (nor collide). When the same call is shared by a
//! helper and needs to tell its uses apart, there are the `_keyed`
//! variants.

use std::panic::Location;
use std::rc::Rc;

use motor::combine::IntoPublisher;
use motor::runtime::Site;
use motor::state::{Binding, Context, ProvidesQueries};
use motor::views::Query;

use crate::action::ActionId;
use crate::effects;
use crate::erased::Erased;
use crate::layout::Color;
use crate::modifier::{Modified, Modifier};
use crate::view::{Single, View, render_line, short_type_name};
use crate::views::Alignment;
use motor::views::{ContentMode, Edge, Font, ListStyle, ProgressViewStyle, TextAlignment};

pub trait ViewExt: View<Arity = Single> + Sized {
    // MARK: - Formatting

    /// `.font(.title)`
    fn font(self, font: Font) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Font(font),
        }
    }

    /// `.font(.system(size: 9))` — a measure the preset scale does not
    /// have. Only the size travels, so `.font_size(9.0).bold()` keeps
    /// the weight and the design in scope. A `.font(.title)` CLOSER to
    /// the view brings its own size and wins: the nearest patch always
    /// does.
    fn font_size(self, size: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FontSize(size),
        }
    }

    /// `.id("source-control")` — the view takes a NAME in the identity.
    ///
    /// Without it a view is known by its POSITION among siblings
    /// (`Stripe/#0/@First/#2`), so inserting a sibling above renames it:
    /// its state, its animation scope and the path its clicks register
    /// under all move. With it, the name is the address — which is also
    /// how a test or a headless probe finds a control it did not build.
    fn id(self, name: impl Into<std::rc::Rc<str>>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Id(name.into()),
        }
    }

    /// `.bold()`
    fn bold(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Bold,
        }
    }

    /// `.padding()`
    fn padding(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Padding,
        }
    }

    /// `.padding(12)` — uniform with an explicit measure.
    fn padding_length(self, length: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::PaddingLength(length),
        }
    }

    /// `.padding(.bottom, 40)`
    fn padding_edge(self, edge: Edge, length: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::PaddingEdge(edge, length),
        }
    }

    /// `.frame(width: 120, height: 80)`
    fn frame(self, width: f64, height: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameWH(width, height),
        }
    }

    /// `.frame(width: 120)` — exact on one axis, natural on the other.
    /// The panel column and the fixed-height row live here; `frame_max`
    /// is a CEILING, not a size, and quietly hands back what the content
    /// does not use.
    fn frame_width(self, width: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameWidth(width),
        }
    }

    /// `.frame(height: 80)` — the vertical sibling of [`ViewExt::frame_width`].
    fn frame_height(self, height: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameHeight(height),
        }
    }

    /// `.frame(maxWidth: .infinity, maxHeight: 60, alignment: .leading)`
    fn frame_max(self, max_width: f64, max_height: f64, alignment: Alignment) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FrameMax(max_width, max_height, alignment),
        }
    }

    /// `.navigationTitle("…")`
    fn navigation_title(self, title: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationTitle(title.into()),
        }
    }

    /// `.navigationBarTitle("…")`
    fn nav_bar_title(self, title: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationBarTitle(title.into()),
        }
    }

    /// `.listStyle(.grouped)`
    fn list_style(self, style: ListStyle) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ListStyle(style),
        }
    }

    /// `.progressViewStyle(.circular)`
    fn progress_style(self, style: ProgressViewStyle) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ProgressViewStyle(style),
        }
    }

    /// `.multilineTextAlignment(.center)`
    fn multiline_text_alignment(self, alignment: TextAlignment) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::MultilineTextAlignment(alignment),
        }
    }

    /// `.aspectRatio(contentMode: .fit)`
    fn aspect_ratio(self, mode: ContentMode) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AspectRatio(mode),
        }
    }

    /// `.resizable()`
    fn resizable(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Resizable,
        }
    }

    /// `.blur(radius: 10)`
    fn blur(self, radius: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Blur(radius),
        }
    }

    /// `.ignoresSafeArea()`
    fn ignores_safe_area(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::IgnoresSafeArea,
        }
    }

    /// `.background { … }` — describes the content without mounting it.
    fn background<C: View<Arity = Single>>(self, content: C) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Background(render_line(&content)),
        }
    }

    /// `.hidden()`
    fn hidden(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Hidden,
        }
    }

    // MARK: - Visuals (semantic node properties — `Styled` in the scene)

    /// `.background(Color.red)` — a solid color as a node property (the
    /// content `.background { view }` is the other method).
    fn background_color(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::BackgroundColor(color),
        }
    }

    /// `.background(RadialGradient(…))` — a two-stop ramp behind the
    /// view, over whatever flat background it already has.
    ///
    /// ```ignore
    /// // the glow of a hero panel: violet at the top, fading out
    /// .background_gradient(
    ///     Gradient::radial(VIOLET, VIOLET.fade())
    ///         .center(UnitPoint::TOP)
    ///         .radius(0.0, 420.0),
    /// )
    /// // a bar with a sheen
    /// .background_gradient(Gradient::linear(TOP_INK, BOTTOM_INK))
    /// ```
    fn background_gradient(self, gradient: crate::layout::Gradient) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::BackgroundGradient(gradient),
        }
    }

    /// `.rendering(Rendering::Gpu)` — on the web's element lowering
    /// this subtree insists on the pixel pipeline: a canvas island our
    /// layout positions, filled with the subtree's own draw commands.
    /// Everywhere else the modifier dissolves (pixel targets already
    /// are the pixel pipeline). `Rendering::Auto` is the default table:
    /// v1 lowers everything to elements.
    /// The REAL element the Dom lowering builds for this view — a
    /// `<tr>`, a `<td>`, a `<button>`. Semantic HTML when the page
    /// wants it; every other lowering ignores the hint, the way a
    /// pixel target ignores `.rendering()`.
    fn element(self, tag: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ElementHint(Some(std::rc::Rc::from(tag.into().as_str())), None, None),
        }
    }

    /// The Dom element's `class` attribute — for a stylesheet the
    /// page brings, or a harness that looks elements up by it.
    fn css_class(self, class: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ElementHint(None, Some(std::rc::Rc::from(class.into().as_str())), None),
        }
    }

    /// The Dom element's `id` attribute.
    fn element_id(self, id: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ElementHint(None, None, Some(std::rc::Rc::from(id.into().as_str())),),
        }
    }

    /// The element lowering keeps the ENGINE's layout for this
    /// subtree: measured, placed and positioned by our numbers —
    /// pixel parity with the canvas where the design demands it.
    /// The flow around it never notices.
    #[cfg(feature = "canvas")]
    fn layout(self, mode: crate::layout::LayoutMode) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::LayoutMode(mode),
        }
    }

    #[cfg(feature = "canvas")]
    fn rendering(self, mode: crate::layout::Rendering) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Rendering(mode),
        }
    }

    /// `.animated(Spring::smooth())` — colors under this view move
    /// through the spring when they change (state, hover) instead of
    /// jumping. Put it AFTER the props it animates; the nearest styled
    /// below consumes the scope.
    fn animated(self, spring: crate::anim::Spring) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Animated(spring),
        }
    }

    /// `.foregroundColor(.secondary)` — inherited by the text below.
    fn foreground_color(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ForegroundColor(color),
        }
    }

    /// `.border(Color.gray, width: 1)` — a frame inward from the edge.
    fn border(self, color: Color, width: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Border(color, width),
        }
    }

    /// `.cornerRadius(8)` — rounds THIS node's background.
    fn corner_radius(self, radius: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::CornerRadius(radius),
        }
    }

    /// `.clipped()` — the subtree cannot paint outside this box, and
    /// the cut FOLLOWS `.corner_radius(…)` when there is one (a plain
    /// rectangle without it). The island that finally holds its
    /// children in: panels with backgrounds of their own stop leaking
    /// over the curve. Put it anywhere in the chain — it fuses into
    /// the same node the radius rides, so the order never matters.
    fn clipped(self) -> Modified<Self> {
        Modified { base: self, modifier: Modifier::Clipped }
    }

    /// `.monospaced()` — swaps the inherited font's design (grids).
    fn monospaced(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Monospaced,
        }
    }

    /// Alternate background under the nearest interactive target's hover
    /// (list rows, chips) — pure paint, layout untouched (the LAW).
    fn background_hovered(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::BackgroundHovered(color),
        }
    }

    /// Alternate background under pressed — the pair of
    /// [`ViewExt::background_hovered`].
    fn background_pressed(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::BackgroundPressed(color),
        }
    }

    /// Alternate INK under the nearest interactive target's hover: a
    /// faint glyph that brightens when the pointer arrives. It reaches
    /// every text below, the way `.foreground_color` does, and it is
    /// paint only — the measure never hears about it (the LAW).
    fn foreground_hovered(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ForegroundHovered(color),
        }
    }

    /// Alternate ink under pressed — the pair of
    /// [`ViewExt::foreground_hovered`].
    fn foreground_pressed(self, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ForegroundPressed(color),
        }
    }

    /// `.onClick { … }` — the view becomes a pointer target WITHOUT the
    /// `Button` chrome: same action retention, same up-inside. It is the
    /// clickable list row.
    fn on_click(self, action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnClick(Rc::new(action)),
        }
    }

    /// `.on_action(SELECT_NEXT, move || …)` — registers the named
    /// action's handler in this subtree. Two handlers with the same id:
    /// the innermost wins. Retained like the click actions: a skipped
    /// view responds. The key arrives through the `Runtime`'s keymap
    /// (`bind` + the shell's gate).
    fn on_action(self, id: ActionId, handler: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnAction(id, Rc::new(handler)),
        }
    }

    /// Paints stretches of the TEXT in another color (byte ranges into
    /// the content) — a finder's match highlight. Only affects `text(…)`.
    fn highlight(self, ranges: Vec<(usize, usize)>, color: Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Highlight(Rc::new(ranges), color),
        }
    }

    /// `.truncationMode(.middle)` — turns wrapping off: the text becomes
    /// ONE line with an ellipsis at the chosen spot when it does not fit.
    fn truncation_mode(self, mode: crate::layout::Truncation) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::TruncationMode(mode),
        }
    }

    /// The scroll region follows this item id: when the id CHANGES, the
    /// region scrolls just enough to reveal the row (keyboard selection
    /// stays visible); the wheel stays sovereign in between.
    fn scroll_target(self, id: impl Into<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::ScrollTarget(id.into()),
        }
    }

    /// The field receives focus on its FIRST appearance — and never
    /// again (a user blur is final; remount under the same identity
    /// does not re-focus).
    fn auto_focus(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AutoFocus,
        }
    }

    /// A soft shadow behind the view — panel-grade halo with the house
    /// ink (quadratic falloff, paints outside the frame only).
    fn shadow(self, radius: f64) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Shadow(radius, crate::layout::Color::rgba(0, 0, 0, 80)),
        }
    }

    /// [`ViewExt::shadow`] with an explicit color (tinted halos, deeper
    /// panels).
    fn shadow_color(self, radius: f64, color: crate::layout::Color) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Shadow(radius, color),
        }
    }

    /// Declares a key context ACTIVE while this view is mounted —
    /// `Runtime::bind_in(context, …)` bindings answer only then. The
    /// palette closes, its keys go quiet.
    fn key_context(self, name: &'static str) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::KeyContext(name),
        }
    }

    // MARK: - Interaction

    /// `.onTapGesture { … }` — in the headless runtime it fires on render.
    fn on_tap(self, action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnTapGesture(Rc::new(action)),
        }
    }

    /// `.onAppear { … }` — fires on render (motor parity).
    fn on_appear(self, action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::OnAppear(Rc::new(action)),
        }
    }

    /// `.onChange(of:initial:) { old, new in … }`
    ///
    /// The change-detection slot's site is the callsite itself
    /// (`#[track_caller]`). If the call lives in a reused helper and each
    /// use needs its own slot, use [`ViewExt::on_change_keyed`].
    #[track_caller]
    fn on_change<V: Clone + PartialEq + 'static>(
        self,
        of: impl Fn() -> V + 'static,
        initial: bool,
        action: impl Fn(&V, &V) + 'static,
    ) -> Modified<Self> {
        self.on_change_keyed(Location::caller(), of, initial, action)
    }

    /// `.onChange` with an explicit site — for helpers that emit the same
    /// callsite for uses that need distinct slots.
    fn on_change_keyed<V: Clone + PartialEq + 'static>(
        self,
        site: impl Into<Site>,
        of: impl Fn() -> V + 'static,
        initial: bool,
        action: impl Fn(&V, &V) + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "onChange",
                detail: "()",
                effect: effects::change_effect(site.into(), of, initial, action),
            },
        }
    }

    /// `.onReceive(publisher) { value in … }`
    ///
    /// The site (`#[track_caller]`) retains the subscription across
    /// renders — SwiftUI's view identity. Without it, every re-render
    /// would create a new publisher (zeroed dedup cell) that would
    /// deliver the current value again, reporting a "change" on every
    /// pump and never stabilizing.
    #[track_caller]
    fn on_receive<V: Clone + PartialEq + 'static>(
        self,
        publisher: impl IntoPublisher<V>,
        action: impl Fn(V) + 'static,
    ) -> Modified<Self> {
        self.on_receive_keyed(Location::caller(), publisher, action)
    }

    /// `.onReceive` with an explicit site — same case as
    /// [`ViewExt::on_change_keyed`].
    fn on_receive_keyed<V: Clone + PartialEq + 'static>(
        self,
        site: impl Into<Site>,
        publisher: impl IntoPublisher<V>,
        action: impl Fn(V) + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "onReceive",
                detail: "()",
                effect: effects::receive_effect(site.into(), publisher.into_publisher(), action),
            },
        }
    }

    /// `.task { await … }` — work that belongs to this view. It starts
    /// on the view's first appearance, never restarts on a re-render,
    /// and is CANCELLED when the view leaves the tree.
    ///
    /// The framework opens no file and no socket: the task is where the
    /// app does that. What the future needs to own it creates inside
    /// itself, which is what lets the closure stay `Fn` (and a
    /// [`ViewExt::task_id`] restart possible):
    ///
    /// ```ignore
    /// row.task(move || async move {
    ///     let (lines, reader) = task::channel();
    ///     std::thread::spawn(move || read_the_log(lines));
    ///     while let Some(line) = reader.recv().await {
    ///         log.update(|all| all.push(line));
    ///     }
    /// })
    /// ```
    ///
    /// Cancelling drops the future where it stands: the reader dies,
    /// the worker's next `send` answers `Err`, and that is the signal
    /// to stop working.
    #[track_caller]
    fn task<F, Fut>(self, start: F) -> Modified<Self>
    where
        F: Fn() -> Fut + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.task_keyed(Location::caller(), None, start)
    }

    /// `.task(id:) { await … }` — the same, plus a restart: an `id`
    /// that moves cancels what runs and starts the work again.
    #[track_caller]
    fn task_id<I, F, Fut>(self, id: I, start: F) -> Modified<Self>
    where
        I: std::fmt::Display,
        F: Fn() -> Fut + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        self.task_keyed(Location::caller(), Some(id.to_string()), start)
    }

    /// `.task` with an explicit site — same case as
    /// [`ViewExt::on_change_keyed`].
    fn task_keyed<F, Fut>(
        self,
        site: impl Into<Site>,
        id: Option<String>,
        start: F,
    ) -> Modified<Self>
    where
        F: Fn() -> Fut + 'static,
        Fut: std::future::Future<Output = ()> + 'static,
    {
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "task",
                detail: if id.is_some() { "(id:)" } else { "()" },
                effect: effects::task_effect(site.into(), id, start),
            },
        }
    }

    /// `.searchable(text: $searchText)`
    fn searchable(self, _text: Binding<String>) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Searchable,
        }
    }

    /// `.refreshable { … }` — a user gesture; inert headless.
    fn refreshable(self, _action: impl Fn() + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Refreshable,
        }
    }

    /// `.sheet(isPresented: $flag) { … }` — the content is the erased
    /// boundary: mounts only when presented (close it with [`erased`]).
    ///
    /// [`erased`]: crate::erased::erased
    fn sheet(
        self,
        is_presented: Binding<bool>,
        content: impl Fn(&Context) -> Erased + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Sheet {
                is_presented,
                content: Rc::new(content),
            },
        }
    }

    /// Marks THIS view as a window-drag handle: on a chrome-less
    /// desktop window (`Chrome::Scene`), pressing it where no button
    /// wins drags the window — the scene's own title bar. Shells
    /// without a window to drag ignore it honestly (web, headless).
    fn window_drag_region(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::WindowDragRegion,
        }
    }

    /// An anchored popover: THIS view is the anchor, `side` the
    /// preferred edge (flip-then-clamp when there is no room). Closes
    /// on Escape and on a press outside — the press is consumed, never
    /// forwarded. On the desktop shell the popover may leave the
    /// window; everywhere else it clamps inside the viewport.
    ///
    /// ```ignore
    /// row.popover(showing.binding(), Side::Trailing, move |_| {
    ///     details_card(&item).erased()
    /// })
    /// ```
    fn popover(
        self,
        is_presented: Binding<bool>,
        side: crate::layout::Side,
        content: impl Fn(&Context) -> Erased + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Popover {
                is_presented,
                side,
                on_dismiss: None,
                content: Rc::new(content),
            },
        }
    }

    /// [`Self::popover`] with a dismissal callback: it runs after ANY
    /// of the framework's dismissal doors closes the popover — the
    /// outside press, Escape, a scrolled-away anchor, an app switch.
    /// The app clearing the binding itself does not count (it already
    /// knows).
    fn popover_on_dismiss(
        self,
        is_presented: Binding<bool>,
        side: crate::layout::Side,
        on_dismiss: impl Fn() + 'static,
        content: impl Fn(&Context) -> Erased + 'static,
    ) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Popover {
                is_presented,
                side,
                on_dismiss: Some(Rc::new(on_dismiss)),
                content: Rc::new(content),
            },
        }
    }

    /// `.toolbar { … }` — inert in the fake runtime, as in the motor.
    fn toolbar(self, _items: impl View) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Toolbar,
        }
    }

    // MARK: - Data & environment

    /// `.inject(diContainer)`
    fn inject<T: 'static>(self, container: Rc<T>) -> Modified<Self> {
        let detail = format!("({})", short_type_name::<T>());
        Modified {
            base: self,
            modifier: Modifier::EnvSet {
                name: "inject",
                detail,
                set: Rc::new(move |values: &mut motor::state::EnvironmentValues| {
                    values.injected = Some(container.clone());
                }),
            },
        }
    }

    /// `.modelContainer(container)`
    fn model_container<T: ProvidesQueries + 'static>(self, container: Rc<T>) -> Modified<Self> {
        let source = container.querySource();
        Modified {
            base: self,
            modifier: Modifier::EnvSet {
                name: "modelContainer",
                detail: "(…)".into(),
                set: Rc::new(move |values: &mut motor::state::EnvironmentValues| {
                    values.querySource = Some(source.clone());
                }),
            },
        }
    }

    /// `.modifier(RootViewAppearance(…))` — re-applied on every render.
    fn modifier(self, custom: impl crate::erased::CustomModifier + 'static) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::Custom(Rc::new(custom)),
        }
    }

    /// `.query(searchText:results:) { search in Query(…) }` — the fake @Query.
    fn query<T: Clone + PartialEq + 'static>(
        self,
        search_text: String,
        results: Binding<Vec<T>>,
        build: impl Fn(String) -> Query<T> + 'static,
    ) -> Modified<Self> {
        let effect: motor::state::EffectFn = Rc::new(move |ctx: &Context| {
            let Some(source) = ctx.values.querySource.clone() else {
                return false;
            };
            let Some(any) = source(std::any::type_name::<T>()) else {
                return false;
            };
            let Ok(storage) = any.downcast::<std::cell::RefCell<Vec<T>>>() else {
                return false;
            };

            let query = build(search_text.clone());
            let items: Vec<T> = storage
                .borrow()
                .iter()
                .filter(|item| (query.filter)(item))
                .cloned()
                .collect();
            let mut keyed: Vec<(String, T)> = items
                .into_iter()
                .map(|item| ((query.sortKey)(&item), item))
                .collect();
            keyed.sort_by(|a, b| a.0.cmp(&b.0));
            let items: Vec<T> = keyed.into_iter().map(|(_, item)| item).collect();

            if results.wrappedValue() != items {
                results.set(items);
                true
            } else {
                false
            }
        });
        Modified {
            base: self,
            modifier: Modifier::Effect {
                name: "query",
                detail: "(searchText: …, results: $…)",
                effect,
            },
        }
    }

    // MARK: - Inert parity

    /// `.navigationDestination(for:) { … }` — inert (the fake
    /// NavigationPath carries only descriptions; the destination does not
    /// mount).
    fn navigation_destination(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationDestination,
        }
    }

    /// `.navigationViewStyle(.stack)`
    fn navigation_view_style(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::NavigationViewStyle,
        }
    }

    /// `.attachEnvironmentOverrides()`
    fn attach_environment_overrides(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AttachEnvironmentOverrides,
        }
    }

    /// `.attachEnvironmentOverrides(onChange: …)`
    fn attach_environment_overrides_on_change(self) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::AttachEnvironmentOverridesOnChange,
        }
    }

    /// `.flipsForRightToLeftLayoutDirection(true)`
    fn flips_for_right_to_left_layout_direction(self, flips: bool) -> Modified<Self> {
        Modified {
            base: self,
            modifier: Modifier::FlipsForRightToLeftLayoutDirection(flips),
        }
    }
}

impl<V: View<Arity = Single>> ViewExt for V {}
