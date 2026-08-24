//! The FLOW lowering: the semantic tree becomes a semantic scene, and
//! the browser lays it out.
//!
//! This walk is the law applied to its last holdout. Everywhere else
//! a `vstack.spacing(8)` stays a stack until a backend consumes it —
//! only the element lowering flattened it into coordinates. Here the
//! stack lowers to what it IS in the browser's own language:
//! `display:flex; flex-direction:column; gap:8px`. No measure pass,
//! no place pass, no text engine — the walk transcribes semantics,
//! and the one layout engine written in C++ that ships with every
//! page does the arithmetic.
//!
//! What still owns numbers: a virtual list's row extents (declared by
//! the app, turned into prefix sums here — pure arithmetic), a canvas
//! island's size (the engine measures its own pixels), and one day a
//! `.layout(Exact)` interior. Everything else speaks records.

use motor::hash::FxHashMap as HashMap;

use crate::dom::{DomHints, DomKind, DomLayout, DomNode, DomStyle, DomText};
use crate::layout::{Axis, Color, CrossAlign, Edges, LayoutNode, Point, Px};
use crate::text_engine::FontSpec;

/// What the walk reads from the runtime — nothing here implies a
/// layout pass.
pub(crate) struct FlowEnv<'a> {
    /// Scroll offsets by region path (the browser reported them).
    pub scroll_offsets: &'a HashMap<String, Point>,
    /// The window box: the root's width and height.
    pub size: (Px, Px),
    /// The island door: a canvas island still measures and paints
    /// through the engine, LOCALLY — the one place a flow frame may
    /// run measure and place, and only under an island's own root.
    pub layout: Option<crate::layout::LayoutEnv<'a>>,
    /// Every body that ran this frame — a boundary with none of them
    /// under it is CLEAN, and the walk promises its reuse instead of
    /// descending.
    pub changed: &'a [String],
    /// The Groups the retained scene actually holds — a promise the
    /// diff cannot match would mount a hole, so the walk checks first.
    pub retained_groups: &'a std::collections::HashSet<std::rc::Rc<str>>,
    /// Browser-reported boxes by island path — a FLEXIBLE island
    /// measures against its real box, not against a guess.
    #[cfg_attr(not(feature = "canvas"), allow(dead_code))]
    pub island_boxes: &'a HashMap<std::rc::Rc<str>, (f64, f64)>,
    /// Which drop targets a live drag rings, in the order the walk
    /// meets them. The pixel path compares rectangles; a flow frame
    /// holds no geometry, so the runtime resolves the ring against the
    /// last layout's regions and the walk reads the answers in order —
    /// both walks record a target BEFORE descending into it, so the
    /// two orders are the same one.
    pub drop_rings: &'a [bool],
}

/// What the walk hands back beside the scene.
pub(crate) struct FlowOutput {
    pub scene: DomNode,
    /// The islands' draw commands — every island's range indexes here,
    /// already in island-local coordinates (each subtree placed at its
    /// own origin).
    pub display: crate::layout::DisplayList,
    /// The fields on stage: `(path, wants the first focus)`.
    pub fields: Vec<(String, bool)>,
    /// The app's boxes inside each island, with ISLAND-LOCAL frames —
    /// exactly the coordinates the browser reports on the canvas.
    pub customs: Vec<(std::rc::Rc<str>, crate::layout::CustomPlacement)>,
}

/// Lowers the semantic tree to a flow scene. The root is the mount
/// point (id 0): the theme's canvas, the window's box.
pub(crate) fn lower(root: &LayoutNode, env: &FlowEnv) -> FlowOutput {
    let mut walk = Walk {
        env,
        ink: Vec::new(),
        ink_scopes: Vec::new(),
        font: FontSpec::DEFAULT,
        line_height: None,
        text_align: None,
        pending_interactive: None,
        pending_transition: None,
        pending_tooltip: None,
        groups: Vec::new(),
        overlay_depth: 0,
        drops_seen: 0,
        overlays: Vec::new(),
        display: crate::layout::DisplayList::default(),
        fields: Vec::new(),
        slot: (None, None),
        pending_boundary_class: None,
        customs: Vec::new(),
    };
    let mut children = Vec::new();
    walk.lower_into(root, &mut children);
    // the mount point is a one-slot column and the window's box is
    // the offer — a flexible app takes it
    Walk::stamp_fill(root, &mut children);
    // popovers mount as the root's LAST children — the portal, by
    // construction, same contract as the absolute capture
    let overlays = std::mem::take(&mut walk.overlays);
    children.extend(overlays);
    let scene = DomNode {
        kind: DomKind::Root,
        x: 0.0,
        y: 0.0,
        width: env.size.0,
        height: env.size.1,
        style: DomStyle {
            background: Some(crate::theme::current().canvas),
            ..DomStyle::default()
        },
        // the root is the one ABSOLUTE citizen of a flow scene: the
        // window's box is real geometry, and a resize is its SetSize
        layout: None,
        hints: DomHints::default(),
        children,
    };
    FlowOutput {
        scene,
        display: walk.display,
        fields: walk.fields,
        customs: walk.customs,
    }
}

