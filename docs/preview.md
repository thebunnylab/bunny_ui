# Preview host and negotiated editing

`#[preview]` registers ordinary view constructors for the native preview host.
The host lists them in `Ready`, builds selected scenes with the requested traits,
and streams rendered frames and source-site rectangles. The application still
runs without a designer. Each selected scene owns its runtime; failures report
against that scene and retain the host for other scenes.

The host connects to the tool's loopback port, sends `Hello` with the framework
version, workspace root and capabilities, then `Ready`. The preview wire remains
version 1. Messages have a length prefix and kind byte: unknown message kinds can
be skipped in full, but unknown tags inside a known payload are malformed.
This wire is independent of the browser DOM ABI.

## Live placement

`Hello.caps & caps::PLACE != 0` announces free form, grid and message kind 79,
`ToHost::Place { id, site, op, patch }`. Its patches are:

- `PlacePatch::Position(Point)`: two physical f64 coordinates.
- `PlacePatch::Span(u8, u8)`: column/row counts for live scrubs. Zero normalizes
  to one. Larger counts remain available in ordinary Rust but cannot be encoded
  by this live patch.

The payload shares the existing scene id, source-site key and operation semantics:
Replace replaces a modifier at its site, Append adds one, Hide hides that site.
The host converts wire patches when received and retains the core override.
Repeated Replace/Hide operations supersede each other. Appends supersede only
an append of the same modifier name, so position, span and style overrides can
coexist. Clearing overrides restores the source definition.

The tool must negotiate this capability before sending Place or landing Rust
that uses `.position`, `.span` or `grid!`. An older host keeps individual previews
and the old inspector available; free form and grid stay disabled with an
explanation that the project needs a newer Bunny UI pin. The IDE integration is
owned by the Studio capability gate, not inferred from a version string.

The 23 tags of `Patch` in message kind 71 remain frozen. Touch (77), Magnify (78)
and Place (79) extend the wire by new message kinds. The golden stream appends
placement cases after every existing byte. `wire_v1_before_place.bin` pins that
prefix independently, so regenerating the current golden cannot silently alter
legacy payloads.

## The whole app

A native build with `preview` makes `app!` export the reserved `app` entry.
The host lists it first, under **App**, at the window size set by `AppConfig`.
Its source reference is the `app!` invocation. It runs the real root and state
under the same scene boundary, panic reporting, input and hot-reload path as
marked previews; it opens no platform window. `app` cannot name a marked or
manually declared preview.

`bunny preview` with no names includes the app. `bunny preview --png pictures`
also writes `pictures/app.png`, preserving the configured window size unless
`--size` overrides it. `--list` and `--list --json` discover the app without
building; JSON adds an optional `app` source site, leaving older records intact.
Macro bodies and strings containing sample invocations are not app entries.
Duplicate invocations are reported with both source sites.

Window chrome and menus are not part of the preview image. A panic while
constructing the root during discovery is reported by name without ending the
host; its row remains available for repair, with default traits until the root
can be constructed again.

The App export is retained when the app lives in a library and the binary only
calls `run()`. `BUNNY_PREVIEW_CHECK` fails for an unresolved entry; rendering a
diagnostic string is not a successful preview check.

## Build-source provenance

The in-process CLI lane emits `Event::Sources` after a successful build/load,
with the generation number in the hot runtime's `loaded <n> <ms>` acknowledgment.
This does not change any v1 wire payload. A tool must correlate frames by that
acknowledged generation, not by the arrival time of `BuildStarted` or `Reloaded`.
Queued frames may still belong to the previous generation.

`BuildInputs` observes package Rust sources, including executable and nested
component files, together with manifests/build scripts before and after the
build. Changed metadata/content, added/removed files, unreadable inputs or source
symlinks leave the proof unavailable. Reloads also require the startup text of
inputs that only a restart can replace. `Sources { sources: None, .. }` means
unknown provenance; the tool keeps the last-good picture but must not infer its
source from whatever happens to be in the editor now. Build-generated files
outside the captured roots are not attested. These observations assume ordinary
filesystem modification tracking; they are not a hermetic compiler input record.

An IDE should fingerprint the observed text once, attach the receipt to each
received frame, and compare it to current documents before geometry-driven edits.
Undo restoring the exact source can restore eligibility. A restart may reuse
generation numbers, so retained pictures must keep their own receipts rather
than borrowing the new process's proof.

## Independent scene instances (T2-STUDIO-79, T2-STUDIO-80)

`Hello.caps & caps::INSTANCES != 0` enables `SelectInstances` (80) and
`Instance` (81). An instance has a validated opaque `SceneInstanceId`, a source
preview ident and rendering traits. Reordering selections keeps the same runtime;
removing and recreating an instance creates fresh state. Duplicate identities or
unexported previews refuse the whole selection before replacing any runtime.
Legacy `Select` still selects the original single-preview lane. Its commands
cannot alias an instance whose identity happens to have the same spelling.

`Select` replaces only the legacy scenes; `SelectInstances` replaces only the
instance scenes. Both selections coexist in one host. Clearing or changing the
Code dock therefore retains every artboard runtime, and closing page scenes
retains the dock. Neither lane can address the other by a matching string.

`Instance` addresses one instance and variant, carrying traits, dimensions,
pointer/wheel/key/text/touch/pinch input, style/placement overrides, clear or
snapshot. A traits command changes that variant; `SelectInstances` changes the
number of variants. Replies use new kinds: `InstanceFrame` (10),
`InstancePanicked` (11), `InstanceSnapshot` (12) and `InstanceError` (13). Shared
image resources remain keyed by content. Existing payloads, version-1 goldens and
legacy host behavior remain unchanged. Tools must retain the old lane for hosts
without the capability and explain why instance composition is unavailable.

