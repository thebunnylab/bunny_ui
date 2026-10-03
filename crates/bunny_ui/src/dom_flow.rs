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
//! the app and turned into prefix sums here, or kept as starts by a
//! list whose rows measure themselves — pure arithmetic), a canvas
//! island's size (the engine measures its own pixels), and one day a
//! `.layout(Exact)` interior. Everything else speaks records.

use motor::hash::FxHashMap as HashMap;

use crate::dom::{DomHints, DomKind, DomLayout, DomLook, DomMarks, DomNode, DomStyle, DomText};
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
    /// The Groups the retained scene actually holds, each with the
    /// environment it was lowered in — a promise the diff cannot match
    /// would mount a hole, so the walk checks first; and a boundary
    /// under a body that ran is reused only when that environment held.
    pub retained_groups: &'a HashMap<std::rc::Rc<str>, GroupRecord>,
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
    /// The fields on stage, and when each asks for the keyboard.
    pub fields: Vec<(String, crate::layout::AutoFocus)>,
    /// The app's boxes inside each island, with ISLAND-LOCAL frames —
    /// exactly the coordinates the browser reports on the canvas.
    pub customs: Vec<(std::rc::Rc<str>, crate::layout::CustomPlacement)>,
    /// The targets drawn inside each island — the island's identity,
    /// the target's path, its frame in the island's own coordinates.
    /// The pointer door routes a canvas click by them.
    pub hits: Vec<(std::rc::Rc<str>, String, crate::layout::Rect)>,
    /// The islands this walk lowered. An island a reuse promise kept
    /// is not among them, and its hits from the last walk stand.
    pub islands_walked: Vec<std::rc::Rc<str>>,
    /// The groups this walk lowered, with what they were lowered in.
    pub groups: Vec<(std::rc::Rc<str>, GroupRecord)>,
}

/// What a boundary inherits from the walk above it — everything a
/// group's own lowering reads from the walk and nothing it owns. A
/// retained group whose key still holds lowers to the same scene, so a
/// walk that meets it under a body that ran may still reuse it: the run
/// above changed nothing the group can see.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FlowKey {
    ink: Color,
    in_ink_scope: bool,
    font: FontSpec,
    line_height: Option<Px>,
    text_align: Option<motor::views::TextAlignment>,
    interactive: Option<std::rc::Rc<str>>,
    transition: Option<(f64, f64)>,
    tooltip: Option<std::sync::Arc<str>>,
    group: Option<u64>,
    in_overlay: bool,
    slot: (Option<Px>, Option<Px>),
}

/// What the lowering keeps of a group: the key it was lowered under,
/// and what the group's OWN lowering gave its element — the parent
/// stamps the rest again when the group is reused.
#[derive(Clone, Debug)]
pub(crate) struct GroupRecord {
    pub env: FlowKey,
    /// `stretch` the group took from a hungry child of its own.
    pub own_stretch: bool,
    /// The class its body declared for it (`boundary_class`).
    pub own_class: Option<std::rc::Rc<str>>,
    /// The binding that class reads through, when it reads for itself.
    pub class_binding: Option<std::rc::Rc<crate::bind::Bound<String>>>,
    /// Drop targets inside the group: a reuse must still count them.
    pub drops: usize,
}

/// Lowers the semantic tree to a flow scene. The root is the mount
/// point (id 0): the theme's canvas, the window's box.
pub(crate) fn lower(root: &LayoutNode, env: &FlowEnv) -> FlowOutput {
    let mut walk = Walk {
        env,
        changed: ChangedIndex::new(env.changed),
        ink: Vec::new(),
        ink_scopes: Vec::new(),
        font: FontSpec::DEFAULT,
        declared: FontSpec::DEFAULT,
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
        // every run lowers its own group, and the few groups above the
        // runs ride on the margin: a thousand rows mounting fill the list
        // without doubling it ten times. No run, no group, no list
        groups_out: Vec::with_capacity(match env.changed.len() {
            0 => 0,
            runs => runs + 4,
        }),
        hits: Vec::new(),
        islands_walked: Vec::new(),
        runs_below: true,
        last_face: None,
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
        face: Some(walk.face_record(FontSpec::DEFAULT)),
        x: 0.0,
        y: 0.0,
        width: env.size.0,
        height: env.size.1,
        style: DomStyle::of_look(DomLook {
            background: Some(crate::theme::current().canvas),
            ..DomLook::default()
        }),
        // the root is the one ABSOLUTE citizen of a flow scene: the
        // window's box is real geometry, and a resize is its SetSize
        layout: None,
        hints: DomHints::default(),
        children,
        binding: None,
    };
    FlowOutput {
        scene,
        display: walk.display,
        fields: walk.fields,
        customs: walk.customs,
        groups: walk.groups_out,
        hits: walk.hits,
        islands_walked: walk.islands_walked,
    }
}

/// The bodies that ran this frame, indexed for the one question a
/// boundary asks: did a run touch me — at my path, under it, or above
/// it? Three lookups per level answer it, whatever the number of runs;
/// the list of a thousand rows that each ran used to be scanned once
/// per boundary the walk met.
struct ChangedIndex<'a> {
    /// The runs themselves.
    exact: motor::hash::FxHashSet<&'a str>,
    /// Every boundary with a run somewhere under it.
    above_a_run: motor::hash::FxHashSet<&'a str>,
}

impl<'a> ChangedIndex<'a> {
    fn new(changed: &'a [String]) -> Self {
        // a run is one entry, and mostly one boundary above it of its own
        // (a list's row under its identity): both sets sized once
        let mut exact = motor::hash::FxHashSet::default();
        exact.reserve(changed.len());
        let mut above_a_run = motor::hash::FxHashSet::default();
        above_a_run.reserve(changed.len());
        for run in changed {
            exact.insert(run.as_str());
            for (at, _) in run.match_indices('/') {
                above_a_run.insert(&run[..at]);
            }
        }
        ChangedIndex { exact, above_a_run }
    }

    /// A run at the boundary or below it changed its interior.
    fn touches(&self, path: &str) -> bool {
        self.exact.contains(path) || self.above_a_run.contains(path)
    }

    /// A run above the boundary re-rendered it inline. For a component
    /// boundary that says nothing — its body is its own, and the walk
    /// compares the environment it was lowered in ([`FlowKey`]) — but
    /// an identity scope with no body of its own (a list's row) is the
    /// run's content, and moves with it.
    fn run_above(&self, path: &str) -> bool {
        path.match_indices('/').any(|(at, _)| self.exact.contains(&path[..at]))
    }
}

