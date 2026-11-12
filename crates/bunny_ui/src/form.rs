//! The form: rows of settings on one card, and the row that names a control.
//!
//! ```ignore
//! form!(
//!     labeled("Name", text_field("Jane Appleseed", self.name.binding())),
//!     toggle(text("Public profile"), self.public.binding()),
//!     labeled("Theme", picker(self.look.binding(), looks())),
//! )
//! ```
//!
//! A form is one card. Each child is a row, with a hairline between two
//! rows. A row can be any view. [`labeled`] is the row most settings
//! want: the words on the leading side, the control on the trailing
//! side. A toggle is such a row by itself.
//!
//! A group of settings with a title is a section around a form:
//!
//! ```ignore
//! section(text("Profile"), form!(…))
//! ```
//!
//! A form does not scroll. Put it in a `scroll(…)` when it can be taller
//! than its place.
//!
//! ## Keys
//!
//! A form adds no key of its own. Tab goes through the controls of the
//! rows in reading order.
//!
//! ## Assistive technology
//!
//! A form has no role. A labeled row gives its words to its control as
//! the control's name, so the words are heard once: "Name, text field",
//! not "Name" and then a field named by its placeholder. A row whose
//! control is several elements, such as a segmented control, keeps the
//! words as a text before them.

use motor::state::Context;
use motor::view::RenderNode;

use crate::layout::{Axis, Corners, CrossAlign, Edges, LayoutNode, Size, VisualProps};
use crate::provenance::{NodeKind, Site, Written};
use crate::text_value::IntoText;
use crate::view::{NodeList, Single, View};
use crate::views::{Text, hstack, spacer};

const CARD_RADIUS: f64 = 10.0;
/// Around the content of a row.
const ROW_INSET: Edges = Edges { top: 8.0, bottom: 8.0, leading: 12.0, trailing: 12.0 };
/// Between the words of a labeled row and its control.
const ROW_GAP: f64 = 12.0;

/// Rows of settings on one card ([`form`]).
#[derive(Clone)]
pub struct Form<C> {
    rows: C,
    site: Site,
}

impl<C: View> View for Form<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        let mut rows = NodeList::new();
        self.rows.render_into(ctx, &mut rows);
        let (prints, layouts) = rows.into_parts();
        out.push(RenderNode::branch(crate::view::print_label("Form"), prints));

        let theme = crate::theme::current();
        let mut children = Vec::with_capacity(layouts.len() * 2);
        for (index, row) in layouts.into_iter().enumerate() {
            if index > 0 {
                children.push(hairline(theme.divider));
            }
            children.push(LayoutNode::Padding { edges: ROW_INSET, child: Box::new(row) });
        }
        out.push_layout(LayoutNode::Styled {
            props: VisualProps {
                background: Some(theme.panel),
                corner_radius: Some(Corners::all(CARD_RADIUS)),
                ..VisualProps::default()
            }
            .shared(),
            child: Box::new(LayoutNode::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: CrossAlign::Start,
                children,
                hints: Default::default(),
                action: None,
            }),
            hints: Default::default(),
            action: None,
        });
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Form, None);
    }
}

/// The line between two rows: one point high, as wide as the card, and
/// inset on the leading side to stand under the words.
fn hairline(color: crate::layout::Color) -> LayoutNode {
    let line = LayoutNode::Styled {
        props: VisualProps { background: Some(color), ..VisualProps::default() }.shared(),
        child: Box::new(LayoutNode::Frame {
            width: None,
            height: Some(1.0),
            align: CrossAlign::Center,
            vertical: CrossAlign::Center,
            child: Box::new(LayoutNode::MaxFrame {
                max_width: f64::INFINITY,
                max_height: f64::INFINITY,
                align: CrossAlign::Center,
                vertical: CrossAlign::Center,
                child: Box::new(LayoutNode::Leaf { size: Size { width: 0.0, height: 0.0 } }),
            }),
        }),
        hints: Default::default(),
        action: None,
    };
    LayoutNode::Padding {
        edges: Edges { top: 0.0, bottom: 0.0, leading: ROW_INSET.leading, trailing: 0.0 },
        child: Box::new(line),
    }
}

/// Rows of settings on one card. [`form!`](crate::form!) takes the rows
/// without the tuple's doubled parentheses.
///
/// [The module](self) has what a row is, the keys and the roles.
#[track_caller]
pub fn form<C: View>(rows: C) -> Form<C> {
    Form { rows, site: crate::provenance::here() }
}

/// A row that names a control: the words on the leading side and the
/// control on the trailing side ([`labeled`]).
#[derive(Clone)]
pub struct Labeled<C> {
    label: Text,
    control: C,
    site: Site,
}

impl<C: View<Arity = Single> + Clone> View for Labeled<C> {
    type Arity = Single;

    fn render_into(&self, ctx: &Context, out: &mut NodeList) {
        let mark = out.layout_mark();
        crate::provenance::quiet(|| {
            hstack((self.label.clone(), spacer(), Written(self.control.clone())))
                .spacing(ROW_GAP)
                .render_into(ctx, out);
        });
        if crate::accessibility::capturing() {
            out.wrap_layout_from(mark, |node| LayoutNode::Accessible {
                semantics: crate::accessibility::Semantics::Labeled,
                child: Box::new(node),
            });
        }
        crate::provenance::seal(ctx, out, mark, self.site, NodeKind::Labeled, None);
    }
}

/// A row that names a control:
/// `labeled("Name", text_field("Jane Appleseed", self.name.binding()))`.
///
/// The words stand on the leading side and the control on the trailing
/// side, with the room between them. The words are also the control's
/// name for assistive technology.
#[track_caller]
pub fn labeled<C: View<Arity = Single> + Clone>(label: impl IntoText, control: C) -> Labeled<C> {
    Labeled { label: label.into_text(), control, site: crate::provenance::here() }
}
