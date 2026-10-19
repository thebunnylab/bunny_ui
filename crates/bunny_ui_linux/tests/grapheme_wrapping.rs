//! Native shaping keeps the public editor's soft lines grapheme-safe.
#![cfg(target_os = "linux")]
#[path = "../../bunny_ui/tests/support/grapheme_wrap.rs"]
mod support;
use bunny_ui_linux::FreeTypeEngine;
use std::rc::Rc;
#[test]
fn native_soft_wrap_preserves_clusters_and_overwide_singletons() {
    support::clusters_stay_whole(&FreeTypeEngine::new());
}
#[test]
fn native_retained_editor_preserves_cluster_geometry() {
    support::retained_editor_keeps_atomic_lines(Rc::new(FreeTypeEngine::new()));
}
