# A world in a box

*Status: the core is complete behind the `3d` feature — the retained
world, cameras, the orbit, picking, the frame, the software rasterizer
and the view — and the `gltf` feature reads assets. Metal draws worlds
itself on the Mac and the phone. Every other tier shows a world too:
the CPU rasterizer draws it, and the picture rides the live road the
feeds use, until the tier's own pass comes — WebGL2, then D3D11, GL and
Vulkan. `cargo test -p bunny-ui-core --features gltf` proves the core
on any machine.*

An app turns worlds on in its manifest:

```toml
bunny-ui = { version = "0.3", features = ["3d", "gltf"] }
```

A world is a retained set of entities — meshes, materials, lights and
lines — placed in meters. A box paints a picture of it through a
camera, and the picture enters the scene as ONE image: clipped, faded
and composited like any other image. Over it, the box paints what it
wants in the same frame: a label, a guide, a mark.

## The door

The short door is a view:

```rust
let world = World::new();
world.add(Mesh::cuboid(1.0, 1.0, 1.0), Material::color(orange));
world.add(Mesh::plane(8.0, 8.0), Material::color(paper)).set_position(vec3(0.0, -0.5, 0.0));
world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)));

scene_view(self.world, self.camera)      // camera: State<Camera>
    .orbit()                             // drag turns, shift pans, the wheel zooms
    .on_tap(move |hit| self.picked.set(hit.map(|hit| hit.entity)))
    .label(move || self.picked.get(), "Selected")
```

The full door is a box of the app's own (`custom(…)`), for an editor:

```rust
fn paint(&self, ctx: &PaintCtx, painter: &mut Painter) {
    painter.world(ctx.bounds(), self.world, self.camera);
    if let Some(hit) = self.under.get()
        && let Some(at) = self.camera.get().project(hit.point, ctx.size())
    {
        painter.fill_rounded(Rect::around(at, 8.0, 8.0), blue, 4.0);
    }
}

fn event(&self, event: &ElementEvent, ctx: &EventCtx) -> Response {
    // the tools first; what they leave moves the camera
    self.orbit.event(event, ctx, self.camera, self.world)
}
```

Only a camera in a `State` can orbit. `scene_view(world, camera)` with a
fixed `Camera` has no `.orbit()`: the call does not compile.

## The world

| You want | You write |
|---|---|
| A shape in a material | `world.add(mesh, material)` |
| A shape of many materials | `world.add_model(Model::new(mesh, materials))`; each vertex names its material by slot |
| Lines: edges, a grid, a guide | `world.add_lines(lines, LineStyle::solid(ink, 1.5))` |
| A light | `world.add_light(Light::sun().toward(d))`, `Light::sky(sky, ground)`, `Light::point(warm)` |
| A group that moves its children | `world.add_group()`, then `child.set_parent(Some(group))` |
| Many changes as one | `world.edit(|edit| …)` |
| The world as a function of the app's data | `world.sync(items.map(|item| (item.id, spec(item))))` |

An `Entity` is a handle: `Copy`, kept in a field, captured by a closure.
Every setter is one write and returns the entity, so one line can add
and place. A change that changes nothing writes nothing.

`world.sync` makes the keyed part of the world match a list: an entity
per key, added when the key is new, changed when its spec moved,
removed when the key is gone. Hand it the whole picture each time, and
only the difference reaches the screen; the same picture again writes
nothing at all.

Meshes are immutable and shared. `Mesh::revise(data)` keeps the
identity and moves the generation: every entity that shows the mesh
shows the new shape, and a tier uploads it once. `Model::merge` makes
many pieces one model — one mesh, one draw — and each piece keeps its
material and its part.

Shapes: `cuboid`, `plane`, `sphere`, `cylinder`, and `extrude` for a
polygon with holes. `mesh.edges(degrees)` gives the lines a drawing
shows: rims, and creases sharper than the limit.

## The law: identity and generation

A world's picture is `ImageSource::World`. Its `key` is its SLOT — the
target a tier keeps for the box that painted it. Its `generation` is a
hash of everything the picture shows: the world's version, the camera,
the style, the size, and the clock while something moves.

| Question | Answer |
|---|---|
| Is this the same box? (the target to keep) | the slot |
| Did the picture change? (the draw, the damage) | the generation |
| Did only the opacity change? | the veil, which rides the identity: the picture is not drawn again |

`Painter::world` reads the world and the camera inside the box's paint.
A change to either repaints that box and runs no body. A frame that
changed nothing in the world has an equal picture, and the presenters
skip it.

## Rest is rest

A world at rest asks for no frame: no body, no layout, no draw, no GPU
work. A world in motion — a camera in flight today — says so while it
paints: a PULSE keeps the frame driver beating, at most `max_fps` times
a second, and the box reads the engine's frame clock, which each beat
moves. The box repaints, asks again while it still moves, and once the
motion ends nothing asks: a quarter-second flight is fifteen beats at
60 Hz, and then the driver parks.

