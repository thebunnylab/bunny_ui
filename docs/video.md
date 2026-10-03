# A video

*Status: the video host — `video(stream)`, the browser's own `<video>`
element playing a media stream the page owns — stands on the three web
roads. The element lowering mounts it as a `<video>` of its own (create
kind 15, op 17 of the patch stream, ABI 11); the canvas shell places it
over the canvas on the CPU and the GPU tier alike, through three host
verbs the glue answers; a page the build painted serializes it at rest.
The core pins it — `a_video_host_places_and_marks_like_a_webview`,
`a_video_creates_once_and_rewires_only_on_a_changed_stream`,
`a_video_host_lowers_to_a_video_that_fills`, the ABI pin and the import
coverage of every glue — and `cargo test -p bunny-ui` is that proof;
`cargo check -p bunny-ui-web --target wasm32-unknown-unknown` is the
shell's. The four native shells refuse it by name and keep the box. A
camera in a real browser has not stood in front of it yet: that
verification is the next step, by hand, and the last section says what
to watch.*

A web app that shows a WebRTC call has the browser's compositor on one
side and the framework's canvas on the other, and the picture has to
cross. Today it crosses the long way: the page draws the remote
`<video>` into a canvas 320 pixels wide, reads the RGBA back, hands the
bytes to the engine and uploads them again — every frame. At 320 by 180
a frame is 230 KB; at thirty frames a second that is 6.9 MB a second
read out of the compositor and written back into it, per feed, for a
picture that is then stretched over a box six hundred points wide. The
readback stalls the GPU, the upload costs a texture a frame, and the
decode the browser already did in hardware is thrown away for a copy.

The right road on the web is the browser's own `<video>` element,
playing the `MediaStream` directly: hardware decode, the compositor's
own scaling, no readback and no upload. The framework holds the element
the way it holds a webview — as a native host (`docs/webview.md`), a
box the layout keeps and the platform fills.

## The door

```rust
video(stream)
    .mirrored()                           // the selfie: flipped left for right
    .aspect_ratio(ContentMode::Fill)      // cover (the default); Fit letterboxes
    .corner_radius(12.0)                  // the browser cuts the corners
    .frame(600.0, 338.0)
```

`video(stream)` is a view like any other. It fills what the parent
proposes and a `.frame(…)` pins it, as a webview does — a feed has no
natural size of its own. `.mirrored()` flips the picture the way a
mirror does (`transform: scaleX(-1)`): what a preview of oneself wants,
and what a peer's picture does not. `.aspect_ratio(…)` says how the
picture meets a box of another shape — `Fill` covers the box and crops
what does not fit (`object-fit: cover`), `Fit` keeps the whole picture
and letterboxes (`contain`); the box never follows the picture.
`.corner_radius(…)` rounds the corners on the element itself. This door
and not the general one, because the general one rounds a background
the framework paints — and the framework paints nothing here: a
platform view is clipped only by its own element.

The element is always `muted autoplay playsinline`. Audio is the app's
business — a call plays its sound on an element of its own, or not at
all through this door — and a muted element is the one every browser
lets play without a gesture.

## The stream never enters wasm

A `MediaStream` is a browser object; there is no number for it, and the
engine has no use for one. The page keeps the stream, and the engine
names it — by `MediaHandle`, the integer the glue's registry answered
when the page registered it.

On a page the classic glue boots, `media.js` loads before `glue.js` (or
`glue_dom.js`), the way `surface.js` does, and holds the registry:

```html
<script src="media.js"></script>
<script src="surface.js"></script>
<script src="glue.js"></script>
```

```js
pc.ontrack = (event) => {
  const handle = bunnyMedia.register(event.streams[0]);   // a u32, from 1
  window.__bunny.atrium_stream(handle);                   // the app's own export
};
// …when the track ends
bunnyMedia.release(handle);
```

On a page another loader boots — wasm-bindgen's — the ES module is the
same door: `import { registerMediaStream, releaseMediaStream } from
"./bunny.js"`.

The app declares the export and keeps the number in state. The export
takes the road the finder's fetch answer takes (`finder_fetched`): a
`task::channel` whose receiver a `.task` on the view awaits, so the
state changes on the engine's own turn and the frame follows the wake.

```rust
#[unsafe(no_mangle)]
pub extern "C" fn atrium_stream(handle: u32) {
    let _ = FEEDS.with(|feeds| feeds.borrow().as_ref().map(|feeds| feeds.send(handle)));
}

// …in the view
video(MediaHandle(self.remote.get()))
```

