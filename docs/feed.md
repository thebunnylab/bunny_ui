# A feed

*Status: the feed door is standing on every tier — Metal, GL, Vulkan,
WebGL2, D3D11 and the CPU oracle — and on Metal a frame the GPU already
holds (a camera's or a decoder's texture, RGBA or BGRA) rides the same
linear road with no copy at all. One texture per feed, sized to the
picture, replaced in place when the generation moves, scaled into its
box by a linear sampler; the shared atlas never hears of it and the
collector is never asked. `cargo test -p bunny-ui-core --features gpu --lib
gpu::walk` proves the allocator on any machine, `cargo test -p
bunny-ui-apple --lib metal` proves the pixels against the oracle on a
Mac, and the WebGL2 tier proves itself at install with a feed in its
probe. The D3D11 half is type-checked from here and runs its parity
tests on a Windows box.*

A camera, a decoded video, a chart that redraws itself: a picture
whose BYTES change every frame and whose identity does not. The image
door was built for pictures that hold still — it resamples once, caches
by identity and uploads once — and a picture that moves broke every one
of those bets at once. This page says what it cost, what the feed does
instead, and the law that makes a frame cheap.

## What a moving picture cost

An app that pushed each frame through `ImageSource::rgba` with a new key
paid, per frame and per tier:

- A resample on the CPU, nearest-neighbour, to the DESTINATION's size.
  A 640×360 frame in a 600 pt box at scale 2 became 2133×1200 pixels of
  blocky upscale before any GPU saw it.
- A new GPU texture. The identity moved, so the cache missed, and the
  frame minted a dedicated texture of the destination's size.
- The collector. Stale textures piled up to the retention cap; the next
  frame asked the collector, the presenter drained every frame in
  flight, the atlas reset, and every text run on screen rasterized
  again. Every eight frames, with one camera on screen.

The picture was late, the picture was blocky, and the text paid for it.

## The door

```rust
let camera = ImageFeed::new();            // once, in state
camera.push((640, 360), rgba);            // per frame: one Rc move, no hash
image(&camera).resizable().aspect_ratio(ContentMode::Fill)
painter.image(rect, &camera);             // the same door from a custom box
```

`image(…)` and `Painter::image(…)` take anything that turns into an
`ImageSource`; a feed does. An empty feed paints nothing. An app that
numbers its own slots builds the source itself with
`ImageSource::feed(FeedKey::new(slot), generation, size, rgba)`.

## The law: identity and generation

A feed's `key` is its SLOT — the texture a tier keeps for it. Its
`generation` climbs with every `push`. The two answer two different
questions:

| Question | Answer |
|---|---|
| Is this the same picture? (the texture to reuse) | the slot |
| Did the picture change? (the damage, the upload) | the generation |

So `ImageSource::key()` returns the slot, and equality compares the slot
AND the generation. A new frame is damage to the CPU diff and to the
Metal skip key; the same slot is the same texture to every ground. A
veil over a feed (`.opacity(…)`) folds the generation into the veil's
own identity, so the faded cache never shows a stale frame.

## What each tier does

| Tier | The texture | The upload | The scale |
|---|---|---|---|
| Metal | private storage, one per slot | a staging buffer per ring slot, copied by a blit encoder at the head of the frame — ordered after every frame still sampling | `live_fragment`, a linear sampler in pixel coordinates |
| GL (Linux) | one per slot, `GL_LINEAR` | `texSubImage2D`, whole | `LIVE_FRAG_BODY` |
| Vulkan | one per slot, the linear sampler in its descriptor set | the recorder's own staging round trip, SHADER_READ → TRANSFER_DST → SHADER_READ | `live.frag` |
| WebGL2 | one per slot, `GL_LINEAR` | `texSubImage2D`, whole | `LIVE_FRAG_BODY` |
| D3D11 | one per slot, `USAGE_DEFAULT` | `UpdateSubresource`, whole | `live_fragment` through the stack's one (linear) sampler |
| CPU oracle | — | — | bilinear, in 16.16 fixed point, cached per slot and size |
| DOM flow | an island | — | the island's own road |

The sprite instance carries the picture's extent in `tex` and the box
in `dest`; the shader scales from the pixel's centre in the box to its
place in the picture, and the sampler reads it linear, clamped at the
edge. At 1:1 the oracle is a copy and the GPU reads texel centres, so
the two agree byte for byte; scaled, they agree within a few steps
(`assert_filtered_close`).

