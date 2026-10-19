# Focus across native popover surfaces

T2-ATR-143 / FR-ATR-43. Tested implementation:
`6a8c3b1797e7fb75b3cf64d1676916eaf1d61bc1`, based on
`6a6b1d209c143b3c5a5be68882279b3eb264b6c1`.

The AppKit adapter correctly exports popover buttons through their child
surface. A client that inspects only the parent's children omits those buttons.
There was also a distinct product defect: after focusing a native popover choice,
the parent's `accessibilityFocusedUIElement` returned its scene container.
The actual child surface already reported the labeled, actionable `AXButton`
and its focus. The new native regression, retained at `32562aa`, failed the
parent-focus assertion before the adapter correction, with terminal101.

Focus lookup now searches the owning scene's surfaces. An active overlay proxy
takes precedence over a native editor behind it; otherwise the native editor
keeps its existing focus handling. The scene boundary prevents another window's
focused proxy from becoming this window's answer. Main-window children remain
unchanged, so clients still need to enumerate native overlay surfaces.

The final native protocol queries real AppKit selectors. Three repeated
open/select cycles check labels, button roles, native-window ownership,
screen-space bounds/hit testing, the child and parent's focused-element answers,
selection callbacks and focus restoration. Retained choices reject actions after
dismissal/reuse and after closing their owner. An independent live window retains
its focus and receives no selection callback. Existing secure fields, native
editor/shared-field-editor focus, disabled controls, modal first-query and
main-thread teardown checks also pass.

On the immutable implementation, the native protocol,35 macOS library tests,
8 trace tests, full rustfmt for both changed Rust files and whitespace pass.
Strict macOS all-target clippy exits101 on both base and candidate. Normalized
comparison finds18 distinct diagnostics on each, zero new and zero suppressed.
The comparison does not turn either strict invocation into a clean lint pass.

[Native logs, commands, times, exit codes, lint comparison and binary identity](native-popover-focus-2026-10-17/)
bind the final native executable SHA-256
`e50ba4406068ac1d0889ce3180b2747c8305b6687fb4285eb5d6b92131d28b8d`.
This in-process AppKit protocol uses synthetic controls and no real accounts,
devices or colleague traffic. It does not replace an external AX client or a
human VoiceOver workflow.

Trinity integration and an actual reaction-picker input/focus review on the
integrated revision remain necessary. The originating observation included
successful native open/Escape/outside dismissal but missing main-window choice
representation. This framework proof separates that inspection limit from the
reproduced focus defect; it does not claim complete product, physical/touch,
server or daily-use phase acceptance.
