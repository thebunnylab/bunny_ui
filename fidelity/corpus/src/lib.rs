//! The bunny half of the fidelity corpus. Each `fidelity/cases/<case>/
//! case.rs` is a module of this crate with one `pub fn view()`: the view
//! meant to look exactly like the `case.html` beside it. The build script
//! exports each view twice for the browser — see `build.rs`.

pub mod prelude;

include!(concat!(env!("OUT_DIR"), "/cases.rs"));

/// The names a view's `.test_id(…)` left in the layout, in placement
/// order.
fn named(runtime: &bunny_ui::prelude::Runtime, view: &impl bunny_ui::prelude::View, size: bunny_ui::prelude::Size) -> Vec<String> {
    runtime.display_frame(view, size);
    runtime.test_frames().into_iter().map(|(id, _)| id.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::{Runtime, Size};

    /// Every case lays out natively and names its page and at least one
    /// box inside it: a case the harness cannot measure measures nothing.
    /// A hand-written case calls its page `page`; an imported one
    /// (`app_…`) names it after the page's own body.
    ///
    /// On a thread with a deep stack: an imported app is a screen of real
    /// size, and laying one out takes more than a test thread's two
    /// megabytes (the web build links eight).
    #[test]
    fn every_case_names_its_boxes() {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(each_case_names_its_boxes)
            .expect("a thread")
            .join()
            .expect("every case names its boxes");
    }

    fn each_case_names_its_boxes() {
        let runtime = Runtime::new();
        for (case, names) in laid_out(&runtime, Size { width: 400.0, height: 300.0 }) {
            let page = if case.starts_with("app_") { "body-0" } else { "page" };
            assert_eq!(names.first().map(String::as_str), Some(page), "{case}: {names:?}");
            assert!(names.len() > 1, "{case}: {names:?}");
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), names.len(), "{case}: a name used twice: {names:?}");
        }
        assert!(!CASES.is_empty());
    }
}
