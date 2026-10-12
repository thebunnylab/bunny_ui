//! The preview wire and the preview scan — the two things a preview host
//! and the tool that shows its frames agree on.
//!
//! A preview host is the app's own binary, started by `bunny preview` or
//! by an IDE in preview mode: it renders the views its `#[preview]`
//! functions build and sends each frame — a display list, not pixels —
//! down a loopback socket, with the source site of every node, so the
//! tool paints the picture with its own renderer and maps a click back to
//! a line. The tool answers with what to show, the input it forwards and
//! the style overrides it wants tried before the code is saved.
//!
//! ## What this ships
//!
//! - [`wire`]: the framing, the messages in both directions
//!   ([`wire::ToIde`], [`wire::ToHost`]), the [`wire::Draw`] that mirrors
//!   the framework's draw commands one to one, the sites, the resources,
//!   the traits and the patches — std only, so a tool that reads the wire
//!   adds nothing else. `WIRE_VERSION` is 1; a message of a kind a reader
//!   does not know is skipped by its length.
//! - [`scan`]: the reference scanner — which functions of a crate are
//!   previews, by reading the source for the attribute, with the fixtures
//!   under `tests/scan/` as the contract every other scanner meets.
//! - [`snapshot`]: a picture against its golden, pixel by pixel, the
//!   difference as a count and a box — what `bunny preview --snapshot`
//!   prints and a test asserts on; std only, like the wire.
//! - With the `core` feature, [`convert`]: the framework's `DisplayList`
//!   to draws and back (resources collected by key), its `Traits` to the
//!   wire's and back, a `Patch` to a `Modifier`.
//! - With the `host` feature, [`host`]: the runner an app becomes under
//!   `bunny preview` — one scene per selected variant, the panic boundary,
//!   the serving loop, the headless check, and a snapshot on request: the
//!   frame's pixels through the scene's own engines, what `bunny preview
//!   --png` writes.
//!
//! ## Production gotchas
//!
//! - A resource (an image's pixels, a vector icon) travels once per key;
//!   the tool caches it until the host says `Forget`. A `Native` image —
//!   a platform surface — has no bytes to send: the tool draws a
//!   placeholder in its frame until the host ships a snapshot of it.
//! - Text arrives wrapped: a `TextLine` is one already-broken line in the
//!   host's font metrics. The tool paints it; it does not shape it again.
//! - Fonts are named on the wire (the family's name, empty for the system
//!   face) and interned again on the tool's side.

#![forbid(unsafe_code)]

pub mod scan;
pub mod snapshot;
pub mod wire;

#[cfg(feature = "core")]
pub mod convert;

#[cfg(feature = "host")]
pub mod host;