struct Walk<'a> {
    env: &'a FlowEnv<'a>,
    /// The inherited ink — the top colors the text (the capture's
    /// exact rule set rides here unchanged).
    ink: Vec<Color>,
    /// Depths where a hover/pressed ink opened: inside one, text
    /// inherits instead of painting its own color.
    ink_scopes: Vec<usize>,
    font: FontSpec,
    /// The inherited line box, mirroring `font` — the browser steps the
    /// lines by it, the way our own placement does.
    line_height: Option<crate::layout::Px>,
    /// The inherited line alignment, mirroring `line_height`.
    text_align: Option<motor::views::TextAlignment>,
    pending_interactive: Option<std::rc::Rc<str>>,
    pending_transition: Option<(f64, f64)>,
    /// A tooltip armed by a wrapper, landing on the next box that
    /// opens — the browser owns the wait and the bubble.
    pending_tooltip: Option<std::sync::Arc<str>>,
    /// The ancestors that declared themselves hover groups: a box
    /// inside one hangs its states off the GROUP's pointer.
    groups: Vec<u64>,
    /// How deep inside an overlay LAYER the walk is: what a layer
    /// paints is decoration until something in it asks to be a target.
    overlay_depth: usize,
    /// How many drop targets the walk has met — the index into
    /// `FlowEnv::drop_rings`.
    drops_seen: usize,
    overlays: Vec<DomNode>,
    display: crate::layout::DisplayList,
    fields: Vec<(String, bool)>,
    /// A class the current boundary's body declared for its OWN
    /// group element (`boundary_class`), consumed when it closes.
    pending_boundary_class: Option<String>,
    /// The nearest ancestor Frame's declared box — the proposal an
    /// island under it measures against (a flexible island learns its
    /// real box from the browser, in the island round).
    slot: (Option<Px>, Option<Px>),
    customs: Vec<(std::rc::Rc<str>, crate::layout::CustomPlacement)>,
}

/// A flow node with nothing to say yet.
fn node(kind: DomKind) -> DomNode {
    // a reuse marker is a promise, not a built node — the counter
    // tracks real construction
    if !matches!(kind, DomKind::Reuse { .. }) {
        crate::stats::note_capture_node();
    }
    DomNode {
        kind,
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        style: DomStyle::default(),
        layout: Some(DomLayout::default()),
        hints: DomHints::default(),
        children: Vec::new(),
    }
}

fn align_code(align: CrossAlign) -> u8 {
    match align {
        CrossAlign::Start => 0,
        CrossAlign::Center => 1,
        CrossAlign::End => 2,
        CrossAlign::Baseline => 3,
    }
}

