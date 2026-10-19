# Positioned stack children

`.position(x, y)` places the **outer box** of a child in its enclosing stack's
content coordinates. It is ordinary Rust and requires no designer runtime.

```rust
zstack!(
    text!("Title").position(24.0, 16.0),
    text!("Body").frame(240.0, 80.0).position(24.0, 56.0),
)
```

| Parent | Sizing and placement |
| --- | --- |
| `vstack!` / `hstack!` | Positioned children take no flow slot, gap or share of the waterfall. They paint after the flow children, in their declaration order. |
| `zstack!` | A positioned child replaces alignment. Its far edges contribute to the stack's natural width and height. |
| No enclosing stack | Position is a transparent annotation; it does not translate the root. |

The stack proposes the room from the point to its far edges. An unspecified
axis remains unspecified. A rigid child can exceed that proposal, just as an
ordinary rigid child can. Position does not imply clipping: use `.clipped()`
on the intended viewport. Negative positions may extend beyond the near edge;
they do not move the stack's origin. Coordinates are physical, so `x` measures
from the left even in a right-to-left interface. The stack itself still follows
its parent's layout and direction.

Wrappers do not hide the child parameter. Both `.frame(...).position(...)` and
`.position(...).frame(...)` place the outer frame at the same point. Padding,
styles, accessibility and source provenance retain their own semantics. A nested
stack establishes its own coordinate space. If multiple positions occur on the
same wrapper chain, the outermost wins. Wrapping rows and virtual-list rows do
not consume position parameters; use ordinary row/column/layer stacks for free
composition.

## Native and browser representation

Native layout retains `Placed` as a transparent decorator; the parent stack
reads its point through wrapper and component boundaries. Preview overrides can
replace a position at its modifier site or append one at a builder site.

The browser uses absolute left/top coordinates under a relative flow stack.
Flexible axes also attach to the far edge. The containing block isolates paint
order so positioned children cannot jump above unrelated sibling containers.
Under layers, physical margins and per-child alignment preserve the natural
extent, including in RTL. A positioned flow stack's outer padding is kept as a
separate wrapper so it is not counted twice. HTML rendering and the DOM decoder
apply the same geometry and clear it when the position disappears.

DOM ABI 21 carries position in `SetBox` bit 5 (flags for layered/fill-x/fill-y,
then f32 x/y), uses bit 6 for grid spans (u16 columns/rows and u8 fill-x), and marks relative containers in
flow bit 15. Coordinates remain per-element geometry, outside shared appearance
rules. Rare placement data is behind one shared pointer: on 64-bit hosts the
flow record is at most 96 bytes and a retained node at most 336 bytes. All glue
copies must carry the same ABI as the core.
