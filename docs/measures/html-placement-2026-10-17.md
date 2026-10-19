# Native placement before initial HTML submission

T2-MAIL-23 / FR-MAIL-10. Tested implementation:
`a47c97181b6afdfb11c8fe36d3893117592446dd`, based on
`46080c1145390e227eb4d6601c9f311ebfec24ff`.

The native host previously called its webview factory before attachment and
initial sizing. The factory submitted HTML immediately. On diagnostic baseline
`b7ca9592d384754526013c2ce45d67b716f8c60b`, the existing two-window reuse
protocol completed, but all four document submissions lacked a native window;
three also had zero width/height. The new placement checker refused with exit1.
The baseline's source and trace are retained; its executable hash was not recorded.

Shared Apple creation now accepts a synchronous placement continuation between
instrumented mount registration and initial document/URL submission. macOS and
iOS attach and size the view inside that continuation, using the existing
clipping and coordinate conversions. Their host-factory functions are private;
the exported immediate `webview::create` behavior is preserved. No preload
document, extra engine reserve, task, readiness shortcut or content-policy change
was introduced. Retained stamp updates, blank scrubbing and generation routing
remain unchanged.

On implementation a47c971, the same native reuse protocol passed and its four
submissions were attached at330×190 or320×180 before loading. All four navigations
committed and finished; both expected application posts arrived. The existing
protocol verifies private-to-public engine reuse without old DOM/context and
independence of another live pane and second window. This is native structural
evidence, not presented-body timing. The Mac was reported locked; no on-screen
visibility, physical input or speed improvement is claimed.

Verification on that immutable implementation:

- Apple library:124 tests passed; macOS library:35; trace example:8.
- Placement and lifecycle checker integrity:3 tests each passed.
- Native reuse, vertical/horizontal parent-scroll, link/text-selection and
  multiple-host protocol, and window lifecycle subprocesses passed.
- iOS simulator-target compilation passed; native iOS behavior remains untested.
- Changed-code rustfmt comparison and whitespace passed, preserving unrelated
  existing formatting.

Strict all-target clippy was run independently for Apple/macOS and for iOS on
the simulator target, on both revisions with separate checkout-local artifacts.
Each invocation exited101 on existing diagnostics. Normalized comparison found
zero new diagnostics: Apple39→39, macOS19→18, iOS3→3 distinct diagnostics.
Grouping macOS host geometry removes its existing excessive-argument finding.
No suppression was added, and a clean strict-lint pass is not claimed.

[Commands, times, exit codes, traces, binary identity and raw lint logs](html-placement-2026-10-17/)
bind the final native executable SHA-256
`fa75f3f8c546faaf0a33a1e0a744083b6f874239dbb748b97ffdf140b369ae71`.
All native data is synthetic; no accounts, providers or capture devices are used.

Consumer integration and identical-content, unlocked own-window measurements
remain necessary before attributing a latency gain. The existing failed Mail
100ms p95 /200ms p99 evidence, first-session requirement and full
HTML/text/search/long-thread/rapid/offline/restart/input/provider matrix remain
open. This ordering correction alone does not complete T2-MAIL-23.
