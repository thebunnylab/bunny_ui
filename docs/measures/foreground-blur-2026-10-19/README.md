# Foreground blur — 2026-10-19

At `9ee003d3083613c28812b69a4668c7cf7e2b470c`, the supported blur modifier changes real painting. The failing-before
public Runtime regression shows identical pixels for blur0 and blur3 at `ab60478d`.
The correction composites the subtree, filters premultiplied color and alpha,
preserves layout/hit/source records and prevents independent custom/scrollbar
layers from bypassing the effect. DOM/SSR keep native elements using CSS filter;
ABI22 advances all decoders together. [Behavior](../../blur.md) describes bounds
and platform-host limitations.

All 1443 core/web production tests pass (59 ignored), including ten focused
public Runtime scenarios and a byte-level DOM payload test. The ten focused scenarios also pass with
`BUNNY_PARANOID=all`, including clipped multi-child and native-host paint order.
The intermediate placeholder defect and its failing-before log are retained;
final ranges are rebound after direct composite replacement. Production native
strict lint and pinned-nightly core wasm strict lint pass. The two additional
lint lanes that exposed inherited diagnostics remain explicitly failed in
[verification](verification.json); they are not represented as green.

The source commit precedes this evidence-only commit. Corrected native Trinity
and standalone verification are pending the combined T182/T183 integration.
Technical proof does not approve its own Quipu criterion.