impl Walk<'_> {
    fn current_ink(&self) -> Color {
        self.ink.last().copied().unwrap_or(crate::theme::current().fg)
    }

    /// Lowers one semantic node into `out` — most nodes append exactly
    /// one flow node; wrappers pass through and arm the next box.
    fn lower_into(&mut self, tree: &LayoutNode, out: &mut Vec<DomNode>) {
        match tree {
            LayoutNode::Stack { axis, spacing, align, children } => {
                let kind = match axis {
                    Axis::Vertical => DomKind::FlexColumn,
                    Axis::Horizontal => DomKind::FlexRow,
                };
                let mut container = node(kind);
                container.children.reserve_exact(children.len());
                // a container can be the pressed thing too (a bare
                // stack hinted into an anchor): the pending action
                // lands here the way it lands on a styled box
                container.style.interactive = self.pending_interactive.take();
                container.style.tooltip = self.pending_tooltip.take();
                container.style.transition = self.pending_transition.take();
                let layout = container.layout.as_mut().expect("flow node");
                if *spacing != 0.0 {
                    layout.gap = Some(*spacing);
                }
                layout.align = Some(align_code(*align));
                for child in children {
                    let opened = container.children.len();
                    self.lower_into(child, &mut container.children);
                    // the flexible child grows — CSS wants the flag on
                    // the ITEM, so the walk stamps it here
                    if child.is_flexible(*axis, Some(*axis)) {
                        for grown in &mut container.children[opened..] {
                            if let Some(layout) = grown.layout.as_mut() {
                                layout.grow = true;
                            }
                        }
                    }
                    // and the CROSS axis is the stack's own extent:
                    // a child flexible across it takes the line
                    // (align-self beats the container's align-items)
                    let cross = match axis {
                        Axis::Vertical => Axis::Horizontal,
                        Axis::Horizontal => Axis::Vertical,
                    };
                    if child.is_flexible(cross, Some(*axis)) {
                        for stretched in &mut container.children[opened..] {
                            if let Some(layout) = stretched.layout.as_mut() {
                                layout.stretch = true;
                            }
                        }
                    }
                }
                Self::inherit_stretch(&mut container);
                out.push(container);
            }
            LayoutNode::Layered { align, children, .. } => {
                let mut container = node(DomKind::Layers);
                container.layout.as_mut().expect("flow node").align =
                    Some(align_code(*align));
                for child in children {
                    self.lower_into(child, &mut container.children);
                }
                out.push(container);
            }
            LayoutNode::Padding { edges, child } => {
                let Edges { top, leading, bottom, trailing } = *edges;
                let mut lowered = Vec::new();
                self.lower_into(child, &mut lowered);
                // FOLD: a padding around one pure layout node becomes
                // that node's own padding (nested edges sum). Styled
                // boxes never fold — their background must stay tight
                // to the child, and CSS padding would slide under it.
                if let [only] = lowered.as_mut_slice()
                    && only.style == DomStyle::default()
                    && matches!(
                        only.kind,
                        DomKind::FlexColumn | DomKind::FlexRow | DomKind::Layers | DomKind::Box
                    )
                    && let Some(layout) = only.layout.as_mut()
                {
                    let (was_top, was_right, was_bottom, was_left) =
                        layout.padding.unwrap_or((0.0, 0.0, 0.0, 0.0));
                    layout.padding = Some((
                        was_top + top,
                        was_right + trailing,
                        was_bottom + bottom,
                        was_left + leading,
                    ));
                    out.append(&mut lowered);
                    return;
                }
                let mut container = node(DomKind::FlexColumn);
                container.layout.as_mut().expect("flow node").padding =
                    Some((top, trailing, bottom, leading));
                container.children = lowered;
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                out.push(container);
            }
            LayoutNode::Frame { width, height, child } => {
                let outer_slot = self.slot;
                self.slot = (*width, *height);
                let mut container = node(DomKind::FlexColumn);
                {
                    let layout = container.layout.as_mut().expect("flow node");
                    layout.width = *width;
                    layout.height = *height;
                    // a frame CENTERS its child — the cross axis obeys
                    // align, the main one the browser's default; v1
                    // concedes exact centring to the flow (Exact
                    // restores it)
                    layout.align = Some(align_code(CrossAlign::Center));
                }
                self.lower_into(child, &mut container.children);
                self.slot = outer_slot;
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                out.push(container);
            }
            // A hug is a native flow rule on the web: a box that is not told
            // to grow already takes what its content needs, and the cap that
            // rides above it is a `max-height` on the frame outside. So the
            // node passes through and the child lowers as itself.
            LayoutNode::Hug { child, .. } => self.lower_into(child, out),

            LayoutNode::MaxFrame { max_width, max_height, align, child } => {
                let mut container = node(DomKind::FlexColumn);
                {
                    let layout = container.layout.as_mut().expect("flow node");
                    if max_width.is_finite() {
                        layout.max_width = Some(*max_width);
                    } else {
                        layout.grow = true;
                    }
                    if max_height.is_finite() {
                        layout.max_height = Some(*max_height);
                    }
                    layout.align = Some(align_code(*align));
                }
                self.lower_into(child, &mut container.children);
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                out.push(container);
            }
            LayoutNode::Spacer => {
                let mut spacer = node(DomKind::Box);
                spacer.layout.as_mut().expect("flow node").grow = true;
                out.push(spacer);
            }
            LayoutNode::Fill => {
                let mut fill = node(DomKind::Box);
                fill.layout.as_mut().expect("flow node").grow = true;
                fill.style.background = Some(Color::FILL);
                out.push(fill);
            }
            LayoutNode::Leaf { size } => {
                let mut leaf = node(DomKind::Box);
                let layout = leaf.layout.as_mut().expect("flow node");
                layout.width = Some(size.width);
                layout.height = Some(size.height);
                out.push(leaf);
            }
            LayoutNode::Styled { props, child } => {
                let outer_font = self.font;
                let outer_line_height = self.line_height;
                let outer_text_align = self.text_align;
                self.font = props.font.apply_over(self.font);
                self.line_height = props.line_height.or(self.line_height);
                self.text_align = props.text_align.or(self.text_align);

                let states = props.foreground_hovered.is_some()
                    || props.foreground_pressed.is_some();
                let inheriting =
                    !self.ink_scopes.is_empty() && props.foreground.is_some();
                let mut boxed = node(DomKind::Box);
                let interactive = self.pending_interactive.take();
                boxed.style = DomStyle {
                    // a layer that asks for nothing lets the click
                    // reach whatever it covers
                    pass_through: self.overlay_depth > 0 && interactive.is_none(),
                    group: self.groups.last().copied(),
                    tooltip: self.pending_tooltip.take(),
                    interactive,
                    transition: self.pending_transition.take(),
                    ..DomStyle::from_props(props)
                };
                // the tint is the half of the material an ELEMENT owns:
                // it sits under whatever the box paints itself, because
                // an element has one background colour. The tint never
                // rides the wire on its own — it folds in here
                if let Some(glass) = props.glass {
                    let tint = glass.resolve(crate::layout::Rect { origin: crate::layout::Point { x: 0.0, y: 0.0 }, size: crate::layout::Size::default() }).tint;
                    boxed.style.background =
                        crate::dom::GlassFilter::under(tint, boxed.style.background);
                    boxed.style.hover_background = boxed
                        .style
                        .hover_background
                        .and_then(|color| crate::dom::GlassFilter::under(tint, Some(color)));
                    boxed.style.pressed_background = boxed
                        .style
                        .pressed_background
                        .and_then(|color| crate::dom::GlassFilter::under(tint, Some(color)));
                }
                if let Some(color) = props.foreground {
                    self.ink.push(color);
                } else {
                    self.ink.push(self.current_ink());
                }
                if states || inheriting {
                    boxed.style.color = Some(self.current_ink());
                }
                if states {
                    self.ink_scopes.push(self.ink.len());
                }
                self.lower_into(child, &mut boxed.children);
                if states {
                    self.ink_scopes.pop();
                }
                self.ink.pop();
                self.font = outer_font;
                self.line_height = outer_line_height;
                self.text_align = outer_text_align;
                Self::stamp_fill(child, &mut boxed.children);
                Self::inherit_stretch(&mut boxed);
                out.push(boxed);
            }
            LayoutNode::Text { content, highlights, truncation } => {
                let mut text = node(DomKind::Text(DomText {
                    content: content.clone(),
                    color: self.current_ink(),
                    inherits_ink: !self.ink_scopes.is_empty(),
                    font: self.font,
                    line_height: self.line_height,
                    text_align: self.text_align,
                    highlights: highlights
                        .as_ref()
                        .map(|h| (std::rc::Rc::clone(&h.ranges), h.color)),
                    truncation: *truncation,
                }));
                text.style.interactive = self.pending_interactive.take();
                text.style.tooltip = self.pending_tooltip.take();
                out.push(text);
            }
            LayoutNode::Field {
                path,
                content,
                placeholder,
                auto_focus,
                multiline,
                bare,
                // a browser input paints ONE colour: there is no way to
                // ink a range inside it without giving up the native
                // caret, the native selection and the composition. The
                // record is honoured on the pixel path and ignored here.
                highlights: _,
            } => {
                self.fields.push((path.clone(), *auto_focus));
                let theme = crate::theme::current();
                let mut field = node(DomKind::Field(crate::dom::DomField {
                    path: path.clone(),
                    content: content.clone(),
                    placeholder: placeholder.clone(),
                    font: self.font,
                    color: self.current_ink(),
                    multiline: *multiline,
                }));
                field.style = DomStyle {
                    background: (!*bare).then_some(theme.field),
                    border: (!*bare).then_some((theme.field_border, 1.0)),
                    corner_radius: (!*bare)
                        .then_some(crate::layout::Corners::all(crate::layout::FIELD_RADIUS)),
                    focus_border: (!*bare).then_some(theme.focus),
                    placeholder_color: Some(theme.placeholder),
                    ..DomStyle::default()
                };
                out.push(field);
            }
            LayoutNode::Image { source, fit, .. } => {
                match source {
                    Some(source) => {
                        let cover = matches!(
                            fit,
                            Some(motor::views::ContentMode::Fill)
                        );
                        out.push(node(DomKind::Image(crate::dom::DomImage {
                            key: source.key(),
                            cover,
                        })));
                    }
                    // no source yet: an empty box holds the room
                    None => out.push(node(DomKind::Box)),
                }
            }
            LayoutNode::Icon { symbol, forced, .. } => {
                out.push(node(DomKind::Icon(crate::dom::DomIcon {
                    key: symbol.key,
                    symbol: *symbol,
                    color: self.current_ink(),
                    inherits_ink: !self.ink_scopes.is_empty(),
                    forced: *forced,
                })));
            }
            LayoutNode::Scroll { path, target, commanded, child, .. } => {
                // a region the app holds in a binding takes the app's
                // value: here the BROWSER is the clamp and the scroll
                // observer writes back what it settled on, so a value
                // past the end makes one round trip and comes home
                // true. Without a binding the engine's retained offset
                // stands, exactly as it did.
                let offset = commanded.unwrap_or_else(|| {
                    self.env
                        .scroll_offsets
                        .get(path.as_deref().unwrap_or(""))
                        .copied()
                        .unwrap_or_default()
                });
                let mut scroll = node(DomKind::Scroll {
                    path: path.clone(),
                    offset: (offset.x, offset.y),
                    target: target.clone(),
                });
                {
                    let layout = scroll.layout.as_mut().expect("flow node");
                    // the leftover length is the scroller's, and the
                    // offered CROSS size too — a pixel scroller takes
                    // the proposal's width, this one stretches to it
                    layout.grow = true;
                    layout.stretch = true;
                }
                let mut lowered = Vec::new();
                self.lower_into(child, &mut lowered);
                match lowered.as_slice() {
                    // a virtual stack IS the content already — no
                    // second skin, or the rows hide one box too deep
                    [only] if matches!(only.kind, DomKind::Content) => {
                        scroll.children = lowered;
                    }
                    _ => {
                        let mut content = node(DomKind::Content);
                        content.children = lowered;
                        scroll.children.push(content);
                    }
                }
                out.push(scroll);
            }
            LayoutNode::VirtualStack { row_extent, count, children, heights } => {
                // the ONE number the browser cannot give: the app
                // declared every row's extent, so the total and each
                // slot are prefix sums — arithmetic, never measure
                let start_of = |index: usize| -> Px {
                    match heights {
                        // prefix sums over the app-declared extents —
                        // arithmetic, never measure (the window is
                        // small; the total is one pass over the count)
                        Some(rows) => (0..index).map(|row| (rows.0)(row)).sum(),
                        None => *row_extent * index as f64,
                    }
                };
                let total = start_of(*count);
                let mut content = node(DomKind::Content);
                content.layout.as_mut().expect("flow node").height = Some(total);
                for (index, child) in children {
                    let opened = content.children.len();
                    self.lower_into(child, &mut content.children);
                    for row in &mut content.children[opened..] {
                        if let Some(layout) = row.layout.as_mut() {
                            layout.slot_y = Some(start_of(*index));
                        }
                    }
                }
                out.push(content);
            }
            LayoutNode::Split { axis, at, children, .. } => {
                let kind = match axis {
                    Axis::Horizontal => DomKind::FlexRow,
                    Axis::Vertical => DomKind::FlexColumn,
                };
                let mut container = node(kind);
                if let [a, b] = children.as_slice() {
                    let opened = container.children.len();
                    self.lower_into(a, &mut container.children);
                    for lane in &mut container.children[opened..] {
                        if let Some(layout) = lane.layout.as_mut() {
                            match axis {
                                Axis::Horizontal => layout.width = Some(*at),
                                Axis::Vertical => layout.height = Some(*at),
                            }
                        }
                    }
                    let opened = container.children.len();
                    self.lower_into(b, &mut container.children);
                    for lane in &mut container.children[opened..] {
                        if let Some(layout) = lane.layout.as_mut() {
                            layout.grow = true;
                        }
                    }
                }
                out.push(container);
            }
            // In this mode the BROWSER lays out, so there is no
            // resolved size here to hand back: the probe lowers as its
            // child and stays quiet. Reporting a box from this side
            // would mean observing the element, which is a road of its
            // own — and a number invented here would be worse than no
            // number at all.
            LayoutNode::Measured { child, .. } => self.lower_into(child, out),

            LayoutNode::Boundary { path, children } => {
                // a CLEAN boundary is a promise, not a walk: no body
                // under it ran, the retained group still holds, and
                // the diff keeps it wholesale — O(change), by absence
                if self.env.retained_groups.contains(path.as_str())
                    && !self.env.changed.iter().any(|run| {
                        // related in EITHER direction dirties: a run
                        // below me changed my interior; a run above me
                        // re-rendered me inline (inline renders never
                        // reach the body-run ledger on their own)
                        let related = |a: &str, b: &str| {
                            a == b
                                || (a.len() > b.len()
                                    && a.as_bytes().starts_with(b.as_bytes())
                                    && a.as_bytes()[b.len()] == b'/')
                        };
                        related(run, path) || related(path, run)
                    })
                {
                    out.push(node(DomKind::Reuse { path: std::rc::Rc::from(path.as_str()) }));
                    return;
                }
                let mut group = node(DomKind::Group { path: std::rc::Rc::from(path.as_str()) });
                group.children.reserve_exact(children.len());
                let outer_pending = self.pending_boundary_class.take();
                for child in children {
                    let opened = group.children.len();
                    self.lower_into(child, &mut group.children);
                    Self::stamp_fill(child, &mut group.children[opened..]);
                }
                Self::inherit_stretch(&mut group);
                if let Some(class) = self.pending_boundary_class.take() {
                    // the body spoke about its own element: an empty
                    // class clears, anything else attributes
                    group.hints.class =
                        (!class.is_empty()).then(|| std::rc::Rc::from(class.as_str()));
                }
                self.pending_boundary_class = outer_pending;
                out.push(group);
            }
            LayoutNode::BoundaryRef { path } => {
                // resolves through the retention IN PLACE, the same
                // door the placement walk uses — a missing entry keeps
                // the identity anchor so the diff can match later
                let lowered = crate::reconciler::with_retained_layout(path, |tree| {
                    tree.map(|tree| {
                        let mut nodes = Vec::new();
                        self.lower_into(tree, &mut nodes);
                        nodes
                    })
                });
                match lowered {
                    Some(nodes) => out.extend(nodes),
                    None => out.push(node(DomKind::Group { path: std::rc::Rc::from(path.as_str()) })),
                }
            }
            LayoutNode::Interactive { path, child } => {
                self.pending_interactive = Some(std::rc::Rc::from(path.as_str()));
                self.lower_into(child, out);
                self.pending_interactive = None;
            }
            LayoutNode::Animated { spec, child, .. } => {
                self.pending_transition = Some((spec.response, spec.damping));
                self.lower_into(child, out);
                self.pending_transition = None;
            }
            LayoutNode::BoundaryHint { class } => {
                self.pending_boundary_class = Some(class.clone().unwrap_or_default());
            }
            LayoutNode::Tooltip { text, child, .. } => {
                // the bubble is a data attribute and one static CSS
                // rule: the browser owns the wait, so a tooltip costs
                // no patch and no clock
                self.pending_tooltip = Some(text.clone());
                self.lower_into(child, out);
                self.pending_tooltip = None;
            }
            LayoutNode::HoverGroup { path, child } => {
                let key = crate::layout::group_key(path);
                let opened = out.len();
                self.groups.push(key);
                self.lower_into(child, out);
                self.groups.pop();
                // the owner names itself; the followers below point
                // their selectors at it
                for owner in &mut out[opened..] {
                    owner.style.group_owner = Some(key);
                }
            }
            LayoutNode::Overlay { behind, layer, child, .. } => {
                // one grid cell, both in it — the browser stacks them
                // in document order
                let mut cell = node(DomKind::Layers);
                self.overlay_depth += 1;
                let mut over = Vec::new();
                self.lower_into(layer, &mut over);
                self.overlay_depth -= 1;
                let mut under = Vec::new();
                self.lower_into(child, &mut under);
                if *behind {
                    cell.children.extend(over);
                    cell.children.extend(under);
                } else {
                    cell.children.extend(under);
                    cell.children.extend(over);
                }
                out.push(cell);
            }
            LayoutNode::Live { child, .. } => {
                // the clock belongs to the pixel modes; here the
                // browser animates from the transition specs
                self.lower_into(child, out);
            }
            LayoutNode::ControlRegion { child, .. } => {
                // a window control means nothing in a browser tab
                self.lower_into(child, out);
            }
            LayoutNode::ContextSource { child, .. } => {
                // the runtime opens the menu off the right press; the
                // element tree carries nothing for it
                self.lower_into(child, out);
            }
            LayoutNode::DragSource { child, .. } => {
                self.lower_into(child, out);
            }
            LayoutNode::DropTarget { child, .. } => {
                let ringed =
                    self.env.drop_rings.get(self.drops_seen).copied().unwrap_or(false);
                self.drops_seen += 1;
                self.lower_into(child, out);
                // element mode never reads the draw list, so the ring
                // must be an ELEMENT here — a box with a border, born
                // with the drag and dying with it, and the LATER
                // sibling so it covers what it rings
                if ringed {
                    let accent = crate::theme::current().accent;
                    let mut ring = node(DomKind::Box);
                    ring.style = DomStyle {
                        border: Some((accent, 2.0)),
                        corner_radius: Some(crate::layout::Corners::all(6.0)),
                        pass_through: true,
                        ..DomStyle::default()
                    };
                    out.push(ring);
                }
            }
            LayoutNode::DragRegion { child } => {
                // a window drag region means nothing in a browser tab
                self.lower_into(child, out);
            }
            LayoutNode::Hinted { tag, class, dom_id, child } => {
                // the hint stamps whatever the child lowered to — one
                // node in practice (a hinted stack, text, or box)
                let opened = out.len();
                self.lower_into(child, out);
                for hinted in &mut out[opened..] {
                    if tag.is_some() {
                        hinted.hints.tag = tag.clone();
                    }
                    if class.is_some() {
                        hinted.hints.class = class.clone();
                    }
                    if dom_id.is_some() {
                        hinted.hints.dom_id = dom_id.clone();
                    }
                }
            }
            #[cfg(feature = "canvas")]
            LayoutNode::ExactLayout { child } => {
                out.push(self.exact(child));
            }
            #[cfg(not(feature = "canvas"))]
            LayoutNode::ExactLayout { .. } => {
                out.push(node(DomKind::Box));
            }
            #[cfg(feature = "canvas")]
            LayoutNode::Island { path, child } => {
                out.push(self.island(child, path.as_deref()));
            }
            #[cfg(feature = "canvas")]
            LayoutNode::Custom { path, .. } => {
                out.push(self.island(tree, Some(path.as_str())));
            }
            // without the canvas feature the claiming APIs are gone,
            // so these arms are unreachable by construction — an
            // empty box keeps the match total
            #[cfg(not(feature = "canvas"))]
            LayoutNode::Island { .. } | LayoutNode::Custom { .. } => {
                out.push(node(DomKind::Box));
            }
            // the native host's web lowering (`docs/webview.md`): the
            // "native view" is an iframe, and the island contract is
            // the one the DOM already enforces
            LayoutNode::Host { spec, .. } => {
                let crate::host::HostSpec::Webview { url, .. } = spec;
                let mut frame = node(DomKind::Iframe { src: std::rc::Rc::clone(url) });
                let layout = frame.layout.as_mut().expect("flow node");
                // a page is a filler on both axes by construction — it
                // grows along the stack and stretches across it; a
                // `.frame(…)` above pins it like anything else
                layout.grow = true;
                layout.stretch = true;
                out.push(frame);
            }
            LayoutNode::Anchored { path, side, overlay, child } => {
                // the anchor gets an IDENTITY the glue can find: a
                // group wrapped around the child, keyed off the
                // popover's own path
                let anchor_path = format!("{path}/#anchor");
                let mut anchor = node(DomKind::Group { path: std::rc::Rc::from(anchor_path.as_str()) });
                self.lower_into(child, &mut anchor.children);
                out.push(anchor);

                let side = match side {
                    crate::layout::Side::Top => 0u8,
                    crate::layout::Side::Bottom => 1,
                    crate::layout::Side::Leading => 2,
                    crate::layout::Side::Trailing => 3,
                };
                let mut popover = node(DomKind::Popover {
                    path: path.clone(),
                    anchor: anchor_path,
                    side,
                });
                // the CARD lowers under the portal — the overlay is a
                // whole subtree, not a marker
                self.lower_into(overlay, &mut popover.children);
                self.overlays.push(popover);
            }
        }
    }

    /// The engine PROPOSES a wrapper's box to its interior; a block
    /// element proposes nothing. Every wrapper is a column instead,
    /// and a vertically flexible interior takes the offer through
    /// `flex: 1 1 auto` — full when the box is definite, content-
    /// sized when it is not (a zero basis would collapse it).
    fn stamp_fill(child: &LayoutNode, lowered: &mut [DomNode]) {
        if child.is_flexible(Axis::Vertical, None) {
            for node in lowered {
                if let Some(layout) = node.layout.as_mut() {
                    layout.fill = true;
                }
            }
        }
    }

    /// A hungry interior keeps its hunger through a pure wrapper —
    /// the stretch has to reach the flex line that can actually feed
    /// it, or the wrapper sizes to its content and starves the child.
    fn inherit_stretch(container: &mut DomNode) {
        if container
            .children
            .iter()
            .any(|child| child.layout.as_ref().is_some_and(|layout| layout.stretch))
        {
            if let Some(layout) = container.layout.as_mut() {
                layout.stretch = true;
            }
        }
    }
}

