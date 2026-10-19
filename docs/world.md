# A world in a box

*Status: the core is complete behind the `3d` feature — the retained
world, cameras, the orbit, picking, the frame, the software rasterizer
and the view — and the `gltf` feature reads assets. Every tier draws
worlds itself: Metal on the Mac and the phone, WebGL2 in a browser,
D3D11 on Windows, Vulkan on Linux and Android, and GL 3.3 on a Linux
box with no Vulkan. A tier that cannot make what a picture needs shows
the picture the CPU draws, through the live road the feeds use.
`cargo test -p bunny-ui-core --features gltf` proves the core on any
machine.*

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
world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());

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
| The axes of a modelling tool | `world.add_axes(100.0)`: x red, y green, z blue, dashed where negative |
| A light | `world.add_light(Light::sun().toward(d))`, `Light::sky(sky, ground)`, `Light::point(warm)` |
| A lamp that lights one room | `entity.set_light_group(1)` on the room's entities, `Light::point(warm).groups(1 << 1)` |
| Light and shade baked on the floor | `world.set_floor_map(Some(FloorMap { picture, origin, size }))`, `floor.set_reads_floor_map(true)` |
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
material and its part. `entity.set_parts(states)` gives each part a
tint of its own or hides it (and its shadow) with no new mesh: the
states ride a table the vertex stage reads. `entity.set_ambient(color)`
tints the light an entity takes from the sky and the ground — the
ambient of the room it stands in.

A model can carry coarser meshes for when it stands small:
`Model::with_lods([(middle, 300.0), (coarse, 120.0)])` draws `middle`
once the model is under 300 pixels tall on the target and `coarse`
under 120. A level holds until the height leaves its step by a tenth,
so a figure that bobs across a step does not flicker. The levels share
the model's materials and skin, and instances at one level still draw
as one.

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

## Only what changed

A target keeps the frame it holds. Each instance of a frame carries a
key of all it shows — its shape, its place, its look, its parts, its
pose — and the rect of pixels it may reach. When the next frame of the
box keeps the eye, the style, the lights and the still shadow, the
tier draws only the rects of what changed: a figure that walks, a clip
that plays, a part that turns red, an entity that comes or goes. The
shadows of what moves add the rects where they may fall. Each rect is
cleared and drawn again with every draw that reaches it, and the rest
of the target keeps its picture. A frame that changes nothing seen
draws nothing. A frame with a bloom, a new eye, or a change past half
of the target is drawn whole.

The CPU proves the rule (`world::damage`): the next frame drawn over
the last in its rects gives the same bytes as the next frame drawn
whole, and each tier proves the same against its own whole picture.
Metal draws rects at one sample: four memoryless samples resolve into
the whole target. The GL family, D3D11 and Vulkan draw them at four
samples too.

## Cameras and the hand

A camera is a plain value: an eye, a target, an up and a lens. A
perspective lens shows far things small; a parallel lens keeps their
size, for a plan or an isometric view. `project` takes a world point to
the box; `ray` takes a point of the box back into the world; the two
agree. `framing` fits a box of the world into view. `fly_to` reaches
another camera along a path that curves around the target; a flight
from one kind of lens to the other is a dolly zoom — the angle closes
as the eye backs away, the target's plane keeps its framing, and the
parallel lens takes over at the end with no jump.

`Orbit` reads the box's events and moves a `State<Camera>`. A drag
turns the view around the surface the press landed on, and that point
stays under the pointer — with the primary button, or with the middle
one, as drawing programs turn; shift pans; the wheel and a pinch zoom
toward the pointer. On a touch screen two fingers pan the point between
them and turn the view as they twist, and a trackpad's twist turns it
too. The pitch and the distance stay inside `OrbitLimits`. A primary
press that never travels comes back IGNORED on its release: that is a
tap, and the caller answers it. While a drag moves the camera, the
pointer is the closed hand (`Cursor::Grabbing`).

A box that answers the press of a button past the primary one holds the
pointer until that button lifts: it hears `ElementEvent::ButtonMoved`
wherever the hand runs, then `ElementEvent::ButtonUp`. Two fingers on a
touch screen arrive as `ElementEvent::Pan` and `ElementEvent::Rotate`,
beside the pinch's `Magnify`.

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
| WebGL2 (browser) | draws worlds itself through the GL family's pass (`gpu::gl_world`): four samples resolved by a blit; `bunny_gpu_world_parity(n)` holds it to the oracle in a page |
| D3D11 (Windows) | draws worlds itself: the vertex shader reads the meshes from structured buffers, four samples resolved by `ResolveSubresource`; the parity tests run on WARP |
| Vulkan (Linux, Android) | draws worlds itself from the GL family's shaders, turned to SPIR-V by a generator that a test holds to their text: four samples resolved in the render pass; the parity tests run on lavapipe |
| GL 3.3 (Linux) | draws worlds itself through the GL family's pass, as WebGL2 does — the road of a box with no Vulkan; the parity tests run on llvmpipe |