`MediaHandle(0)` names no stream: the element stays black until the
handle arrives. The handle is the only thing that crosses. The glue
finds the stream in the registry when it places the element and wires
it ONCE — a changed handle rewires; a changed flag (the mirror, the
fit, the radius) never touches `srcObject`, because writing the same
stream again restarts the playback. Releasing a handle forgets the
name; it stops no track, and an element still showing the stream keeps
it until its host leaves the scene. The page owns the stream from
`getUserMedia` or `ontrack` to `track.stop()`.

## Three roads, one element

In the element lowering the host IS an element: a `<video>` the diff
creates once, hungry on both axes like the iframe, with its whole
record — stream, mirror, fit, radius — in one patch. The mirror rides
the `scale` property there and not `transform`, which the layout
record resets whenever the box changes. A page the build painted
serializes the same element at rest, with a stream of zero: a handle
is minted per page session and never survives a build.

On the canvas roads the engine draws no pixel for the box, and the
page puts the element ABOVE the canvas: an overlay `div` inside `#app`,
after the surface, `pointer-events: none`. Each present the shell walks
the hosts of the last layout and speaks three verbs — `js_host_begin`,
one `js_host_video` per video host, `js_host_end` — the mac's
`host_place` and `host_sweep` discipline with the glue holding the
elements. The glue keeps one clip `div` per host on the window the
layout granted, holding the element at the whole box: the mac's
container and tenant, in CSS. A feed half scrolled off is cut, never
rescaled; a feed clipped away entirely is hidden, never unmounted, and
its playback keeps its state; a host that left the scene is swept at
`end`. A stamp of the record gates the attribute writes, and the
stream is rewired only when the handle changed. The overlay survives a
tier swap: the surface door keeps whatever carries `data-bunny-keep`
above the new canvas, so the first CPU blit after a GPU refusal does
not take the picture with it. Coordinates are logical points, which on
this page are CSS pixels.

## The z-order law

On the web the element sits above the canvas, and that is the whole
law: anything the scene paints AFTER the host in the display list is
under it. The desktops lift that tail onto a segment surface over the
platform view (`docs/webview.md`); this page does not yet — a web
sandwich is a follow-up. So the consumer keeps badges, names and
controls OUTSIDE the video's rectangle, or in a DOM layer of its own
above the page. Clipping to the window and the rounded corners are CSS,
on the element. Pointer events stay with the canvas: the overlay lets
them through, and a click on the picture reaches the box under it. In
the element lowering the host is an element among elements, and what
the scene lowers after it stacks above it in the browser's own order —
there a badge over the feed costs nothing.

## Capabilities

| | DOM flow | canvas, CPU | canvas, GPU | macOS | iOS | Windows | Linux |
| -- | -- | -- | -- | -- | -- | -- | -- |
| `video(stream)` | a `<video>` element | overlay element | overlay element | no¹ | no¹ | no¹ | no¹ |
| mirror, fit, radius | CSS on the element | CSS on the element | CSS on the element | — | — | — | — |
| clipped to the window | the browser's flow | clip `div` | clip `div` | — | — | — | — |
| the scene painted after it | above the element | under the element | under the element | — | — | — | — |
| pointer events | the box under it | the canvas | the canvas | — | — | — | — |

¹ The box stays reserved and empty, exactly as the layout placed it,
and the console says so once: `bunny_ui: a video host is the web's;
the box stays empty here`. A native camera view is another tenant, for
another day.

## The proof

```bash
cargo test -p bunny-ui                                        # placement, patch, lowering, ABI 11, glue coverage
cargo check -p bunny-ui-web --target wasm32-unknown-unknown   # the shell: canvas and the GPU tier
cargo check -p bunny-ui-web --target wasm32-unknown-unknown --no-default-features --features canvas
cargo check -p bunny-ui-macos                                 # the refusals compile — and the other three shells
```

What stands unproven here, by name: a camera in a browser. The page is
run with a `getUserMedia` stream registered through `media.js`, and
three things are watched. The first frame shows the picture — the
overlay survived the first blit. A `.mirrored()` toggle leaves the
playback running — the stamp gated the rewire. A feed scrolled half
away is cut straight — the clip `div` did its work.

## What this is not

Not a video player: no controls, no seek, no audio — a stream in a
box. Not a frame grabber: the pixels never come back; an app that
needs them (a snapshot, an effect) reads the `<video>` itself, on the
page. Not a native camera view: the four native shells keep the box
and say so.