#[cfg(feature = "canvas")]
impl Walk<'_> {
    /// `.layout(Exact)`: the subtree keeps the ENGINE's numbers. It
    /// measures at its slot, places with the ABSOLUTE capture — the
    /// machinery the whole mode once ran on — and splices the result
    /// under a relative box the flow sizes and carries. Pixel parity
    /// with the canvas, by construction: same measure, same place.
    fn exact(&mut self, subtree: &LayoutNode) -> DomNode {
        let Some(env) = self.env.layout else {
            return node(DomKind::Box);
        };
        let proposal = crate::layout::Proposal {
            width: self.slot.0,
            height: self.slot.1,
        };
        let (size, fit) = subtree.measure(proposal, env);
        let mut placement = crate::layout::Placement::with_capture(size, self.current_ink());
        subtree.place(
            crate::layout::Rect { origin: Point::default(), size },
            fit,
            env,
            &mut placement,
        );
        // the interior arrives ABSOLUTE (geometry on every node); the
        // wrapper is a Content box — position:relative by creation —
        // sized by our answer, carried by the flow like any other box
        let captured = placement.take_capture().finish();
        let mut wrapper = node(DomKind::Content);
        {
            let layout = wrapper.layout.as_mut().expect("flow node");
            layout.width = Some(size.width);
            layout.height = Some(size.height);
        }
        wrapper.children = captured.children;
        self.display.extend(placement.display);
        wrapper
    }

    /// A canvas island: the engine measures and paints ITS OWN pixels,

    /// locally — the subtree places at its own origin, so the commands
    /// are island-local by construction and the element gets a fixed
    /// box the browser never argues with.
    fn island(&mut self, subtree: &LayoutNode, path: Option<&str>) -> DomNode {
        let path: Option<std::rc::Rc<str>> = path.map(std::rc::Rc::from);
        let Some(env) = self.env.layout else {
            return node(DomKind::Canvas { origin: (0.0, 0.0), display: (0, 0), path });
        };
        // a flexible axis belongs to the browser: once it reported a
        // box, the island measures against THAT — the pixels and the
        // element agree after one round trip
        let reported = path
            .as_deref()
            .and_then(|island| self.env.island_boxes.get(island))
            .copied();
        let flexible = (
            subtree.is_flexible(Axis::Horizontal, None),
            subtree.is_flexible(Axis::Vertical, None),
        );
        // the reported box IS the container's offer — a pixel stack
        // proposes its extent to every child, flexible or not, and
        // the child answers with what it wants
        let proposal = crate::layout::Proposal {
            width: reported.map(|(w, _)| w).or(self.slot.0),
            height: reported.map(|(_, h)| h).or(self.slot.1),
        };
        let (measured, fit) = subtree.measure(proposal, env);
        // which axes FOLLOW the proposal? offer a different box and
        // watch what moves — a moved axis belongs to the browser:
        // `align-self: stretch`, no pinned size, and every resize
        // comes back through the observer
        let shifted = crate::layout::Proposal {
            width: Some(proposal.width.unwrap_or(measured.width) + 97.0),
            height: Some(proposal.height.unwrap_or(measured.height) + 97.0),
        };
        let (moved, _) = subtree.measure(shifted, env);
        let hungry = (
            (moved.width - measured.width).abs() > 0.5,
            (moved.height - measured.height).abs() > 0.5,
        );
        // a flexible subtree measures NATURAL even against an exact
        // proposal — granting the slack is its container's job. The
        // browser is that container here: the reported box wins.
        // a MEASURED island covers what is visible, never the whole
        // box: a box that declared four thousand points of content
        // would otherwise mint a canvas that tall, and the paint inside
        // it is one screen anyway. A reported box is the browser's own
        // answer and needs no clamp.
        let size = crate::layout::Size {
            width: match (flexible.0, reported) {
                (true, Some((w, _))) => w,
                _ => measured.width.min(self.env.size.0),
            },
            height: match (flexible.1, reported) {
                (true, Some((_, h))) => h,
                _ => measured.height.min(self.env.size.1),
            },
        };
        let start = self.display.len();
        let mut placement = crate::layout::Placement::with_ink(self.current_ink());
        subtree.place(
            crate::layout::Rect { origin: Point::default(), size },
            fit,
            env,
            &mut placement,
        );
        self.display.extend(placement.display);
        // the boxes inside this island keep their LOCAL frames — the
        // pointer door routes the browser's canvas coordinates by them
        if let Some(island) = &path {
            for custom in placement.customs {
                self.customs.push((std::rc::Rc::clone(island), custom));
            }
        }
        let mut island = node(DomKind::Canvas {
            origin: (0.0, 0.0),
            display: (start, self.display.len()),
            path,
        });
        {
            let layout = island.layout.as_mut().expect("flow node");
            layout.width = (!hungry.0).then_some(size.width);
            layout.height = (!hungry.1).then_some(size.height);
            layout.stretch = hungry.0 || hungry.1;
        }
        // the node's own box feeds the raster — the flow diff never
        // reads it, but the island ledger sizes the pixels by it
        island.width = size.width;
        island.height = size.height;
        island
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn env_fixture(offsets: &HashMap<String, Point>) -> FlowEnv<'_> {
        FlowEnv {
            scroll_offsets: offsets,
            size: (400.0, 300.0),
            layout: None,
            changed: &[],
            // no drag in a fixture: nothing is ringed
            drop_rings: &[],
            retained_groups: {
                thread_local! {
                    static EMPTY: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                }
                // tests never reuse: an empty retained set
                Box::leak(Box::new(std::collections::HashSet::new()))
            },
            island_boxes: Box::leak(Box::new(HashMap::default())),
        }
    }

    fn text_node(content: &str) -> LayoutNode {
        LayoutNode::Text {
            content: Arc::from(content),
            highlights: None,
            truncation: None,
        }
    }

    /// The native host lowers to the browser's own island: an iframe
    /// carrying its url, hungry on both axes — a page has no natural
    /// size, so the row it sits in hands it the leftover.
    #[test]
    fn a_host_lowers_to_an_iframe_that_fills() {
        let tree = LayoutNode::Stack {
            axis: Axis::Horizontal,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![
                text_node("side"),
                LayoutNode::Host {
                    path: "pane".into(),
                    spec: crate::host::HostSpec::Webview {
                        url: "https://example.test/docs".into(),
                        scripts: Vec::new().into(),
                        console: false,
                        requests: false,
                    },
                },
            ],
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let row = &scene.children[0];
        let pane = &row.children[1];
        assert!(
            matches!(&pane.kind, DomKind::Iframe { src } if &**src == "https://example.test/docs"),
            "the host is an iframe with its url: {:?}",
            pane.kind
        );
        let layout = pane.layout.as_ref().expect("flow");
        assert!(layout.grow, "the page takes the leftover");
        assert!(layout.stretch, "and follows the cross axis");
    }

    /// The dream mapping: a vstack with spacing IS a flex column with
    /// a gap, and the spacer inside it grows.
    #[test]
    fn a_stack_lowers_to_flex_with_gap_and_grow() {
        let tree = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 8.0,
            align: CrossAlign::Start,
            children: vec![text_node("head"), LayoutNode::Spacer, text_node("foot")],
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let column = &scene.children[0];
        assert!(matches!(column.kind, DomKind::FlexColumn));
        let layout = column.layout.as_ref().expect("flow");
        assert_eq!(layout.gap, Some(8.0));
        assert_eq!(layout.align, Some(0));
        assert_eq!(column.children.len(), 3);
        assert!(
            column.children[1].layout.as_ref().expect("flow").grow,
            "the spacer grows"
        );
        assert!(!column.children[0].layout.as_ref().expect("flow").grow);
    }

    /// No geometry anywhere: a flow scene's nodes all carry layout
    /// records and zeroed coordinates.
    #[test]
    fn a_flow_scene_carries_no_coordinates() {
        let tree = LayoutNode::Stack {
            axis: Axis::Horizontal,
            spacing: 4.0,
            align: CrossAlign::Center,
            children: vec![
                text_node("a"),
                LayoutNode::Padding {
                    edges: Edges {
                        top: 2.0,
                        leading: 4.0,
                        bottom: 2.0,
                        trailing: 4.0,
                    },
                    child: Box::new(text_node("b")),
                },
            ],
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        fn walk(node: &DomNode) {
            assert!(node.layout.is_some(), "every flow node speaks records");
            assert_eq!((node.x, node.y, node.width, node.height).1, 0.0);
            for child in &node.children {
                walk(child);
            }
        }
        for child in &scene.children {
            walk(child);
        }
        let row = &scene.children[0];
        let padded = &row.children[1];
        assert_eq!(
            padded.layout.as_ref().expect("flow").padding,
            Some((2.0, 4.0, 2.0, 4.0)),
            "trailing rides right, leading rides left"
        );
    }

    /// Virtual rows sit at their prefix-sum slots inside a content box
    /// sized to the WHOLE extent — the scrollbar stays honest and no
    /// survivor ever moves.
    #[test]
    fn virtual_rows_take_their_slots() {
        let tree = LayoutNode::VirtualStack {
            row_extent: 22.0,
            count: 1000,
            children: vec![
                (3, text_node("row 3")),
                (4, text_node("row 4")),
            ],
            heights: None,
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let content = &scene.children[0];
        assert!(matches!(content.kind, DomKind::Content));
        assert_eq!(
            content.layout.as_ref().expect("flow").height,
            Some(22.0 * 1000.0)
        );
        let slots: Vec<_> = content
            .children
            .iter()
            .map(|row| row.layout.as_ref().expect("flow").slot_y)
            .collect();
        assert_eq!(slots, vec![Some(66.0), Some(88.0)]);
    }

    /// The ink rules ride the walk exactly as they ride the capture:
    /// under a hover ink the text inherits instead of painting.
    #[test]
    fn text_under_a_hover_ink_inherits() {
        use crate::layout::VisualProps;
        let mut props = VisualProps::default();
        props.foreground = Some(Color::hex(0x888888));
        props.foreground_hovered = Some(Color::hex(0xFFFFFF));
        let tree = LayoutNode::Styled {
            props: Box::new(props),
            child: Box::new(text_node("flip me")),
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let boxed = &scene.children[0];
        assert_eq!(boxed.style.color, Some(Color::hex(0x888888)));
        let DomKind::Text(text) = &boxed.children[0].kind else {
            panic!("a text under the box");
        };
        assert!(text.inherits_ink, "the box owns both states");
    }

    /// A popover mounts under the root — the portal survives the flow.
    #[test]
    fn a_popover_lands_under_the_root() {
        let tree = LayoutNode::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: CrossAlign::Start,
            children: vec![LayoutNode::Anchored {
                path: "app/[row]".into(),
                side: crate::layout::Side::Bottom,
                overlay: std::rc::Rc::new(text_node("the card")),
                child: Box::new(text_node("the row")),
            }],
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let last = scene.children.last().expect("the portal");
        assert!(matches!(
            &last.kind,
            DomKind::Popover { path, anchor, side: 1 }
                if path == "app/[row]" && anchor == "app/[row]/#anchor"
        ));
        assert!(!last.children.is_empty(), "the card lowered under the portal");
    }
}
