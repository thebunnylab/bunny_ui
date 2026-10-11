# Live line charts

`line_chart` renders a validated `LineSeries` through the existing canvas and
retained scene. It adds no dependency. Applications own data collection, timing
and rolling history; the control owns geometry, nearest-sample inspection,
crosshair and tooltip painting. `canvas` is required, including for an
otherwise element-mode web page, where the chart becomes a canvas island.

```rust
use bunny_ui::prelude::*;

fn history() -> Result<impl View, ChartError> {
    let data = State::new(LineSeries::new([
        LineSample::new(17, 0.0, 42.0)?,
        LineSample::new(18, 0.1, 43.0)?,
    ])?.with_y_domain(ChartDomain::new(0.0..=100.0)?)?);
    let selected = State::new(None);
    Ok(vstack!(
        text("CPU history (%)"),
        line_chart(move || data.get())
            .inspection(selected.binding(), |s| {
                format!("{:.2}% · sample {} · {:.2}s", s.value(), s.id(), s.x())
            })
            .frame_height(180.0)
            .id("cpu-history"),
        text_with(move || {
            selected.get().and_then(|id| data.get().sample(id)).map_or_else(
                || "Use Left/Right to inspect a sample".to_owned(),
                |s| format!("Sample {} at {:.2}s: {:.2}%", s.id(), s.x(), s.value()),
            )
        }),
    ))
}
```

The ordinary text is a selected-value alternative; applications needing the
whole dataset should also offer a labeled list/table or export. The sample IDs
belong to the application and remain stable when the front of a rolling window
is removed. They need not increase numerically. X coordinates must increase
strictly: repeated x values and vertical segments are outside this control's
contract. The chart never sorts or drops points on the application's behalf.

Construct a snapshot once per data update and return a cheap clone from the
reader. `LineSample::new` rejects nonfinite coordinates. `LineSeries::new`
rejects duplicate IDs, disordered x and overflowing spans. An explicit
`ChartDomain` must have finite, increasing, representable bounds;
`with_y_domain` rejects points outside it rather than silently clamping them.
Without a fixed domain, each snapshot fits its own y minimum/maximum. Constant
axes are centered; singleton data draws a dot, and empty data draws no curve.
Every supplied point is plotted, including all 60 or 300 points in short live
histories. Larger datasets need an application-owned bounded window; there is
no automatic decimation or performance claim for unbounded inputs.

Construction costs O(n log n); painting visits all n points; sample-ID and
nearest-x lookup cost O(log n). The existing path rasterizer/cache pays its
normal cost when curves change. A plot is not a new renderer or a benchmark
shortcut. Data readers and tooltip formatters must not mutate state.

The default plot is flexible in both axes, with a 260 × 180 point fallback,
18-point insets, a two-point stroke and five horizontal grid lines.
`sparkline()` removes the grid, uses two-point insets and defaults to 48 points
high. Tiny insets collapse safely. Ink/grid colors default to theme accent and
divider; `.color` and `.grid_color` override them. Tooltip background/text and
focus outline follow the current theme. Paint stays clipped to the chart;
long tooltip text is clipped, so the external data alternative matters.

Inspection is opt-in. Hover or a press selects the real sample nearest the
pointer's x coordinate, with ties toward earlier x. A press captures movement;
release adds no second selection. Pointer exit, cancellation, blur and Escape
clear inspection. Left/Right step through samples in visual order; Home/End
choose first/last x. X and horizontal arrows mirror in RTL. Modified shortcuts
and other keys remain available to the application. The chart never requests
text input.

A click/release can focus an inspecting chart, and
`runtime.focus_named("cpu-history")` works after placement, including in web
canvas islands. A name alone does not enroll a chart in automatic Tab order on
this framework revision; applications must integrate it with their focus flow.
Keyboard selection and Escape survive the runtime's reread of a stationary
pointer. Moving the pointer again resumes hover. While hover is active, a new
frame re-evaluates the nearest point under that stationary pointer as live data
moves. Otherwise selection resolves by stable ID against the latest snapshot:
updated values appear immediately, and a removed ID draws no marker or tooltip.
Painting does not rewrite a removed ID in application state;
`LineSeries::sample(id)` returns `None` for the text/table alternative too.

These are canvas interactions, not a native accessibility chart tree. Curve
vertices and painted tooltip text are not semantic AX, UIA or AT-SPI children.
Keyboard/runtime tests and native screenshots do not qualify VoiceOver,
Narrator or Orca. Keep visible labels and real text/table alternatives, and
verify their platform access independently. Web island behavior likewise does
not imply a semantic DOM series or chart keyboard reachability from a browser's
own Tab order.