A tier that draws worlds keeps a target per box and hands the 2D pass
the target like a feed. A world whose picture did not move is not
drawn again: a repeated frame, a fade, or a frame that changed only the
2D around it costs the GPU nothing for the world. Meshes and textures
go up once per identity and generation, and what no frame reads for a
while is given back.

## The style

`scene_view(world, camera).style(style)` and
`painter.world_styled(rect, world, camera, &style)` take a
`WorldStyle`: the backdrop (none, a colour, a vertical ramp, or a sky
over a ground whose horizon moves as the camera tilts), the
quality, a section plane that cuts the world away beyond it, an
exposure, and `xray` — every surface see-through and every edge on top
of them, the look of an editor over a busy model. An X-ray writes no
depth: its surfaces blend from the farthest to the nearest, and its
edges, hidden or not, show.

A grade (`WorldStyle::grade`) gives the colours a mood — a day's
warmth, a night's blue — after the tone curve. It is a table of
16 × 16 × 16 points from the codes a surface would show to the codes it
shows; `Grade::from_fn` samples a function into one. A colour between
the points blends the four corners of the tetrahedron that holds it, so
a grade costs four reads a pixel at every quality, and the CPU's
picture is graded the same way. Lines and the backdrop keep their own
colours.

```rust
let night = Grade::from_fn(|[r, g, b]| [0.8 * r, 0.85 * g, 0.1 + 0.9 * b]);
let style = WorldStyle { grade: Some(night), ..WorldStyle::default() };
```

A bloom (`WorldStyle::bloom`, from 0 to 1) spills the light of what
glows around it: a lamp's halo, a sign's haze. Only `Quality::High`
draws it. A surface glows as far as its emissive light is strong
(`Material::emissive`: a strength of four glows fully), and the pass
draws that glow beside the colour, one byte a pixel. A chain of small
pictures — four steps down, each half the size, and three back up —
spreads the glowing colours, and the result is laid over the picture as
a screen, `c + s·(1 − c)`: it brightens and never clips, and a pixel
that glows keeps its own look. Every step sums whole codes, so every
tier and the CPU draw the same bloom.

```rust
world.add(Mesh::sphere(0.2), Material::unlit(warm).emissive(warm, 3.0));
let style = WorldStyle { quality: Quality::High, bloom: 0.6, ..WorldStyle::default() };
```

A section cuts the picture in the shaders. `world.section_lines(plane)`
finds the cut itself on the CPU — the outline the plane leaves through
every shown solid, in the world's space — for a drawing to ink over the
open faces with a `LineDepth::Always` style.

## Shadows

A sun casts shadows when it says so:

```rust
world.add_light(Light::sun().toward(vec3(-1.0, -2.0, -1.5)).with_shadows());
```

The first sun of a world is its sun; one sun casts. Every entity whose
`casts_shadow` is true casts with its solid surfaces — opaque and
cut-out ones; see-through and glowing ones let the light through. An
entity out of view casts too: its shadow may fall into the view.

The shadow is one map, fitted to what casts as the sun sees it, and it
is CACHED: its generation hashes the casters, their meshes and the sun,
and nothing else. A camera that pans, turns or zooms under a sun fixed
to the world reads the map it has; the GPU draws the casters again
only when one of them moves or the sun turns. A sun fixed to the view
(`Sun::from_view`) turns with the camera, so a turn draws it again. The
fit's side climbs a ladder of quarter octaves and its centre sits on
its own texel grid: a caster that moves inside the fit moves no other
shadow by a fraction of a texel.

