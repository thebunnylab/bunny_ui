# Foreground blur — 2026-10-19

At `b8c0e6ced1b564127722fc43f541d607263a3f1b`, the supported blur modifier changes real painting. The failing-before
public Runtime regression shows identical pixels for blur0 and blur3 at `ab60478d`.
The correction composites the subtree, filters premultiplied color and alpha,
preserves layout/hit/source records and prevents independent custom/scrollbar
layers from bypassing the effect. DOM/SSR keep native elements using CSS filter;
ABI22 advances all decoders together. [Behavior](../../blur.md) describes bounds
and platform-host limitations.

All 1441 core/web production tests pass (59 ignored), including eight focused
public Runtime scenarios and a byte-level DOM payload test. Production native
strict lint and pinned-nightly core wasm strict lint pass. The two additional
lint lanes that exposed inherited diagnostics remain explicitly failed in
[verification](verification.json); they are not represented as green.

The source commit precedes this evidence-only commit. Corrected native Trinity
and standalone verification are pending the combined T182/T183 integration.
Technical proof does not approve its own Quipu criterion.
