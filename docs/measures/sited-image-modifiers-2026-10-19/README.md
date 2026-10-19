# Pixel modifiers through recorded source sites

Studio rendered a 3×1 image at intrinsic size despite `.resizable().frame(180.0, 120.0)`. Fit/Fill edited valid Rust but had no effect on preview painting. The retained failing-before test reproduces the exact geometry discrepancy with source recording enabled.

`rewrite_pixel_node` now traverses `Sited`, preserving every provenance field. Three public runtime regressions check image Fit/Fill geometry (180×60 contain, 360×120 cover inside the 180×120 clip), icon resize, retained mounted sites, existing action/element-hint behavior and isolation of the overlay layer.

[Verification](verification.json) records test totals and source hashes. Default and production-feature core tests pass. CI-exact production strict native and pinned-nightly wasm clippy pass. The initial default-feature all-targets lint failure is retained: an unchanged `glass.rs` test import is unused without GPU features. The fix neither suppresses that diagnostic nor changes unrelated code.

Native corrected Trinity pin/helper validation is still required by T182; this package supplies the framework portion only. The complete source change is the commit containing this package. Authentic acceptance remains separate.