struct Walk<'a> {
    env: &'a FlowEnv<'a>,
    changed: ChangedIndex<'a>,
    /// The inherited ink — the top colors the text (the capture's
    /// exact rule set rides here unchanged).
    ink: Vec<Color>,
    /// Depths where a hover/pressed ink opened: inside one, text
    /// inherits instead of painting its own color.
    ink_scopes: Vec<usize>,
    font: FontSpec,
    /// The face declared by the nearest element above that declares
    /// one — the root's default until a box changes it. A text with
    /// this face inherits it.
    declared: FontSpec,
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
    fields: Vec<(String, crate::layout::AutoFocus)>,
    /// A class the current boundary's body declared for its OWN
    /// group element (`boundary_class`), consumed when it closes — with
    /// the binding it reads through, when it reads for itself.
    pending_boundary_class: Option<(String, Option<std::rc::Rc<crate::bind::Bound<String>>>)>,
    /// The nearest ancestor Frame's declared box — the proposal an
    /// island under it measures against (a flexible island learns its
    /// real box from the browser, in the island round).
    slot: (Option<Px>, Option<Px>),
    customs: Vec<(std::rc::Rc<str>, crate::layout::CustomPlacement)>,
    /// The targets inside each island, canvas-local — see `FlowOutput::hits`.
    hits: Vec<(std::rc::Rc<str>, String, crate::layout::Rect)>,
    /// The islands lowered this walk — see `FlowOutput::islands_walked`.
    islands_walked: Vec<std::rc::Rc<str>>,
    /// The groups lowered this walk, for the lowering's records.
    groups_out: Vec<(std::rc::Rc<str>, GroupRecord)>,
    /// May a run sit under the boundary being lowered? False once a
    /// boundary with nothing run below it opens: a thousand kept rows
    /// under a list that ran alone ask the index nothing.
    runs_below: bool,
    /// The face this walk declared last, shared by the boxes that
    /// declare it again — the rows of a list declare one face as many
    /// times as there are rows.
    last_face: Option<std::rc::Rc<FontSpec>>,
}

thread_local! {
    /// The root's face, and every box's that declares the default one:
    /// one record for the thread, never built again.
    static DEFAULT_FACE: std::rc::Rc<FontSpec> = std::rc::Rc::new(FontSpec::DEFAULT);
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
        binding: None,
        face: None,
    }
}

/// The tags whose browser display is inline: a stack wearing one of
/// these around a single child folds to the tag's own display.
const INLINE_TAGS: &[&str] = &[
    "a", "span", "b", "i", "em", "strong", "small", "label", "code", "u", "s", "mark", "abbr",
    "sub", "sup", "q", "cite", "kbd", "var", "time",
];

fn align_code(align: CrossAlign) -> u8 {
    match align {
        CrossAlign::Start => 0,
        CrossAlign::Center => 1,
        CrossAlign::End => 2,
        CrossAlign::Baseline => 3,
    }
}