Both input lanes offer strokes to the runtime's custom handlers before applying
ordinary field edits. `Text` carries committed Unicode text; `Key` carries
navigation, deletion, selection and submission, without inserting printable key
names a second time. Command+A and Control+A select the focused field's text.
Enter and vertical arrows follow the runtime's single/multiline field policy.
The host never reads or writes the system clipboard on behalf of a key message.

Each instance variant owns an immutable `provenance::OverrideSet`. Rendering,
input and dirty checks enter its context and restore the outer context, even on
panic. A set retains its own version across passes, so another artboard does not
invalidate its layout cache. The legacy thread-wide override table remains the
legacy lane. A delayed snapshot holds the runtime identity, preventing a removed
artboard's request from attaching to a recreated instance. A generation change
invalidates the last-sent frame even when its pixels are identical, allowing tools
to associate the new source receipt with every retained instance.

### Basic shape authoring capability

Hosts offering `ellipse()` and inherited foreground ink for `rectangle()` announce
`caps::SHAPES` (`4`) in `Hello.caps`. An editor gates source insertion on this bit.
The ellipse uses the existing vector resource and image draw, so no frozen draw,
resource or message payload changes. Source sites name its builder `ellipse`.
See [Basic shapes](shapes.md) for layout, ink and responsive DOM behavior.

### Mount actions in application components

`ViewExt::on_mount` runs once per call site and view identity, independently for
each preview scene instance. Rebuilding or reordering the same identity does not
repeat it; removing and later mounting that identity runs it again. Helpers that
emit multiple actions at one source site use `on_mount_keyed` with distinct keys.
The ordinary application runtime owns these retained effect slots; Studio is not
required. `on_appear` retains its existing render-time behavior for compatibility.
The preview host advertises `Hello.caps & MOUNT` (bit 8), so older projects can
keep previewing while the designer withholds unsupported mount insertion. No wire
message or existing capability changes. The designer's Mount action emits
`on_mount`, while legacy appear callbacks remain
explicitly identified by their different behavior.

### Design and Play execution contexts

`Hello.caps & caps::EXECUTION != 0` (bit 16) enables `SetExecution` (kind 82),
addressed by `SceneInstanceId` and variant with `Design` or `Play`. It does not
change a frozen selection, command, frame or version-1 payload. Tools send it only
after negotiation; older hosts retain the individual preview lane and cannot
promise automatic fixture/live switching. An absent instance or variant returns
`InstanceError`. Repeating the current mode is inert and only a changed scene is
marked dirty. Rendering traits and provisional overrides remain independent.

Each scene owns a `preview::ExecutionScope`, initially Design. Preview discovery
uses a temporary Design scope which closes immediately after reading the traits.
Construction, layout/mount, dirty checks and input run under the addressed
context. Reordering or changing traits preserves it; removing a scene, replacing
a failed runtime or dropping the host closes the old context. Recreating the
same identity opens a fresh Design context. Light/dark variants are independent.

Application adapters capture `preview::ExecutionContext::current()` when their
ViewModel is constructed, read `mode()` for the initial state and retain an
`ExecutionSubscription` from `subscribe` to observe subsequent changes.
`Some(Design)` means fixtures, `Some(Play)` permits the application's configured
live services, and `None` means the owner has closed: cancel work and reject late
results. The framework supplies no HTTP client, executor, credentials or service
model. Standalone applications may own an explicit scope around their own runtime.

Captured handles do not extend the owner's lifetime. A scoped `with` call restores
the previous thread-local context on return and unwind, and does not revive a
closed handle. Async tasks must capture their context explicitly; global task
polling does not infer a scene. Notifications run under their own context with
no registry borrow, permit detach/reentry, and skip obsolete intermediate states
when another listener changes the mode. They observe state rather than replaying
an event log. Adapter callbacks must not oscillate modes indefinitely. A failed
listener does not prevent the others from observing closure; its panic resumes
after notification, except during an existing unwind, which preserves the
original failure. Reading a mode alone is not a reactive dependency: subscriptions
must project changes into the application's own state when the view depends on it.

### Initial execution selection

`caps::EXECUTION_SELECTION` (bit 32) enables `SelectExecutions` (kind 83). Each
selection batch carries an opaque request id, and each `ExecutionSelection` contains
an ordinary `InstanceSelection` and its initial
Design/Play intent. The host validates the whole list before changing a scene.
A new scene's factory and mount callbacks run under the requested intent from
their first invocation. Changing intent closes the old scope and remounts that
instance's variants; unchanged selections and reordering keep their state.

Remount retains logical resize and per-variant overrides, and advances the
existing frame sequence so an IDE cannot reuse an image from the previous mode.
Other instances and the legacy lane remain independent. The existing
`SelectInstances` retains a standing scene's mode and starts new scenes in Design;
`SetExecution` still changes intent without remounting. Their frozen payloads and
behavior do not change. Tools needing mount-time live services negotiate this
new capability instead of racing a selection against a later mode message.

After applying the complete selection, the host emits `ExecutionsSelected`
(kind 14) with that request id, before rendering any frames for the new selection.
Failed preflight emits no acknowledgement. This ordered stream boundary lets an
IDE reject old frames already in transit, ignore superseded acknowledgements,
and wait for a new addressed frame before enabling input on a remounted scene.
Every accepted selection republishes the selected frames (or their explicit scene errors),
including retained runtimes, without replaying mount effects. This also settles a
rapid mode change/revert when intermediate frames were discarded. Clients cache
unchanged selections rather than sending them on every UI frame. The last-good
image may remain visible while that boundary is pending.
