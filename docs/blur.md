# Foreground blur

`.blur(radius)` composites the view's framework paint as one layer and blurs it.
It does not change measurement, hit testing, accessibility bounds or source-site
identity. A clip before blur cuts the input; a clip after blur cuts the result.
Styles do not fold across this boundary, and consecutive blurs compose.

Radius is Gaussian standard deviation in logical pixels. Nonpositive and
nonfinite values are inert; positive values are capped at 128. Pixel targets
approximate the Gaussian using three separable box filters. Filtering includes
premultiplied RGB and alpha, followed by one conversion to straight alpha for
the existing immutable image transport. This avoids dark fringes around
translucent content. The layer retains the kernel support outside the viewport;
its allocation follows the visible surface and device scale, not an unbounded
scroll document. Backdrop materials can still sample paint beneath the layer.

Custom paint and scrollbar thumbs inside a blur remain in the scene. A looping
custom view requests a scene repaint so its independent surface cannot bypass
the effect. Native platform hosts such as a webview or video surface remain
platform-owned content outside framework paint; this modifier does not capture
or filter those native surfaces. They remain paint barriers: framework paint
on each side is composited separately, preserving the host's stacking order.
Composite replacement rebinds custom paint ranges and native-host marks;
invisible placeholder commands are not retained.

The element renderer keeps native text and controls and emits CSS `filter`.
DOM ABI 22 adds style bit 25 and a trailing f32 radius. The encoder, SSR rules,
canonical browser decoder and shipped copies advance together; stale browser
code is rejected by the existing ABI handshake. The Studio preview protocol
and project format are unchanged.

`crates/bunny_ui/tests/blur_paint.rs` checks actual painting, transparency, clip
order, overlapping children, nested effects, source recording, click geometry,
radius normalization, device scale, animation, scrollbar layers and SSR.
