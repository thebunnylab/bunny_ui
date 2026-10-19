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

## Image modifiers with source recording

Recorded source sites remain transparent to image and icon modifiers. A
resizable image and its Fit/Fill mode use the same geometry as the standalone
application, including when visual wrappers precede the modifier. Rewriting the
pixel leaf retains its source-site metadata and does not traverse an overlay
layer or bypass existing action/element-hint boundaries.

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

Text paste is an explicit, negotiated shell operation. `caps::CLIPBOARD_TEXT`
(256) enables `Paste` (kind86) for legacy scenes and the additive `InstanceCommand`
tag12 for an exact instance/variant. Both carry the original key/modifiers and
one complete UTF-8 clipboard snapshot. The tool captures it only on an explicit
Command/Control+V gesture; the host offers that shortcut to application handlers
first. If unclaimed, `Runtime::paste_text` replaces the focused selection as one
history step, separate from surrounding typing and consecutive pastes. The host
never consults its own clipboard. Modified/non-paste shortcuts are refused by
this door. Existing `Key`/`Text` bytes and behavior stay frozen; incapable hosts
must not receive the new command, and tools must explain the update requirement.


Each instance variant owns an immutable `provenance::OverrideSet`. Rendering,
input and dirty checks enter its context and restore the outer context, even on
panic. A set retains its own version across passes, so another artboard does not
invalidate its layout cache. The legacy thread-wide override table remains the
legacy lane. A delayed snapshot holds the runtime identity, preventing a removed
artboard's request from attaching to a recreated instance. A generation change
invalidates the last-sent frame even when its pixels are identical, allowing tools
to associate the new source receipt with every retained instance.

### Read-only state observation foundation

`Runtime::sites()` now attaches an exact `ReadBinding` to a reactive text site.
`reading.observe()` consults the retained read graph and actual `State` storage;
it never evaluates the text closure, clones an application value or invokes its
formatting. A containing stack or modifier does not inherit child dependencies.
The token expires when recording stops, its runtime drops or that exact binding
is retired/replaced, including replacement at the same structural path.

State allocation retains its original `#[track_caller]` location, dependency
identity and write version. Revived state keeps its original creation location;
a recycled slot receives a new identity. Integers preserve all 128 bits, floats
preserve their bit patterns, and booleans, characters and standard string types
have built-in inspection. Other types are explicitly opaque and require no
`Debug` or `Serialize` bound. Strings above 4096 bytes are reported as oversized,
never truncated. A request above 512 dependencies or borrowed storage refuses
the complete snapshot; a value temporarily owned by `update` is marked updating.
Store dependencies remain explicitly unsupported.

These are current execution facts, not reconstructed source expressions. The
constructor caller is not necessarily a model field or the whole initialization
expression. A text can read several states; multiple texts can share one state.
No runtime setter or source edit is exposed.

### Selected state observation protocol

Hosts advertise `caps::STATE_OBSERVATIONS` (128). After enabling `ObserveViews`,
a client sends `ObserveState` (kind 85) with a nonzero, monotonically increasing
request and a target: scene instance (or the legacy lane), preview identifier,
variant, generation and mounted view identity. `target: None` cancels. Only one
watch exists per connection. A newer request replaces it even if the new target
is invalid; an old request cannot clear a newer watch. A legacy target matching
multiple scene aliases is explicitly ambiguous, never the first match.

`StateObservation` (kind 16) distinguishes a frame-bound snapshot from a stopped
or refused request. An active watch publishes `StateSnapshot`, `ViewSnapshot`,
then `Frame`/`InstanceFrame`, preserving the existing view/frame adjacency and
all frozen version-1 bytes. Values are sampled after layout. The selected
binding is also polled through its read-only lifetime token: a storage/version
change publishes a new frame even when formatted text and paint are identical.
Unselected nodes are not inspected. Remount, removal, panic, generation changes
and disabling view observation revoke the watch explicitly.

`StateSnapshot::match_views` requires validated view/frame metadata and the
client's current request and target. It verifies the exact publication and
mounted occurrence, unique dependencies and bounds. Consumers must additionally
fence the host connection and compiled-source freshness; a proof from an old
connection is unusable in a new one. The wire carries no source-edit permission.
Bounds are checked before decoding allocations: at most 512 dependencies and
4096 UTF-8 bytes per value/metadata string. Unsupported, missing, retired,
borrowed, updating and oversized outcomes remain distinct; no truncation or
application callbacks are used. Integers retain 128 bits and floats travel as
IEEE-754 bits, including signed zero and NaN payloads.

Older peers receive no state messages without opting in; clients must show an
unavailable capability when a host does not advertise it. Native Studio session,
connection/source fencing, inspector and initializer controls remain integration
work under T2-STUDIO-128.

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

## Waking an embedded preview lane

`bunny_cli::preview::Preview<Live>::set_waker` installs an optional thread-safe
notification after wire messages are queued or the reader closes. Registration
also rings once, covering messages received before installation. The callback
runs outside the registration lock and should only signal the owner's event
loop; `poll`/`wait` still own ordered message delivery. Spurious notifications
are allowed. Replacing a hook does not wait for one already in flight. A
successful restart retains the hook; polling clients and wire bytes are unchanged.

## Mounted view identities (T2-STUDIO-119)

`Hello.caps & caps::VIEW_IDENTITIES != 0` offers separate mounted identities.
After `ObserveViews { enabled: true }` (84), `ViewSnapshot` (15) immediately
precedes each published frame. It carries optional SceneInstanceId, preview,
variant, generation, seq and site-index-to-optional-nonzero-ID records. Enabling
observation forces a fresh pair. Identity changes publish even if paint is
unchanged. Disabling observation stops metadata. Old peers retain the same
Frame/SiteBox bytes and wire-v1 behavior.

IDs belong to a mounted declaration, not a layout ordinal or source expression.
The existing structural cursor gives keyed/positional semantics; source site,
role and repeated-site occurrence distinguish declarations within that scope.
Repaint, resize, reactive updates and cached subtrees retain identity. Absence
from an evaluated subtree, scene teardown and remount retire it. IDs are never
recycled in a host lifetime. No active pass means unavailable identity. No
recording means no identity allocation.

Clients join only exact host-connection/scene/preview/variant/generation/seq
evidence, reject duplicate/missing/stale metadata and retain separate source
revision proofs. An ID does not prove Rust ownership or editing capability.

Frame sequences increase across all publications within a host connection,
including scene removal/recreation. A consumer must compare ordering, not expect
contiguous numbers per preview. This prevents a new mount's metadata from joining
a queued frame of an older mount with the same scene name/build. Restarted hosts
still require a new client connection epoch.
