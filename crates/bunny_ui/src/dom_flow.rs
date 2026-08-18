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
}

/// Lowers the semantic tree to a flow scene. The root is the mount
/// point (id 0): the theme's canvas, the window's box.
pub(crate) fn lower(root: &LayoutNode, env: &FlowEnv) -> FlowOutput {
    let mut walk = Walk {
        env,
        ink: Vec::new(),
        ink_scopes: Vec::new(),
        font: FontSpec::DEFAULT,
        pending_interactive: None,
        pending_transition: None,
        overlays: Vec::new(),
        display: crate::layout::DisplayList::default(),
        fields: Vec::new(),
        slot: (None, None),
    };
    let mut children = Vec::new();
    walk.lower_into(root, &mut children);
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
    FlowOutput { scene, display: walk.display, fields: walk.fields }
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
    pending_interactive: Option<String>,
    pending_transition: Option<(f64, f64)>,
    overlays: Vec<DomNode>,
    display: crate::layout::DisplayList,
    fields: Vec<(String, bool)>,
    /// The nearest ancestor Frame's declared box — the proposal an
    /// island under it measures against (a flexible island learns its
    /// real box from the browser, in the island round).
    slot: (Option<Px>, Option<Px>),
}

/// A flow node with nothing to say yet.
fn node(kind: DomKind) -> DomNode {
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
                    if child.is_flexible(*axis) {
                        for grown in &mut container.children[opened..] {
                            if let Some(layout) = grown.layout.as_mut() {
                                layout.grow = true;
                            }
                        }
                    }
                }
                out.push(container);
            }
            LayoutNode::Layered { align, children } => {
                let mut container = node(DomKind::Layers);
                container.layout.as_mut().expect("flow node").align =
                    Some(align_code(*align));
                for child in children {
                    self.lower_into(child, &mut container.children);
                }
                out.push(container);
            }
            LayoutNode::Padding { edges, child } => {
                let mut container = node(DomKind::FlexColumn);
                let Edges { top, leading, bottom, trailing } = *edges;
                container.layout.as_mut().expect("flow node").padding =
                    Some((top, trailing, bottom, leading));
                self.lower_into(child, &mut container.children);
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
                out.push(container);
            }
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
                self.font = props.font.apply_over(self.font);

                let states = props.foreground_hovered.is_some()
                    || props.foreground_pressed.is_some();
                let inheriting =
                    !self.ink_scopes.is_empty() && props.foreground.is_some();
                let mut boxed = node(DomKind::Box);
                boxed.style = DomStyle {
                    interactive: self.pending_interactive.take(),
                    transition: self.pending_transition.take(),
                    ..DomStyle::from_props(props)
                };
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
                out.push(boxed);
            }
            LayoutNode::Text { content, highlights, truncation } => {
                let mut text = node(DomKind::Text(DomText {
                    content: content.clone(),
                    color: self.current_ink(),
                    inherits_ink: !self.ink_scopes.is_empty(),
                    font: self.font,
                    highlights: highlights
                        .as_ref()
                        .map(|h| (std::rc::Rc::clone(&h.ranges), h.color)),
                    truncation: *truncation,
                }));
                text.style.interactive = self.pending_interactive.take();
                out.push(text);
            }
            LayoutNode::Field { path, content, placeholder, auto_focus } => {
                self.fields.push((path.clone(), *auto_focus));
                let theme = crate::theme::current();
                let mut field = node(DomKind::Field(crate::dom::DomField {
                    path: path.clone(),
                    content: content.clone(),
                    placeholder: placeholder.clone(),
                    font: self.font,
                    color: self.current_ink(),
                }));
                field.style = DomStyle {
                    background: Some(theme.field),
                    border: Some((theme.field_border, 1.0)),
                    corner_radius: Some(crate::layout::FIELD_RADIUS),
                    focus_border: Some(theme.focus),
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
            LayoutNode::Icon { symbol, .. } => {
                out.push(node(DomKind::Icon(crate::dom::DomIcon {
                    key: symbol.key,
                    symbol: *symbol,
                    color: self.current_ink(),
                    inherits_ink: !self.ink_scopes.is_empty(),
                })));
            }
            LayoutNode::Scroll { path, target, child } => {
                let offset = self
                    .env
                    .scroll_offsets
                    .get(path.as_deref().unwrap_or(""))
                    .copied()
                    .unwrap_or_default();
                let mut scroll = node(DomKind::Scroll {
                    path: path.clone(),
                    offset: (offset.x, offset.y),
                    target: target.clone(),
                });
                scroll.layout.as_mut().expect("flow node").grow = true;
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
            LayoutNode::Boundary { path, children } => {
                let mut group = node(DomKind::Group { path: path.clone() });
                for child in children {
                    self.lower_into(child, &mut group.children);
                }
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
                    None => out.push(node(DomKind::Group { path: path.clone() })),
                }
            }
            LayoutNode::Interactive { path, child } => {
                self.pending_interactive = Some(path.clone());
                self.lower_into(child, out);
                self.pending_interactive = None;
            }
            LayoutNode::Animated { spec, child, .. } => {
                self.pending_transition = Some((spec.response, spec.damping));
                self.lower_into(child, out);
                self.pending_transition = None;
            }
            LayoutNode::DragRegion { child } => {
                // a window drag region means nothing in a browser tab
                self.lower_into(child, out);
            }
            LayoutNode::Island { child } => {
                out.push(self.island(child));
            }
            LayoutNode::Custom { .. } => {
                out.push(self.island(tree));
            }
            LayoutNode::Anchored { path, side, overlay, child } => {
                // the anchor gets an IDENTITY the glue can find: a
                // group wrapped around the child, keyed off the
                // popover's own path
                let anchor_path = format!("{path}/#anchor");
                let mut anchor = node(DomKind::Group { path: anchor_path.clone() });
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
}

impl Walk<'_> {
    /// A canvas island: the engine measures and paints ITS OWN pixels,
    /// locally — the subtree places at its own origin, so the commands
    /// are island-local by construction and the element gets a fixed
    /// box the browser never argues with.
    fn island(&mut self, subtree: &LayoutNode) -> DomNode {
        let Some(env) = self.env.layout else {
            return node(DomKind::Canvas { origin: (0.0, 0.0), display: (0, 0) });
        };
        let proposal = crate::layout::Proposal { width: self.slot.0, height: self.slot.1 };
        let (size, fit) = subtree.measure(proposal, env);
        let start = self.display.len();
        let mut placement = crate::layout::Placement::with_ink(self.current_ink());
        subtree.place(
            crate::layout::Rect { origin: Point::default(), size },
            fit,
            env,
            &mut placement,
        );
        self.display.extend(placement.display);
        let mut island = node(DomKind::Canvas {
            origin: (0.0, 0.0),
            display: (start, self.display.len()),
        });
        {
            let layout = island.layout.as_mut().expect("flow node");
            layout.width = Some(size.width);
            layout.height = Some(size.height);
        }
        island
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn env_fixture(offsets: &HashMap<String, Point>) -> FlowEnv<'_> {
        FlowEnv { scroll_offsets: offsets, size: (400.0, 300.0), layout: None }
    }

    fn text_node(content: &str) -> LayoutNode {
        LayoutNode::Text {
            content: Arc::from(content),
            highlights: None,
            truncation: None,
        }
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