Why Metal stages: `replaceRegion` is a CPU write with no hazard tracking
against the command buffers still in flight, and the ring keeps three.
The bytes go into the slot's own staging buffer — free, because the slot
was acquired after its last frame completed — and the frame's command
buffer copies them into the texture before it draws. GL, WebGL and
D3D11 order the write themselves; Vulkan records the barriers it always
did.

Metal keeps staged copies pending until their command is committed. If a
drawable or upload encoder cannot be obtained, the next walk invalidates
those feed stamps and retries the same generation. Committed copies carry
only texture/generation metadata in the existing ring slot. A terminal
command error invalidates the matching generation and the presenter's
retained-frame shortcuts; a later generation and unrelated feeds stay valid.
The next requested frame performs the retry. This does not schedule a new
frame on its own or change the successful upload/render strategy.

The recovery tests use actual Metal pixels, including a real nil drawable,
plus injected command-error outcomes. An injected outcome is not a physical
GPU failure, and successful upload recovery is not proof of display timing.

## Lifetime

- A feed's texture is minted on first sight and kept while frames read
  it. A size change mints a new one and gives the old one back.
- `LIVE_KEEP` (16) feeds stay warm; past it, the slot nobody read this
  walk and idle the longest makes room. A frame that reads more keeps
  every one — a feed never fails a frame.
- `LIVE_IDLE_WALKS` (120) walks unread, and the texture is given back
  — a tile that scrolled away, a call that ended.
- A reset of the atlas leaves the feeds standing (their textures are
  their own, written in place, never in virgin space) and marks them
  `stale`: the next walk uploads their bytes again. A tier whose reset
  threw the frame's uploads away — a Vulkan staging arena that grew —
  is made whole by it.

## A frame the GPU already holds (Metal)

A camera's pixel buffer, a hardware decoder's output, a renderer's own
texture: on the Mac and on iOS these already live on the GPU, and the
feed's upload would be a copy for nothing. `bunny_ui_apple::surface`
wraps such a texture as a frame the compositor samples where it lies:

```rust
// the app's own CoreVideo bindings mint the texture from the pixel buffer
let frame = unsafe { MetalFrame::from_texture(texture, Arc::new(lease)) }?;
image(frame.image()).resizable().aspect_ratio(ContentMode::Fill)
```

- The texture is 2D, one level, shader-readable, `RGBA8Unorm` or
  `BGRA8Unorm`; anything else is refused by name. Metal reads every
  ordered format in r, g, b, a, so BGRA needs no view and no swizzle.
  The sRGB twins are refused: a decode on read would land in linear
  light inside a gamma-space compositor.
- The frame retains the texture once and keeps `lease` — a pixel buffer,
  a cache entry — alive until the last command buffer that sampled it
  completed. The lease is not optional: a pixel buffer released while
  its texture is in flight is a torn or black frame.
- A frame rides the live pipeline: the picture's own size, scaled into
  its box by the linear sampler, like a feed. The two paint the same
  bytes (`a_native_frame_samples_like_a_feed`).
- The wgpu pool (`SurfacePool`, feature `wgpu-surface`) copies a
  completed wgpu texture GPU to GPU into a leased frame through the same
  door, with backpressure at twelve frames.
- Every other tier paints nothing for a native frame; a consumer hands
  them a feed instead.

## Limits

- One format: straight RGBA8. A planar layout (NV12, I420) would be a
  variant of `PixelFormat`, an upload arm per ground and a conversion
  shader; the enum is open for it, and nothing here would change shape.
- A veil over a feed rides the CPU road: the faded bytes are an image
  of their own, resampled and uploaded per generation. Fade the box,
  not the picture, when the picture moves.
- On a CPU tier a feed scaled to a large box costs its bilinear, once
  per generation and size.
- A feed drawn at several sizes on one screen keeps one texture and
  one raster per size.

## Proof

```
cargo test -p bunny-ui-core --features gpu --lib gpu::walk
cargo test -p bunny-ui-core --lib image_engine
cargo test -p bunny-ui-apple --lib metal
cargo test -p bunny-ui-apple --lib surface
cargo test -p bunny-ui-apple --features wgpu-surface --lib surface
cargo test -p bunny-ui-vulkan --lib the_committed_spirv_matches_its_source
cargo check -p bunny-ui-windows --target x86_64-pc-windows-msvc --tests
cargo check -p bunny-ui-web --target wasm32-unknown-unknown
```

## Observing native presentation