Motion is a pure function of the engine clock (`task::now()`). A camera
flight is a start time and a length: no state is written while it flies.

## Cameras and the hand

A camera is a plain value: an eye, a target, an up and a lens. A
perspective lens shows far things small; a parallel lens keeps their
size, for a plan or an isometric view. `project` takes a world point to
the box; `ray` takes a point of the box back into the world; the two
agree. `framing` fits a box of the world into view. `fly_to` reaches
another camera along a path that curves around the target.

`Orbit` reads the box's events and moves a `State<Camera>`. A drag
turns the view around the surface the press landed on, and that point
stays under the pointer; shift pans; the wheel and a pinch zoom toward
the pointer. The pitch and the distance stay inside `OrbitLimits`. A
press that never travels comes back IGNORED on its release: that is a
tap, and the caller answers it.

## Picking

`world.pick(camera, point, size)` returns the first surface under a
point of the box: the entity, the point, the normal, the distance, the
triangle, its part, and the side the ray met. It runs on the CPU, the
same on every tier: each mesh keeps a bounding volume hierarchy, built
the first time a ray asks. Hidden and unpickable entities are skipped.

## Precision

Public numbers are `f64`. Each instance is placed relative to the eye
in `f64` and only then cast to `f32`, so the GPU never sees a large
number: a cube three thousand kilometres from the origin draws exactly
like one at the origin.

## The frame and the tiers

`world.frame(camera, style, size, scale)` decides one picture on the
CPU: which entities the camera sees, which share a draw, the order of
the draws, the near and far planes, the lights in the eye's space. A
tier runs the draws in order. The bytes have one layout for every tier,
built of 16-byte rows so that std140, std430, HLSL and MSL agree; tests
pin every offset.

| Tier | Today |
|---|---|
| CPU oracle | draws every world; byte-stable on every machine |
| Metal (Mac, phone) | draws worlds itself: one pass per world, four samples resolved in tile memory, held to the oracle by parity tests |
| WebGL2, D3D11, GL, Vulkan | show the oracle's picture through the live road; their own passes come next |

A tier that draws worlds keeps a target per box and hands the 2D pass
the target like a feed. A world whose picture did not move is not
drawn again: a repeated frame, a fade, or a frame that changed only the
2D around it costs the GPU nothing for the world. Meshes and textures
go up once per identity and generation, and what no frame reads for a
while is given back.

## Quality

| | Samples | Pixel budget |
|---|---|---|
| `Quality::Low` | 1 | 1.2 million |
| `Quality::Medium` (default) | 4 | 2.2 million |
| `Quality::High` | 4 | 4 million |

A box inside the budget is drawn at its own size. A larger box is drawn
smaller, and the compositor stretches it to the box.

## The oracle

The software rasterizer follows the rules a GPU follows: clipping in
homogeneous space, corners snapped to 1/256 of a pixel, the top-left
rule, depth linear on the screen, attributes corrected for perspective,
colour blended as an 8-bit target blends it. Lines are quads of analytic
coverage, pulled a hair toward the eye so an edge wins against its
faces. Where a frame draws edges, each opaque surface also steps back
by its own depth slope, so an edge on a surface that turns away from
the eye stays whole across its width — and never past the far plane. Its shading uses `+ − × ÷`, `sqrt`, `floor` and fixed tables
only: the same frame gives the same bytes on every machine, and a test
pins the fingerprint of one picture.

`world.render(camera, style, size, scale)` draws a picture on the CPU —
a thumbnail, an export, a test. `world.render_with(images, …)` decodes
textures through an image engine.

## Assets

With the `gltf` feature, `Asset::from_bytes` reads a GLB file, or a
glTF text whose buffers are data URIs: triangle meshes, quantized
attributes, metal-rough materials with a base colour texture, emissive
light, alpha modes, two sides, unlit surfaces, and the node tree. The
`extras` reach the app as JSON text. `world.spawn(&asset)` places it
under a group; an asset spawned twice draws as one instanced mesh.
Every malformed input is an error, never a panic.

## Limits

- A world inside a `.looping` box or an overlay island is drawn on the
  CPU: those roads rasterize their boxes. Paint a world on the frame's
  road.
- Labels are painted text. A label that is a view comes later.
- No shadows, no skins and no animation clips yet.
- A model holds at most 256 materials.
- The preview wire carries no picture of a world: a preview shows the
  box empty.

## Proof

```bash
cargo test -p bunny-ui-core --features 3d
cargo test -p bunny-ui-core --features gltf
BUNNY_PARANOID=all cargo test -p bunny-ui-core --all-features
cargo test -p bunny-ui-apple --features 3d
cargo run -p bunny-ui-macos --features 3d --example world_window
BUNNY_WORLD_GLB=chair.glb BUNNY_WORLD_DUMP=/tmp cargo test -p bunny-ui-core --features gltf --lib a_glb_from_the_environment -- --ignored
```