impl Walk<'_> {
    /// The environment a boundary met here would be lowered in.
    fn flow_key(&self) -> FlowKey {
        FlowKey {
            ink: self.current_ink(),
            in_ink_scope: !self.ink_scopes.is_empty(),
            font: self.font,
            line_height: self.line_height,
            text_align: self.text_align,
            interactive: self.pending_interactive.clone(),
            transition: self.pending_transition,
            tooltip: self.pending_tooltip.clone(),
            group: self.groups.last().copied(),
            in_overlay: self.overlay_depth > 0,
            slot: self.slot,
        }
    }

    fn current_ink(&self) -> Color {
        // the theme is read only when no ink is open: every boundary
        // asks, and the whole theme is copied out to answer
        self.ink.last().copied().unwrap_or_else(|| crate::theme::current().fg)
    }

    /// A shared record of this face: the thread's for the default
    /// face, the walk's last one when it is the same face again.
    fn face_record(&mut self, face: FontSpec) -> std::rc::Rc<FontSpec> {
        if face == FontSpec::DEFAULT {
            return DEFAULT_FACE.with(std::rc::Rc::clone);
        }
        match &self.last_face {
            Some(record) if **record == face => std::rc::Rc::clone(record),
            _ => {
                let record = std::rc::Rc::new(face);
                self.last_face = Some(std::rc::Rc::clone(&record));
                record
            }
        }
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
                container.style.set_tooltip(self.pending_tooltip.take());
                container.style.set_transition(self.pending_transition.take());
                let layout = container.layout.as_mut().expect("flow node");
                if *spacing != 0.0 {
                    layout.gap = Some(*spacing as f32);
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
                Self::fold_table_wrapper(&mut container);
                out.push(container);
            }
            // a row that wraps is the browser's own: a flex row that
            // wraps its items, with the gaps in both directions — the
            // lines break where the items' own widths say, as here
            LayoutNode::Flow { spacing, line_spacing, align, children } => {
                let mut container = node(DomKind::FlexRow);
                container.style.interactive = self.pending_interactive.take();
                container.style.set_tooltip(self.pending_tooltip.take());
                container.style.set_transition(self.pending_transition.take());
                let layout = container.layout.as_mut().expect("flow node");
                if *spacing != 0.0 {
                    layout.gap = Some(*spacing as f32);
                }
                layout.align = Some(align_code(*align));
                layout.wrap = Some(*line_spacing as f32);
                for child in children {
                    self.lower_into(child, &mut container.children);
                }
                out.push(container);
            }
            // In flow mode there is no native child view to cross and
            // no second surface to present on: the sheet is a layer over
            // the page, which is what it looks like anyway.
            LayoutNode::Sheet { content, child, .. } => {
                let mut container = node(DomKind::Layers);
                container.layout.as_mut().expect("flow node").align =
                    Some(align_code(crate::layout::CrossAlign::Center));
                self.lower_into(child, &mut container.children);
                self.lower_into(content, &mut container.children);
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
                    && only.style.is_default()
                    && matches!(
                        only.kind,
                        DomKind::FlexColumn | DomKind::FlexRow | DomKind::Layers | DomKind::Box
                    )
                    && let Some(layout) = only.layout.as_mut()
                {
                    let (was_top, was_right, was_bottom, was_left) =
                        layout.padding.unwrap_or((0.0, 0.0, 0.0, 0.0));
                    let sum = |was: f32, more: Px| (f64::from(was) + more) as f32;
                    layout.padding = Some((
                        sum(was_top, top),
                        sum(was_right, trailing),
                        sum(was_bottom, bottom),
                        sum(was_left, leading),
                    ));
                    out.append(&mut lowered);
                    return;
                }
                let mut container = node(DomKind::FlexColumn);
                container.layout.as_mut().expect("flow node").padding =
                    Some((top as f32, trailing as f32, bottom as f32, leading as f32));
                container.children = lowered;
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                out.push(container);
            }
            LayoutNode::Frame { width, height, align, child } => {
                let outer_slot = self.slot;
                self.slot = (*width, *height);
                let mut container = node(DomKind::FlexColumn);
                {
                    let layout = container.layout.as_mut().expect("flow node");
                    layout.width = width.map(|width| width as f32);
                    layout.height = height.map(|height| height as f32);
                    // a frame places its child on the edge it was given —
                    // the cross axis obeys align, the main one the
                    // browser's default; v1 concedes exact centring to
                    // the flow (Exact restores it)
                    layout.align = Some(align_code(*align));
                }
                self.lower_into(child, &mut container.children);
                self.slot = outer_slot;
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                Self::fold_table_wrapper(&mut container);
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
                        layout.max_width = Some(*max_width as f32);
                    } else {
                        layout.grow = true;
                    }
                    if max_height.is_finite() {
                        layout.max_height = Some(*max_height as f32);
                    }
                    layout.align = Some(align_code(*align));
                }
                self.lower_into(child, &mut container.children);
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                Self::fold_table_wrapper(&mut container);
                out.push(container);
            }
            // The flex frame on the web flow: a growing box. Its FLOOR is not
            // honoured here yet — the flow layout carries no minimum, so a
            // pane narrower than a table's floors squeezes the lanes on the
            // web where the desktop scrolls them. Recorded, not hidden.
            LayoutNode::FlexFrame { align, child, .. } => {
                let mut container = node(DomKind::FlexColumn);
                {
                    let layout = container.layout.as_mut().expect("flow node");
                    layout.grow = true;
                    layout.align = Some(align_code(*align));
                }
                self.lower_into(child, &mut container.children);
                Self::stamp_fill(child, &mut container.children);
                Self::inherit_stretch(&mut container);
                Self::fold_table_wrapper(&mut container);
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
                fill.style.look_mut().background = Some(Color::FILL);
                out.push(fill);
            }
            LayoutNode::Leaf { size } => {
                let mut leaf = node(DomKind::Box);
                let layout = leaf.layout.as_mut().expect("flow node");
                layout.width = Some(size.width as f32);
                layout.height = Some(size.height as f32);
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
                // an ink-only style over a text owns no element: the text
                // takes the ink itself, as it always did, and the box
                // that would have carried nothing is not made. A state
                // the ink answers to, an ink inside a hover scope, a
                // layer, a transition — those keep their box
                if matches!(**child, LayoutNode::Text { .. })
                    && !states
                    && !inheriting
                    && self.overlay_depth == 0
                    && self.pending_transition.is_none()
                    && !DomLook::paints(props)
                {
                    self.ink.push(props.foreground.unwrap_or_else(|| self.current_ink()));
                    self.lower_into(child, out);
                    self.ink.pop();
                    self.font = outer_font;
                    self.line_height = outer_line_height;
                    self.text_align = outer_text_align;
                    return;
                }
                let mut boxed = node(DomKind::Box);
                let interactive = self.pending_interactive.take();
                let mut look = DomLook {
                    // a layer that asks for nothing lets the click
                    // reach whatever it covers
                    pass_through: self.overlay_depth > 0 && interactive.is_none(),
                    group: self.groups.last().copied(),
                    transition: self.pending_transition.take(),
                    ..DomLook::from_props(props)
                };
                let marks = DomMarks { tooltip: self.pending_tooltip.take(), group_owner: None };
                // the tint is the half of the material an ELEMENT owns:
                // it sits under whatever the box paints itself, because
                // an element has one background colour. The tint never
                // rides the wire on its own — it folds in here
                if let Some(glass) = props.glass {
                    let tint = glass.resolve(crate::layout::Rect { origin: crate::layout::Point { x: 0.0, y: 0.0 }, size: crate::layout::Size::default() }).tint;
                    look.background = crate::dom::GlassFilter::under(tint, look.background);
                    look.hover_background = look
                        .hover_background
                        .and_then(|color| crate::dom::GlassFilter::under(tint, Some(color)));
                    look.pressed_background = look
                        .pressed_background
                        .and_then(|color| crate::dom::GlassFilter::under(tint, Some(color)));
                }
                if let Some(color) = props.foreground {
                    self.ink.push(color);
                } else {
                    self.ink.push(self.current_ink());
                }
                if states || inheriting {
                    look.color = Some(self.current_ink());
                }
                boxed.style = DomStyle::new(interactive, look, marks);
                if states {
                    self.ink_scopes.push(self.ink.len());
                }
                // a box that changes the face declares it for its
                // subtree, once; the texts under it with that face
                // inherit it and name none of their own
                let outer_declared = self.declared;
                if self.font != self.declared {
                    boxed.face = Some(self.face_record(self.font));
                    self.declared = self.font;
                }
                self.lower_into(child, &mut boxed.children);
                self.declared = outer_declared;
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
                // a text that reads for itself carries its binding into
                // the scene: the lowering patches it by key when a write
                // reaches it, and no walk comes this way for that
                let binding = content.bound().cloned();
                let mut text = node(DomKind::Text(DomText {
                    content: content.get(),
                    color: self.current_ink(),
                    inherits_ink: !self.ink_scopes.is_empty(),
                    font: self.font,
                    line_height: self.line_height.map(|height| height as f32),
                    text_align: self.text_align,
                    highlights: highlights
                        .as_ref()
                        .map(|h| (std::rc::Rc::clone(&h.ranges), h.color)),
                    truncation: *truncation,
                    inherits_face: self.font == self.declared,
                }));
                text.style.interactive = self.pending_interactive.take();
                text.style.set_tooltip(self.pending_tooltip.take());
                text.binding = binding.map(crate::dom::NodeBinding::Text);
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
                secret,
            } => {
                self.fields.push((path.clone(), *auto_focus));
                let theme = crate::theme::current();
                let mut field = node(DomKind::Field(Box::new(crate::dom::DomField {
                    path: path.clone(),
                    content: content.clone(),
                    placeholder: placeholder.clone(),
                    secret: *secret,
                    font: self.font,
                    color: self.current_ink(),
                    multiline: *multiline,
                })));
                field.style = DomStyle::of_look(DomLook {
                    background: (!*bare).then_some(theme.field),
                    border: (!*bare).then_some((theme.field_border, 1.0)),
                    corner_radius: (!*bare)
                        .then_some(crate::layout::Corners::all(crate::layout::FIELD_RADIUS)),
                    focus_border: (!*bare).then_some(theme.focus),
                    placeholder_color: Some(theme.placeholder),
                    ..DomLook::default()
                });
                out.push(field);
            }
            // a feed has no bytes an `<img>` could name: it is painted,
            // as an island, by the road that scales it on the GPU
            #[cfg(feature = "canvas")]
            LayoutNode::Image { source: Some(crate::image_engine::ImageSource::Feed { .. }), .. } => {
                out.push(self.island(tree, None));
            }
            #[cfg(not(feature = "canvas"))]
            LayoutNode::Image { source: Some(crate::image_engine::ImageSource::Feed { .. }), .. } => {
                out.push(node(DomKind::Box));
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
            LayoutNode::VirtualStack { row_extent, count, children, heights, measured } => {
                // the ONE number the browser cannot give: each row's
                // slot and the total, prefix sums — arithmetic, never
                // measure. Summed once and read for every row on the
                // glass: a sum per row walked the whole head above it,
                // once for each
                let starts: Option<std::rc::Rc<Vec<Px>>> = match (measured, heights) {
                    // rows that measure themselves: the starts their
                    // cache keeps, which a settled list never re-sums
                    (Some(cache), _) => Some(cache.offsets(*count)),
                    // the app declared every row's extent
                    (None, Some(rows)) => {
                        let mut starts = Vec::with_capacity(*count + 1);
                        let mut top: Px = 0.0;
                        starts.push(top);
                        for row in 0..*count {
                            top += (rows.0)(row);
                            starts.push(top);
                        }
                        crate::stats::note_rows_summed(*count);
                        Some(std::rc::Rc::new(starts))
                    }
                    (None, None) => None,
                };
                let start_of = |index: usize| -> Px {
                    match &starts {
                        Some(starts) => starts[index.min(*count)],
                        None => *row_extent * index as f64,
                    }
                };
                let total = start_of(*count);
                let mut content = node(DomKind::Content);
                content.layout.as_mut().expect("flow node").height = Some(total as f32);
                for (index, child) in children {
                    let opened = content.children.len();
                    self.lower_into(child, &mut content.children);
                    for row in &mut content.children[opened..] {
                        if let Some(layout) = row.layout.as_mut() {
                            layout.slot_y = Some(start_of(*index) as f32);
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
                                Axis::Horizontal => layout.width = Some(*at as f32),
                                Axis::Vertical => layout.height = Some(*at as f32),
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

            LayoutNode::Boundary { path, children, .. } => {
                self.lower_boundary(path, children, false, out);
            }
            LayoutNode::BoundaryRef { path, slot, hints } => {
                let opened = out.len();
                // resolves through the retention IN PLACE, the same
                // door the placement walk uses, and lowers straight into
                // the parent's list — a list of its own was a block of
                // four nodes per row, filled with one and copied out. A
                // missing entry keeps the identity anchor so the diff
                // can match later
                let found = slot.with_layout(|tree| match tree {
                    // the frame's tree names only retained boundaries:
                    // an entry that left keeps its slot filled until the
                    // page is idle, but no tree of the frame refers to it
                    // any more — so the boundary is not asked again
                    Some(LayoutNode::Boundary { path, children, .. }) => {
                        debug_assert!(
                            crate::reconciler::is_retained(path),
                            "the frame reached a boundary that left: {path}"
                        );
                        self.lower_boundary(path, children, true, out);
                        true
                    }
                    Some(tree) => {
                        self.lower_into(tree, out);
                        true
                    }
                    None => false,
                });
                if !found {
                    out.push(node(DomKind::Group { path: std::rc::Rc::clone(path) }));
                }
                // the hints an `.element(…)` gave the boundary, stamped
                // as the wrapper they replace would stamp them
                if !hints.is_empty() {
                    Self::stamp_hints(&mut out[opened..], &hints.tag, &hints.class, &hints.dom_id);
                }
            }
            LayoutNode::Interactive { path, child } => {
                self.pending_interactive = Some(std::rc::Rc::clone(path));
                self.lower_into(child, out);
                self.pending_interactive = None;
            }
            LayoutNode::Animated { spec, child, .. } => {
                self.pending_transition = Some((spec.response, spec.damping));
                self.lower_into(child, out);
                self.pending_transition = None;
            }
            LayoutNode::BoundaryHint { class } => {
                self.pending_boundary_class = Some(match class {
                    Some(class) => (class.get(), class.bound().cloned()),
                    None => (String::new(), None),
                });
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
                    owner.style.set_group_owner(Some(key));
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
                    ring.style = DomStyle::of_look(DomLook {
                        border: Some((accent, 2.0)),
                        corner_radius: Some(crate::layout::Corners::all(6.0)),
                        pass_through: true,
                        ..DomLook::default()
                    });
                    out.push(ring);
                }
            }
            LayoutNode::DragRegion { child } => {
                // a window drag region means nothing in a browser tab
                self.lower_into(child, out);
            }
            LayoutNode::IgnoresSafeArea { child } => {
                // the browser keeps its own safe area outside the tab
                self.lower_into(child, out);
            }
            LayoutNode::Hinted { tag, class, dom_id, child } => {
                let opened = out.len();
                self.lower_into(child, out);
                Self::stamp_hints(&mut out[opened..], tag, class, dom_id);
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
            // the native host's web lowering (`docs/webview.md`,
            // `docs/video.md`): the "native view" is the browser's own
            // element — an iframe for a page, a video for a stream —
            // and the island contract is the one the DOM already
            // enforces
            LayoutNode::Host { spec, .. } => {
                let mut frame = node(match spec {
                    // a document rides SEALED: the browser's sandbox
                    // with no powers holds it as `srcdoc`, its policy at
                    // its head — never the url, which the policy forbade
                    crate::host::HostSpec::Webview { url, document, .. } => match document {
                        Some(document) => DomKind::Iframe {
                            src: std::rc::Rc::from(document.sealed()),
                            sealed: true,
                        },
                        None => DomKind::Iframe { src: std::rc::Rc::clone(url), sealed: false },
                    },
                    // the stream rides by its handle; the glue finds it
                    // in the page's registry — the stream itself never
                    // crossed into the engine
                    crate::host::HostSpec::Video { stream, mirrored, cover, corner_radius } => {
                        DomKind::Video {
                            stream: stream.0,
                            mirrored: *mirrored,
                            cover: *cover,
                            radius: *corner_radius as f32,
                        }
                    }
                });
                let layout = frame.layout.as_mut().expect("flow node");
                // a host is a filler on both axes by construction — it
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

    /// The hints stamp whatever their child lowered to — one node in
    /// practice (a hinted stack, text, box, or boundary).
    fn stamp_hints(
        lowered: &mut [DomNode],
        tag: &Option<std::rc::Rc<str>>,
        class: &Option<std::rc::Rc<str>>,
        dom_id: &Option<std::rc::Rc<str>>,
    ) {
        // a table cell that holds one plain text IS that text
        if let Some(tag) = tag
            && (&**tag == "td" || &**tag == "th")
            && let [cell] = lowered
        {
            Self::fold_cell(cell);
        }
        // an inline tag around one child is no flex box
        if let Some(tag) = tag
            && INLINE_TAGS.contains(&&**tag)
            && let [only] = lowered
        {
            Self::fold_inline(only);
        }
        for hinted in lowered {
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

    /// A boundary: a promise of reuse when nothing it shows can have
    /// changed, its group lowered again otherwise. `retained` says the
    /// walk reached it through its slot, which only a retained entry
    /// fills — the retention is not asked again.
    fn lower_boundary(
        &mut self,
        path: &std::rc::Rc<str>,
        children: &[LayoutNode],
        retained: bool,
        out: &mut Vec<DomNode>,
    ) {
        // a CLEAN boundary is a promise, not a walk: no body at
        // or under it ran, the retained group still holds, and
        // it was lowered in the environment the walk carries
        // now — so the diff keeps it wholesale, O(change), by
        // absence. The promise is born as the group's own
        // shell, and the parent stamps its part again.
        let key = self.flow_key();
        let holds = |record: &GroupRecord| {
            if retained || crate::reconciler::is_retained(path) {
                // a component's body is its own: the run above
                // changed nothing it shows unless the environment
                // it is lowered in moved
                record.env == key
            } else {
                // an identity scope with no body (a list's row) is
                // the content of the body above it
                !self.changed.run_above(path)
            }
        };
        if let Some(record) = self.env.retained_groups.get(&**path)
            && !(self.runs_below && self.changed.touches(path))
            && holds(record)
        {
            let mut promise = node(DomKind::Reuse { path: std::rc::Rc::clone(path) });
            if let Some(layout) = promise.layout.as_mut() {
                layout.stretch = record.own_stretch;
            }
            promise.hints.class = record.own_class.clone();
            promise.binding = record.class_binding.clone().map(crate::dom::NodeBinding::Class);
            self.drops_seen += record.drops;
            out.push(promise);
            return;
        }
        let mut group = node(DomKind::Group { path: std::rc::Rc::clone(path) });
        group.children.reserve_exact(children.len());
        let outer_pending = self.pending_boundary_class.take();
        let drops_before = self.drops_seen;
        // what ran under this boundary is what ran under its parent and
        // under its own path: when nothing did, no boundary below it is
        // touched, and none of them asks
        let outer_runs = self.runs_below;
        self.runs_below = outer_runs && self.changed.above_a_run.contains(&**path);
        for child in children {
            let opened = group.children.len();
            self.lower_into(child, &mut group.children);
            Self::stamp_fill(child, &mut group.children[opened..]);
        }
        self.runs_below = outer_runs;
        Self::inherit_stretch(&mut group);
        let own_stretch = group.layout.as_ref().is_some_and(|layout| layout.stretch);
        let mut class_binding = None;
        if let Some((class, binding)) = self.pending_boundary_class.take() {
            // the body spoke about its own element: an empty
            // class clears, anything else attributes
            group.hints.class =
                (!class.is_empty()).then(|| std::rc::Rc::from(class.as_str()));
            class_binding = binding;
        }
        group.binding = class_binding.clone().map(crate::dom::NodeBinding::Class);
        self.pending_boundary_class = outer_pending;
        self.groups_out.push((
            std::rc::Rc::clone(path),
            GroupRecord {
                env: key,
                own_stretch,
                own_class: group.hints.class.clone(),
                class_binding,
                drops: self.drops_seen - drops_before,
            },
        ));
        out.push(group);
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

    /// A stack with one child, wearing an inline tag — a link around a
    /// word, a span around an icon — needs no flex line: with one
    /// child there is nothing to distribute, and the browser's own
    /// display for the tag lays the child out the same. A gap, a wrap,
    /// a pinned size or a slot keep the flex box, because those are
    /// flex semantics the browser's inline flow would not honour.
    fn fold_inline(node: &mut DomNode) {
        // one child or none: an empty inline tag — a glyph the page's
        // stylesheet draws — has no line to lay out either
        let at_most_one = matches!(node.kind, DomKind::FlexRow | DomKind::FlexColumn)
            && node.children.len() <= 1;
        if !at_most_one {
            return;
        }
        if let Some(layout) = node.layout.as_mut()
            && layout.gap.is_none()
            && layout.wrap.is_none()
            && layout.width.is_none()
            && layout.height.is_none()
            && layout.slot_y.is_none()
        {
            layout.plain = true;
        }
    }

    /// A column around ONE table, pinned to the leading edge, is a
    /// block. A table is never a flex line's cheap item: as the item of
    /// a column the browser measures the whole table for its flex base
    /// size and lays it out again at the size the line settles on —
    /// every row, on every relayout the table takes part in. As the one
    /// child of a block it is laid out once, at the same place and the
    /// same width: a table in block flow already sizes to its content
    /// and sits on the leading edge, as a column's leading item does.
    /// A table that grows, fills or stretches keeps the flex line — a
    /// block would not hand it the offer — and so does any other
    /// alignment, which a block cannot say.
    fn fold_table_wrapper(container: &mut DomNode) {
        if !matches!(container.kind, DomKind::FlexColumn) || container.children.len() != 1 {
            return;
        }
        let table = &container.children[0];
        let lone_table = table.hints.tag.as_deref() == Some("table")
            && table.layout.as_ref().is_none_or(|layout| {
                !layout.grow && !layout.fill && !layout.stretch && layout.slot_y.is_none()
            });
        if !lone_table {
            return;
        }
        if let Some(layout) = container.layout.as_mut()
            && layout.align == Some(align_code(CrossAlign::Start))
            && layout.wrap.is_none()
        {
            layout.plain = true;
        }
    }

    /// A cell whose one child is a plain text becomes that text: the
    /// cell element carries the words, and the flex box between them
    /// is not made. A cell lays itself out as a table cell, so nothing
    /// the flex box said about layout is lost; what the box took from
    /// the walk — the press, the tooltip, the transition — the text
    /// takes.
    fn fold_cell(cell: &mut DomNode) {
        let plain_text = matches!(cell.kind, DomKind::FlexRow | DomKind::FlexColumn)
            && cell.children.len() == 1
            && matches!(cell.children[0].kind, DomKind::Text(_))
            && cell.children[0].hints.is_empty();
        if !plain_text {
            return;
        }
        let mut text = cell.children.pop().expect("the one child");
        if text.style.interactive.is_none() {
            text.style.interactive = cell.style.interactive.take();
        }
        if text.style.tooltip().is_none() {
            text.style.set_tooltip(cell.style.take_tooltip());
        }
        if text.style.look().transition.is_none() {
            text.style.set_transition(cell.style.take_transition());
        }
        *cell = text;
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
        let (size, fit) = subtree.measure(proposal, &env);
        let mut placement = crate::layout::Placement::with_capture(size, self.current_ink());
        subtree.place(
            crate::layout::Rect { origin: Point::default(), size },
            &fit,
            &env,
            &mut placement,
        );
        // the interior arrives ABSOLUTE (geometry on every node); the
        // wrapper is a Content box — position:relative by creation —
        // sized by our answer, carried by the flow like any other box
        let captured = placement.take_capture().finish();
        let mut wrapper = node(DomKind::Content);
        {
            let layout = wrapper.layout.as_mut().expect("flow node");
            layout.width = Some(size.width as f32);
            layout.height = Some(size.height as f32);
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
        let (measured, fit) = subtree.measure(proposal, &env);
        // which axes FOLLOW the proposal? offer a different box and
        // watch what moves — a moved axis belongs to the browser:
        // `align-self: stretch`, no pinned size, and every resize
        // comes back through the observer
        let shifted = crate::layout::Proposal {
            width: Some(proposal.width.unwrap_or(measured.width) + 97.0),
            height: Some(proposal.height.unwrap_or(measured.height) + 97.0),
        };
        let (moved, _) = subtree.measure(shifted, &env);
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
            &fit,
            &env,
            &mut placement,
        );
        self.display.extend(placement.display);
        // the boxes inside this island keep their LOCAL frames — the
        // pointer door routes the browser's canvas coordinates by them
        if let Some(island) = &path {
            for custom in placement.customs {
                self.customs.push((std::rc::Rc::clone(island), custom));
            }
            self.islands_walked.push(std::rc::Rc::clone(island));
            for (target, frame) in placement.hits {
                self.hits.push((std::rc::Rc::clone(island), target, frame));
            }
        }
        let mut island = node(DomKind::Canvas {
            origin: (0.0, 0.0),
            display: (start, self.display.len()),
            path,
        });
        {
            let layout = island.layout.as_mut().expect("flow node");
            layout.width = (!hungry.0).then_some(size.width as f32);
            layout.height = (!hungry.1).then_some(size.height as f32);
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
            retained_groups: Box::leak(Box::new(HashMap::default())),
            island_boxes: Box::leak(Box::new(HashMap::default())),
        }
    }

    fn text_node(content: &str) -> LayoutNode {
        LayoutNode::Text {
            content: crate::bind::TextSource::from(content),
            highlights: None,
            truncation: None,
        }
    }

    /// A table the column must feed — one that stretches across it —
    /// keeps the flex line: a block would leave it at its content's
    /// width. A table that asks nothing of the line sits in a block.
    #[test]
    fn a_table_the_line_feeds_keeps_its_flex_line() {
        let column = |stretch: bool| {
            let mut table = node(DomKind::FlexRow);
            table.hints.tag = Some("table".into());
            table.layout.as_mut().expect("flow").stretch = stretch;
            let mut container = node(DomKind::FlexColumn);
            container.layout.as_mut().expect("flow").align = Some(align_code(CrossAlign::Start));
            container.children.push(table);
            Walk::fold_table_wrapper(&mut container);
            container.layout.expect("flow").plain
        };
        assert!(column(false), "a table that asks nothing sits in a block");
        assert!(!column(true), "a stretched table keeps the line that stretches it");
    }

    /// The exact box says WHERE on the second road too: the browser
    /// places the child with the same edge the scene does, so a column
    /// of a grid reads the same in both.
    #[test]
    fn an_exact_frame_carries_its_edge_to_the_flow() {
        let lane = |align: CrossAlign| LayoutNode::Frame {
            width: Some(132.0),
            height: Some(30.0),
            align,
            child: Box::new(text_node("Ada")),
        };
        let offsets = HashMap::default();
        let code = |align: CrossAlign| {
            let scene = lower(&lane(align), &env_fixture(&offsets)).scene;
            scene.children[0].layout.as_ref().expect("flow").align
        };

        assert_eq!(code(CrossAlign::Start), Some(align_code(CrossAlign::Start)));
        assert_eq!(code(CrossAlign::Center), Some(align_code(CrossAlign::Center)));
        assert_eq!(code(CrossAlign::End), Some(align_code(CrossAlign::End)));
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
                        document: None,
                        url: "https://example.test/docs".into(),
                        scripts: Vec::new().into(),
                        console: false,
                        requests: false,
                        full_motion: false,
                    },
                },
            ],
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let row = &scene.children[0];
        let pane = &row.children[1];
        assert!(
            matches!(
                &pane.kind,
                DomKind::Iframe { src, sealed: false } if &**src == "https://example.test/docs"
            ),
            "the host is an iframe with its url: {:?}",
            pane.kind
        );
        let layout = pane.layout.as_ref().expect("flow");
        assert!(layout.grow, "the page takes the leftover");
        assert!(layout.stretch, "and follows the cross axis");
    }

    /// A DOCUMENT lowers to the same iframe, sealed: the page rides
    /// as the frame's own document — never its url, which the policy
    /// forbade — and the seal stands at its head.
    #[test]
    fn a_document_lowers_to_a_sealed_iframe() {
        let document = crate::host::Document::new(
            "<p>a letter</p>",
            "https://mail.test/",
            crate::host::NetworkPolicy::Deny,
        );
        let tree = LayoutNode::Host {
            path: "letter".into(),
            spec: crate::host::HostSpec::Webview {
                url: "about:blank".into(),
                document: Some(document.clone()),
                scripts: Vec::new().into(),
                console: false,
                requests: false,
                full_motion: false,
            },
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let pane = &scene.children[0];
        match &pane.kind {
            DomKind::Iframe { src, sealed: true } => {
                assert_eq!(**src, document.sealed());
                assert!(src.starts_with("<meta http-equiv=\"Content-Security-Policy\""));
                assert!(!src.contains("about:blank"), "never the url the policy forbade");
            }
            other => panic!("the document is a sealed iframe: {other:?}"),
        }
    }

    /// The video host lowers to the browser's own `<video>`: the stream
    /// by its handle, the mirror, the fit and the radius riding the
    /// kind — hungry on both axes like the iframe, because a feed has
    /// no natural size either; the frame above decides it.
    #[test]
    fn a_video_host_lowers_to_a_video_that_fills() {
        let tree = LayoutNode::Host {
            path: "feed".into(),
            spec: crate::host::HostSpec::Video {
                stream: crate::host::MediaHandle(3),
                mirrored: true,
                cover: false,
                corner_radius: 12.0,
            },
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let feed = &scene.children[0];
        assert_eq!(
            feed.kind,
            DomKind::Video { stream: 3, mirrored: true, cover: false, radius: 12.0 },
            "the host is a video with its handle and its asks"
        );
        let layout = feed.layout.as_ref().expect("flow");
        assert!(layout.grow, "the feed takes the leftover");
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
            measured: None,
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

    /// Rows whose extents the app declares sit at the sum of the rows
    /// above them, and the content spans every row — and each row is
    /// asked for its extent ONCE, however deep the glass sits: three rows
    /// at the end of a thousand asked nearly four thousand times when
    /// every slot was a sum of its own.
    #[test]
    fn declared_rows_take_their_prefix_sums_asked_once() {
        let asked = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        let heights = {
            let asked = std::rc::Rc::clone(&asked);
            crate::layout::RowHeights(std::rc::Rc::new(move |row: usize| {
                asked.set(asked.get() + 1);
                if row.is_multiple_of(3) { 40.0 } else { 20.0 }
            }))
        };
        let tree = LayoutNode::VirtualStack {
            row_extent: 0.0,
            count: 1000,
            children: vec![
                (997, text_node("row 997")),
                (998, text_node("row 998")),
                (999, text_node("row 999")),
            ],
            heights: Some(heights),
            measured: None,
        };
        let offsets = HashMap::default();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let content = &scene.children[0];
        // 334 rows of forty (0, 3, …, 999) and 666 of twenty
        let height = content.layout.as_ref().expect("flow").height;
        assert_eq!(height, Some(334.0 * 40.0 + 666.0 * 20.0));
        let slots: Vec<_> = content
            .children
            .iter()
            .map(|row| row.layout.as_ref().expect("flow").slot_y)
            .collect();
        // above row 997: 333 rows of forty and 664 of twenty
        assert_eq!(slots, vec![Some(26_600.0), Some(26_620.0), Some(26_640.0)]);
        assert_eq!(asked.get(), 1000, "each row asked once");
    }

    /// Rows that measure themselves are slotted by the starts their row
    /// cache keeps: the heights closure is never walked, and the same
    /// list lowered again sums nothing.
    #[test]
    fn measured_rows_take_the_starts_their_cache_keeps() {
        let cache = std::rc::Rc::new(crate::layout::RowCache::new("transcript".to_string()));
        cache.set_estimate(30.0);
        let asked = std::rc::Rc::new(std::cell::Cell::new(0_usize));
        // the closure the list hands every road, as `virtual_list` does
        let heights = {
            let (asked, cache) = (std::rc::Rc::clone(&asked), std::rc::Rc::clone(&cache));
            crate::layout::RowHeights(std::rc::Rc::new(move |row: usize| {
                asked.set(asked.get() + 1);
                cache.height(row)
            }))
        };
        let tree = LayoutNode::VirtualStack {
            row_extent: 0.0,
            count: 1000,
            children: vec![(998, text_node("row 998")), (999, text_node("row 999"))],
            heights: Some(heights),
            measured: Some(std::rc::Rc::clone(&cache)),
        };
        let offsets = HashMap::default();
        let _ = crate::stats::take();
        let scene = lower(&tree, &env_fixture(&offsets)).scene;
        let content = &scene.children[0];
        assert_eq!(content.layout.as_ref().expect("flow").height, Some(30_000.0));
        let slots: Vec<_> = content
            .children
            .iter()
            .map(|row| row.layout.as_ref().expect("flow").slot_y)
            .collect();
        assert_eq!(slots, vec![Some(29_940.0), Some(29_970.0)]);
        assert_eq!(asked.get(), 0, "the closure is never walked");
        assert_eq!(crate::stats::take().rows_summed, 1000, "the cache summed its rows once");
        let _ = lower(&tree, &env_fixture(&offsets));
        assert_eq!(crate::stats::take().rows_summed, 0, "and kept them");
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
        assert_eq!(boxed.style.look().color, Some(Color::hex(0x888888)));
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

    /// A keyed list that re-runs alone keeps every row as a promise, and
    /// the walk asks nothing of a row it reached through the row's slot:
    /// the slot holds a tree only while the row is retained, and below a
    /// boundary nothing ran under no row can have been touched. A run
    /// UNDER the list is under it, though — a row whose own state moved
    /// in the frame its list reordered is lowered again, with its new
    /// words, and the rows around it stay promises.
    #[test]
    fn a_row_that_ran_while_its_list_reordered_is_lowered_again() {
        use crate::prelude::*;

        #[derive(Clone, Copy)]
        struct Item {
            id: usize,
            seen: State<usize>,
        }

        #[derive(Clone, Copy)]
        struct Row(Item);

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("row {} seen {}", self.0.id, self.0.seen.get()))
            }
        }

        #[derive(Clone, Copy)]
        struct Table {
            rows: State<std::rc::Rc<Vec<Item>>>,
        }

        impl Component for Table {
            fn body(self, _ctx: &Context) -> impl View {
                for_each(self.rows, |item| item.id.to_string(), |item| Row(*item))
            }
        }

        let size = crate::layout::Size { width: 400.0, height: 300.0 };
        let made: Vec<Item> = (1..=5).map(|id| Item { id, seen: State::new(0) }).collect();
        let table = Table { rows: State::new(std::rc::Rc::new(made.clone())) };
        let runtime = crate::runtime::Runtime::new();
        let _ = runtime.dom_frame(&table, size);
        let _ = crate::stats::take();
        let moves = |patches: &[crate::dom::DomPatch]| {
            patches.iter().filter(|p| matches!(p, crate::dom::DomPatch::Move { .. })).count()
        };

        // the list alone: every row a promise, two moves
        let mut order = made.clone();
        order.swap(1, 3);
        table.rows.set(std::rc::Rc::new(order.clone()));
        let patches = runtime.dom_frame(&table, size);
        assert_eq!(crate::stats::take().diff_reused, 5, "every row kept: {patches:?}");
        assert_eq!((moves(&patches), patches.len()), (2, 2), "{patches:?}");

        // the list and a row in one frame: that row is lowered again
        order.swap(0, 4);
        table.rows.set(std::rc::Rc::new(order));
        made[2].seen.set(1);
        let patches = runtime.dom_frame(&table, size);
        assert_eq!(crate::stats::take().diff_reused, 4, "the rows that did not run: {patches:?}");
        assert!(
            patches.iter().any(|p| matches!(
                p,
                crate::dom::DomPatch::SetContent { text, .. } if &**text == "row 3 seen 1"
            )),
            "the row that ran shows its new words: {patches:?}"
        );
        assert_eq!(moves(&patches), 2, "{patches:?}");
    }

    /// A row's `.element("tr")` rides the reference to the row's kept
    /// boundary instead of a wrapper around it: the list's tree holds
    /// the references with their hints and no box between, and the page
    /// wears them as it did — the tag and the class on every row, and a
    /// swap that moves two rows and says nothing else.
    #[test]
    fn a_hint_over_a_kept_row_rides_its_reference() {
        use crate::prelude::*;

        #[derive(Clone, Copy)]
        struct Row(usize);

        impl Component for Row {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("row {}", self.0))
            }
        }

        #[derive(Clone, Copy)]
        struct Table {
            rows: State<std::rc::Rc<Vec<usize>>>,
        }

        impl Component for Table {
            fn body(self, _ctx: &Context) -> impl View {
                for_each(self.rows, |id| id.to_string(), |id| {
                    Row(*id).element("tr").css_class("row")
                })
            }
        }

        let size = crate::layout::Size { width: 400.0, height: 300.0 };
        let table = Table { rows: State::new(std::rc::Rc::new(vec![1, 2, 3, 4, 5])) };
        let runtime = crate::runtime::Runtime::new();
        let _ = runtime.dom_frame(&table, size);
        let hinted_refs = || {
            crate::reconciler::slot_of("Table/Keyed").with_layout(|tree| match tree {
                Some(LayoutNode::Boundary { children, .. }) => children
                    .iter()
                    .filter(|row| matches!(
                        row,
                        LayoutNode::BoundaryRef { hints, .. }
                            if hints.tag.as_deref() == Some("tr")
                                && hints.class.as_deref() == Some("row")
                                && hints.dom_id.is_none()
                    ))
                    .count(),
                other => panic!("the list's retained tree: {other:?}"),
            })
        };
        assert_eq!(hinted_refs(), 5, "every row a hinted reference, no wrapper");
        let page = crate::ssr::render(&table, size);
        assert_eq!(page.html.matches("<tr ").count(), 5, "{}", page.html);
        assert_eq!(page.html.matches("class=\"row ").count(), 5, "{}", page.html);

        table.rows.set(std::rc::Rc::new(vec![1, 4, 3, 2, 5]));
        let patches = runtime.dom_frame(&table, size);
        assert_eq!(hinted_refs(), 5, "the list re-ran: its rows are references again");
        assert_eq!(patches.len(), 2, "{patches:?}");
        assert!(
            patches.iter().all(|p| matches!(p, crate::dom::DomPatch::Move { .. })),
            "the kept rows keep their tag and class: {patches:?}"
        );
    }

    /// A page whose root view is hinted is not the bare boundary a frame
    /// with nothing to do stands in for — the stand-in would carry no
    /// hints, and the page would lose the class it wears. Such a root
    /// runs its pass; the frame still says nothing.
    #[test]
    fn a_hinted_root_keeps_its_hints_through_a_quiet_frame() {
        use crate::prelude::*;

        #[derive(Clone, Copy)]
        struct Page {
            count: State<usize>,
        }

        impl Component for Page {
            fn body(self, _ctx: &Context) -> impl View {
                text(format!("count {}", self.count.get()))
            }
        }

        let size = crate::layout::Size { width: 400.0, height: 300.0 };
        let count = State::new(0);
        let root = Page { count }.element("main").css_class("page");
        let runtime = crate::runtime::Runtime::new();
        let mount = runtime.dom_frame(&root, size);
        assert!(
            mount.iter().any(|p| matches!(
                p,
                crate::dom::DomPatch::Create { hints, .. }
                    if hints.tag.as_deref() == Some("main")
                        && hints.class.as_deref() == Some("page")
            )),
            "{mount:?}"
        );
        let quiet = runtime.dom_frame(&root, size);
        assert!(quiet.is_empty(), "nothing changed, nothing is said: {quiet:?}");
        count.set(1);
        let counted = runtime.dom_frame(&root, size);
        assert!(
            !counted.iter().any(|p| matches!(p, crate::dom::DomPatch::SetHints { .. })),
            "the page keeps its class: {counted:?}"
        );
    }

    /// An island seeds its measured box on the frame that mounts it, so
    /// the browser's first report — the mount's own echo — buys no frame,
    /// and a box of the browser's own does. The seed reads the finished
    /// scene, and only when the walk lowered an island: a page of
    /// elements, a list of a thousand rows, is never walked for one.
    #[test]
    fn an_island_seeds_its_box_on_the_frame_that_mounts_it() {
        use crate::prelude::*;

        #[derive(Clone, Copy)]
        struct Card;

        impl Component for Card {
            fn body(self, _ctx: &Context) -> impl View {
                crate::vstack!(
                    text("above the island"),
                    spacer()
                        .frame(120.0, 40.0)
                        .background_color(Color::hex(0x3B82F6))
                        .rendering(Rendering::Gpu)
                )
            }
        }

        let runtime = crate::runtime::Runtime::new();
        let size = crate::layout::Size { width: 600.0, height: 300.0 };
        let mount = runtime.dom_frame(&Card, size);
        let canvas = mount
            .iter()
            .find_map(|patch| match patch {
                crate::dom::DomPatch::Create { id, kind: crate::dom::CreateKind::Canvas, .. } => {
                    Some(*id)
                }
                _ => None,
            })
            .expect("the island mounted");
        assert!(!runtime.dom_island_box(canvas, 120.0, 40.0), "the mount's echo is not news");
        assert!(runtime.dom_island_box(canvas, 130.0, 40.0), "a box of the browser's own is");
    }
}
