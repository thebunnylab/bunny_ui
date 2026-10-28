# Sticky views

`.sticky(top)` is CSS `position: sticky; top`. The view is measured and laid out
in its slot as before. Once its nearest vertical scroll region has carried the
slot above the line `top` points below the region's top edge, placement moves
the view down to that line. It never crosses the bottom edge of its containing
block, and that edge carries it away again.

```rust
hstack!(cards, detail.sticky(0.0)).alignment(VerticalAlignment::Top)
```

The containing block is the box of the stack, flow, grid, pile, virtual list,
split or several-view component body that lays the view out among its siblings.
When the view sits in the region directly, the block is the region's content. A
wrapper of one box (a frame, padding, style or overlay base) is that box itself
and bounds nothing. Put `.sticky` outermost among a view's modifiers. A
non-finite `top` never sticks.

Paint, presses, focus stops, the focus ring and accessible bounds move with the
view, because the shift is applied before anything reads the frame. Springs
inside it anchor to the moved box, so scrolling never animates an `.animated`
child.

## Native and browser representation

Native placement carries two scoped stacks beside the clip and the scroll
anchors:

- the viewport of every enclosing vertical region, which is the line's edge;
- every open containing block, which is the bound.

The shift is `min(line − slot.top, block.bottom − slot.bottom)`, floored at
zero. It is a `VisualProps` field (placement only, like `text_align`), so it
folds into the style a panel already wears. Canvas islands stick as native
paint does.

A browser scrolls what the element renderer lowers without asking for a layout,
so the browser does the sticking there:

- **Flow pages.** The element carries `position:sticky;top`. Its DOM parent is
  the element of the same container, so the browser bounds the view as native
  placement does.
- **`.layout(Exact)` interiors.** Elements are positioned by frame and CSS has
  no stick for an absolutely positioned box. The capture stands the view in a
  wrapper that takes no pointer, running from the slot to the bottom of the
  containing block. The view sits at the wrapper's top in the flow, where the
  browser sticks it. It takes the pointer back with `pointer-events:auto`,
  unless a subtree around it declined the pointer. Its children stay absolute.

DOM ABI 23 carries the stick in `SetBox` bit 7: an f32 top, then a u8 flag
(1 adds `pointer-events:auto`). It is per-element geometry, kept in the rare
placement record and outside shared appearance rules. The decoder and the
served-page renderer apply it after every other position the box names. The
exception is a box a row or column took out of the flow with `.position(x, y)`:
it stays absolute and does not stick. All glue copies carry the same ABI as the
core.

## Differences from CSS

- **Paint order stays the tree's.** A sibling drawn after a stuck view paints,
  and takes presses, over it where they overlap. A browser lifts a positioned
  box above its in-flow siblings. A pinned header over rows therefore goes after
  them in a pile: `zstack!(rows, header.sticky(0.0))`, with the rows padded down
  by the header's height.
- **Only the top edge sticks, and only a vertical region holds it.** A sideways
  strip between the view and that region is passed through. A browser would take
  the strip as the scrollport.
- **Region padding is content.** A browser holds the line inside a scroll
  container's padding. Here padding is a node inside the region's content, so
  the inset includes it: a region whose content is padded by 24 sticks at
  `.sticky(24.0)` to match `top: 0` in CSS.

`crates/bunny_ui/tests/sticky.rs` covers the slot before the line, holding the
line, release at the containing row, no region, presses following the stuck
view, animated content inside it, and both served lowerings.
