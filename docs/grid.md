# Equal-column grids

`grid!` arranges children in equal-width tracks, in reading order. It defaults
to two columns, zero gap and centered cell content.

```rust
grid!(
    text!("Heading").span(2, 1),
    text!("Name"),
    text_field(name),
)
.columns(2)
.spacing(12.0)
.row_spacing(8.0)
.alignment(Alignment::Leading)
```

`.spacing(s)` sets the column gap and the default row gap; `.row_spacing(s)`
overrides only the latter. Counts passed to `.columns` and `.span(columns, rows)`
are clamped to 1..=65535 on every target. A cell's column span is further clamped
to the grid's column count. A span survives frames, styles, provenance and
component boundaries; the outermost span wins. Outside a grid, it is transparent.

Placement is sparse: children follow a forward cursor and do not fill a hole
left before a wider child. A row span holds its columns for the following rows.
The native layout records cell allocation during measurement and reuses it in
placement. Column width is the proposed width minus gaps, divided equally.
Without a width proposal, the widest natural per-column share sets the tracks.
Single-row children set row heights; a multi-row child adds any deficit to its
last row. Children center vertically; horizontal alignment follows the chosen
edge. Column zero is on the right in RTL. The grid is flexible horizontally and
keeps its measured height.

The browser lowers a grid to kind 16, with `repeat(N,minmax(0,1fr))`, column and
row gaps, and per-cell spans. Browser auto-row sizing follows CSS Grid; native
multi-row deficit allocation follows the last-row rule above. Flexible children
fill their cell's width without changing the alignment of rigid siblings.
HTML rendering, live DOM rules and all shipped decoders share ABI 21: layout bit
14 carries u16 columns; SetBox bit 6 carries u16 column/row spans and u8 fill-x.
The position/grid family consumes a single ABI revision.

Explicit rows and track names, auto-fit/minmax configuration, fixed track widths,
dense packing, per-cell alignment and a one-argument `.span(n)` are deferred.

`layout::grid_cells` returns complete cell rectangles from already measured
outer child sizes and spans, including empty space around aligned content.
Native placement uses the same allocation, row sizing and cell rectangles.
Design tools can hit-test cells without copying the layout algorithm or using
text bounds as a proxy for the containing cell. Bounds are the Grid content
box; RTL mirrors the columns, and the returned order remains the child order.

All stack macros accept zero children: `vstack!()`, `hstack!()`, `zstack!()`
and `grid!()` retain their layout node and take modifiers such as padding or
an explicit frame. Their empty tuple is a zero-child `View` fragment; it adds
no placeholder child. This lets a visual editor create a blank container or
move its final child elsewhere without producing invalid Rust.