`ImageFeed::generation()` counts pushes. It does not count frames displayed by
an operating system. On desktop shells, opt in after opening a window:

```rust,ignore
let observer = app.observe_feed(window, camera.key(), 64).expect("open window");
// Poll from an existing task; observation itself starts no animation or timer.
for event in observer.drain() {
    println!("session={} feed={:?} {event:?}", observer.session(), observer.feed());
}
let state = observer.snapshot();
```

One observer selects one feed in one top-level window. Attaching another closes
the previous subscription. The session ID is process-local, never a native
pointer, and is unique even when a window address is reused. The same feed in
two windows needs two observers. Attachment is not retroactive: an unchanged
frame already on the window does not acquire a receipt just because an observer
was added. Dropping the observer disables tracking; replacing it or closing the
window invalidates pending callbacks. Observers and callback blocks retain only
scalar metadata and weak queue references, not image bytes or native windows.

Events deliberately distinguish stages:

- `Submitted` means the renderer encountered a new feed snapshot. It does not
  report capture/decode or every `push` that happened before a scene was drawn.
- `Superseded` means a newer submitted generation replaced a snapshot before it
  acquired a native commit ticket. Generations overwritten between pushes can
  be absent from the event stream entirely; they never receive presentation credit.
- `Committed` means a native drawable encoded a matching live texture. It is
  neither a GPU-completion timestamp nor proof of presentation.
- `Presented` requires both successful command completion and the drawable's
  presented callback with a finite positive native timestamp. One generation
  earns at most one receipt per subscription, even through repeated redraws.
- `RenderFailed` and `Unconfirmed` do not count. A zero/invalid native timestamp
  does not explain whether the frame was skipped, dropped, or never visible.
- `Unavailable` names an unsupported path or current limitation. `Closed` ends
  the subscription. Neither a timeout nor an unavailable state invents a drop.

The event queue holds 1–256 events (the requested capacity is clamped). If a
consumer falls behind, the oldest event is evicted and `lost_events` increases.
Three native generation attempts may be pending; saturation reports
`Backpressure` and refuses additional observation tickets, without changing
rendering or pretending to know which frame was dropped. Completed receipts
can arrive out of timestamp order. `snapshot().presented` is a cumulative count
of confirmed unique generations, including evicted receipts; it is **not FPS**.
Queue loss, unavailable intervals, or an incomplete observation window preclude
a complete cadence measurement. Manual non-monotonic feed generations and
multiple conflicting snapshots of one feed in a frame are unsupported.

### Current platform boundary

The implemented timestamp source is macOS Metal's
[`MTLDrawable.addPresentedHandler`](https://developer.apple.com/documentation/metal/mtldrawable/addpresentedhandler(_:))
and [`presentedTime`](https://developer.apple.com/documentation/metal/mtldrawable/presentedtime),
with runtime selector checks (available on macOS 10.15.4 and later). Time is in
Metal's host-time seconds, not Unix time or a Rust `Instant`; `drawableID` belongs
to its layer and must be associated with the observer session. A successful
`MTLCommandBuffer` completion is an additional validity gate, never the receipt
source. No elapsed-time resource-release heuristic issues receipts.

Only whole-frame Metal drawables with a clipped-in live RGBA feed draw qualify
for that path. The final atlas generation must match after upload retries;
stale or failed uploads cannot borrow the requested generation's identity.
The receipt's own command must also encode a complete upload of that texture
through valid blit and paint encoders. Merely reusing a cached generation is
`NoCurrentUpload`: its earlier command may have failed or been abandoned before
submission. Observation does not force a reupload to manufacture a receipt.
Native bands, CPU drawing, glass passes, partial or reused patches, faded/mirrored feeds,
native-surface imports, and Windows/Wayland/X11 presentation currently report
unavailable. Enabling observation preserves the selected rendering strategy.
Hidden or AppKit-occluded windows suspend tickets, including callbacks racing
with encoding. A later visible render can start a fresh attempt.

A Metal receipt describes a drawable presentation reported by the platform. It
does not prove every feed pixel was unobscured by later drawing, other windows,
or compositor layers; it does not measure photons or input-to-photon latency.
The native fixture in `crates/bunny_ui_macos/tests/feed_receipts.py` compares
synthetic frame IDs against independently captured window pixels and refuses
visible qualification in a locked or unknown session. Window captures alone,
headless tests, and a wired callback do not establish a qualified presented-FPS
measurement. Camera/native-surface paths and each other desktop backend require
independent validation before an application labels a counter “Presented FPS.”
