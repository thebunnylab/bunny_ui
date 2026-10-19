# Basic shapes

`rectangle()` and `ellipse()` are ordinary views. Each accepts the width and
height proposed by its parent; `.frame(width, height)` fixes both axes. A square
ellipse is a circle. A non-square ellipse retains two independent radii, so it
never becomes a capsule or a centered circle.

```rust
use bunny_ui::prelude::*;

hstack!(
    rectangle().frame(160.0, 100.0),
    ellipse().frame(160.0, 100.0),
)
.foreground_color(theme::accent())
```

Both shapes use inherited foreground ink, including preview overrides. Without
an explicit foreground they retain the original rectangle's `Color::FILL`.
Rectangle ink is painted by the existing rectangle command. Ellipse geometry is
a four-arc cubic vector path carried by the existing image resource pipeline;
its key includes geometry and ink. Zero or non-finite extents paint nothing.
The path is rasterized at the requested physical size by the same implementation
on every native backend. No new preview drawing or resource tag is required.

Responsive DOM rendering uses a native box with `background:currentColor`;
ellipses use percentage corner radii, preserving their shape as both axes change.
The native and served page inherit foreground hover/press colors. Normal view
modifiers still describe the surrounding box: a background, border or corner
radius is a box decoration, not a vector path stroke.

The builder and its modifiers retain source sites. Preview hosts advertise the
additive `SHAPES` capability (`Hello.caps & 4`) so design tools can refuse source
insertion on a project using an older framework. Existing hosts and display-list
messages retain their prior payloads. Studio's frame tool remains an ordinary
stack; artboards are editor organization, not a new framework container.