Something that moves by itself casts into a second map of its own: an
entity that walks (`move_to`, its own or a parent's) and a skin that
plays a clip. The still map stays as it is while they move. The second
map holds only what moves in view, fitted to it, and the GPU draws it
again each frame they move: 1024² with one lookup at `Quality::Medium`,
2048² with four at `Quality::High`. A surface reads both maps, and the
sun reaches a point only where both let it.

At `Quality::Low` what moves casts nothing. `Model::blob_shadow(radius,
darkness)` is the cheap road there: a soft dark disc, one see-through
quad that casts nothing, parented to what it follows at the floor's
height.

A surface reads the map at its own point, pushed off along its normal
(further as it turns from the sun) and a step toward the sun, so it
never shades itself. Each lookup compares four texels and blends the
answers, as a comparing sampler does.

## Baked light

Some light never moves, and needs no work a frame. A mesh carries a
baked occlusion a vertex (glTF's `_AO`): how much of the sky reaches
it. A floor map is a picture laid on the ground: its colour is light
added to the surfaces that read it — a room's lamps, baked once — and
its alpha the share of the sky's light that reaches them, the shade a
chair leaves under itself. A surface reads one texel of it a pixel;
off the map it reads nothing, and a surface that does not ask reads
nothing at all.

## Quality

| | Samples | Pixel budget | Shadow map | Shadow lookups | Map of what moves | Bloom |
|---|---|---|---|---|---|---|
| `Quality::Low` | 1 | 1.2 million | 1024² | 1 | — | — |
| `Quality::Medium` (default) | 4 | 2.2 million | 2048² | 4 | 1024², 1 lookup | — |
| `Quality::High` | 4 | 4 million | 2048² | 9 | 2048², 4 lookups | as the style asks |

The style's quality is an ask, and a governor answers it for each box,
never above it. It starts from the name the device's graphics answer to
— a software rasterizer or an older integrated part low, a part of the
middle at the middle, a discrete or newer part high — and it weighs the
beat of the frames while the world moves: a beat that keeps missing the
display's steps (nine frames in ten over one and a half steps) or a run
of long frames steps a quality down; a long stretch with room to spare
steps up, once a session and one step past the start at most. A world
at rest is not weighed: it costs nothing.

A box inside the budget is drawn at its own size. A larger box is drawn
smaller, and the compositor stretches it to the box. At four samples a
cut-out surface (`Material::mask`) turns its edge into coverage — the
cut sharpened to a pixel's width — so leaves and grilles keep a smooth
edge; at one sample it cuts as the CPU does.

## The oracle

The software rasterizer follows the rules a GPU follows: clipping in
homogeneous space, corners snapped to 1/256 of a pixel, the top-left
rule, depth linear on the screen, attributes corrected for perspective,
colour blended as an 8-bit target blends it. Lines are quads of analytic
coverage, pulled a hair toward the eye so an edge wins against its
faces. Where a frame draws edges, each opaque surface also steps back
by its own depth slope, so an edge on a surface that turns away from
the eye stays whole across its width — and never past the far plane.
It draws the sun's map by the same rules, into 16-bit depths, and reads
it as a comparing sampler reads it, with the eight bits of blend a
texture unit keeps. Its shading uses `+ − × ÷`, `sqrt`, `floor` and
fixed tables only: the same frame gives the same bytes on every
machine, and a test pins the fingerprint of one picture.

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

## Skins and clips

A figure's vertices hang from joints. A `Skeleton` is a tree of nodes
in their rest pose; a `Skin` names the nodes a mesh hangs from and the
inverse bind of each; a `Clip` moves the nodes with keys of
translation, rotation and scale, held or blended. glTF brings all three
(`asset.skeleton`, `asset.skins`, `asset.clips`): cubic spline keys are
refused, so sample them at a steady rate. An entity plays a clip, and
every skinned entity below it moves with it — a figure and the clothes
it wears read one pose:

```rust
let figure = world.spawn(&asset);
figure.play(asset.clip("walk").unwrap());   // over and over, from now
```

The pose is worked out on the CPU, once a frame for each entity that
plays, with the four operations and square roots alone: the same bytes
on every machine. Between two keys a rotation turns along the straight
blend of the two, made unit again. Each skinned instance takes its
palette — three rows a joint — and the vertex stage moves each vertex
by its four joints; the shadow pass casts the pose. Instances of one
skinned mesh still draw as one, each with its own palette. A playing
clip asks for frames until it rests; a clip played once holds its last
pose. A skinned mesh is culled by its rest box, grown by half its
longest side each way.

`entity.cross_fade(&clip, seconds)` takes over from the clip that
plays: across the fade the old pose — still moving — blends into the
new one, a walk easing into a stand. `entity.move_to(to, speed)` walks
the entity from where it stands to `to`, `speed` meters a second,
facing its way (its +Z front, as a glTF figure's): its place is a pure
function of the engine clock, so the walk writes the world once and a
box asks for frames until it arrives. Placing the entity stops it.

```rust
figure.cross_fade(asset.clip("walk").unwrap(), 0.25).move_to(vec3(4.0, 0.0, 2.0), 1.3);
```

A figure's regions are its mesh's parts — glTF's `_REGION`, one 16-bit
number a vertex: the skin under a coat, a sleeve, the hair. Each entity
gives each part its own state (`entity.set_parts(states)`): a tint (a
shirt in this figure's colour) or hidden (the skin a coat covers while
it sits, so the two never fight). Hidden parts cast no shadow, and the
states cost no new mesh and no new draw.

A mesh carries up to two morph targets (`MeshData::morphs`, glTF's
`targets`): how far each vertex moves, and how its normal turns, at full
weight. The entity weighs them, each in `[0, 1]`
(`entity.set_morph([0.6, 0.0])`) — a coat that settles on a seat, a
cheek that puffs. A vertex takes its morphs first, then its pose; the
shadow casts both. The weights ride the instance, so instances of one
mesh with their own weights still draw as one.

## Limits

- A world inside a `.looping` box or an overlay island is drawn on the
  CPU: those roads rasterize their boxes. Paint a world on the frame's
  road.
- Labels are painted text. A label that is a view comes later.
- What moves casts no shadow while it is out of view.
- A cut-out surface casts its whole shape: the shadow pass reads no
  texture.
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
# in a page built with the web shell's `3d` feature, from the console:
#   for (let n = 0; n < 21; n++) __bunny.bunny_gpu_world_parity(n)
#   for (let n = 0; n < 4; n++) __bunny.bunny_gpu_world_redraw(n)
BUNNY_WORLD_GLB=chair.glb BUNNY_WORLD_DUMP=/tmp cargo test -p bunny-ui-core --features gltf --lib a_glb_from_the_environment -- --ignored
```
