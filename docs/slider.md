# Bounded scalar input

`slider` binds a horizontal control to a `Binding<f64>` and a validated
`SliderRange`. It uses the framework's existing capture, focus and rendering
paths and adds no dependency. The `canvas` feature is required; element-mode
web renders it as a canvas island.

```rust
use bunny_ui::prelude::*;

fn rate_control(rate: State<f64>) -> Result<impl View, SliderRangeError> {
    let domain = SliderRange::new(1.0..=60.0)?.with_step(1.0)?;
    Ok(slider(rate.binding(), domain).width(220.0).id("rate"))
}
```

The control stretches horizontally and defaults to 160 × 28 points when no
size is proposed. Put a visible label and current value beside it. `.disabled`
prevents writes and focus, including when toggled during a drag.

Clicking the track changes the value immediately. Grabbing the thumb preserves
the grab offset and the current value, even if external state is off the step
grid. Dragging remains captured outside the box and clamps at the endpoints.
Escape or touch cancellation ends the gesture at the last committed value;
it does not roll the binding back. Releasing after cancellation adds no write.

A pointer release inside the control takes keyboard focus. An application can
also call `runtime.focus_named("rate")` after layout, including in element mode,
and use that name in its own Tab order. An exact text-field name retains
priority; a name wrapping several focusable custom controls is ambiguous and
returns `false`. Naming does not register an automatic Tab order.

| Key | Effect |
| --- | --- |
| Right / Up | Increase by one step |
| Left / Down | Decrease by one step |
| PageUp / PageDown | Move ten steps |
| Home / End | Reach the minimum / maximum |

Horizontal arrows and pointer positions mirror in RTL. Up/Down keep their
numeric meaning. Modified shortcuts are left to the application. A continuous
range uses 1% of its span as the keyboard step. Repeated endpoint input emits no
redundant binding write, and the slider never requests text input.

`SliderRange::new` rejects nonfinite, empty, reversed and unrepresentable spans.
`with_step` rejects nonpositive/nonfinite steps and steps beyond the domain's
floating-point precision, including a grid with more than 2^52 intervals. Steps
are anchored to the minimum; both endpoints remain reachable when a step does
not divide the span. Painting reads the binding live and never repairs it:
NaN displays at the minimum, other out-of-range values at the nearest endpoint,
and the next adjustment writes a valid bounded value.

Native AX/UIA/AT-SPI range semantics are not implemented for this control yet.
Visible labeling and keyboard operation do not establish screen-reader support.
Applications requiring that support need the native range accessibility work
before treating this primitive as qualified.
