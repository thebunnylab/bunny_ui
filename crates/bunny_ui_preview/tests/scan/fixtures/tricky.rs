const S: &str = "#[preview] fn not_a_preview() {}";
// #[preview] fn commented_out() {}

#[cfg_attr(feature = "x", preview)]
fn cfg_attr_ignored() -> impl View {
    text("x")
}

#[preview]
fn with_params(x: u32) -> impl View {
    text("x")
}

#[preview(label = "nope")]
fn unknown_arg() -> impl View {
    text("x")
}

#[preview]
struct NotAFn;

#[preview(name = "Fine", scope = "Status")]
fn fine() -> impl View {
    text("x")
}
