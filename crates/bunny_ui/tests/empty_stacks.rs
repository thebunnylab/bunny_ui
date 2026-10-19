//! Blank containers remain real layout nodes in native and DOM output.
use bunny_ui::dom::{CreateKind, DomPatch};
use bunny_ui::layout::Proposal;
use bunny_ui::prelude::*;
use bunny_ui_core as bunny_ui;

#[test]
fn every_empty_stack_accepts_padding_without_a_synthetic_child() {
    for view in [
        erased(vstack!()),
        erased(hstack!()),
        erased(zstack!()),
        erased(grid!()),
    ] {
        let layout = Runtime::new().layout(&view.padding_length(8.0), Proposal::unspecified());
        assert_eq!(
            layout.size,
            Size {
                width: 16.0,
                height: 16.0
            }
        );
        assert!(layout.display.is_empty());
    }
}

#[test]
fn empty_stacks_keep_their_dom_layout_kind() {
    for (view, expected) in [
        (erased(vstack!()), CreateKind::FlexColumn),
        (erased(hstack!()), CreateKind::FlexRow),
        (erased(zstack!()), CreateKind::Layers),
        (erased(grid!()), CreateKind::Grid),
    ] {
        let patches = Runtime::new().dom_frame(
            &view,
            Size {
                width: 240.0,
                height: 120.0,
            },
        );
        let kinds = patches
            .iter()
            .filter_map(|patch| match patch {
                DomPatch::Create { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(kinds.contains(&expected), "{kinds:?}");
        assert!(
            !kinds.contains(&CreateKind::Text),
            "no placeholder child is inserted"
        );
    }
}
