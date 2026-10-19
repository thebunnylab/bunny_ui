# Native blur pixels — 2026-10-19

Blur previously sent its computed pixels as a headless RawImages envelope in
ImageSource::Bytes. Apple ImageIO cannot decode that envelope, so the entire
subtree disappeared in the signed native preview. Both the decoder-independent
Runtime regression and the real Apple decoder test fail on d633ebe7.

Source `c8dc378e5d37d82b6b7b3d1e8f428390a7d2b0e4` retains the content-and-size identity and passes already
decoded pixels through the existing ImageSource::Rgba variant. Both regressions
and the core, Apple and preview suites pass. The existing preview resource/wire
roundtrip carries RGBA unchanged; there is no protocol or project-format change.

[Verification](verification.json) preserves all command results. Core/preview
strict native and pinned-nightly wasm gates pass. The additional Apple strict
lint fails on 34 diagnostics in unchanged files; it is not reported as passing.
Corrected native-window and same-source standalone observations remain pending
in T183 until the new framework is integrated into the signed Trinity host.
