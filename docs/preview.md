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

Each instance variant owns an immutable `provenance::OverrideSet`. Rendering,
input and dirty checks enter its context and restore the outer context, even on
panic. A set retains its own version across passes, so another artboard does not
invalidate its layout cache. The legacy thread-wide override table remains the
legacy lane. A delayed snapshot holds the runtime identity, preventing a removed
artboard's request from attaching to a recreated instance. A generation change
invalidates the last-sent frame even when its pixels are identical, allowing tools
to associate the new source receipt with every retained instance.
