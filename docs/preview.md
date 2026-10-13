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
