# Preview reload diagnostics — T2-STUDIO-161

A failed hot reload now carries the existing structured `Error` to the preview lane. Compiler diagnostics retain their message, code, original file, line, column and rendered text. Failures without compiler diagnostics emit an actionable note (including a hint when present) and `BuildFailed`; the running host keeps its last good generation. The terminal lane prints the error summary without replaying compiler diagnostics already rendered by Cargo. No wire or project format changed.

## Reproduction and checks

- `trinity-before-fix.log`: the real Trinity host test failed against Bunny `8412616d51c8deaf1dcb441b20b08390c24b7d93`. An invalid body reached failed status with no compiler diagnostic. Trinity base: `cb75901af0235ca2702bba1260094cd543f508a3`, strengthened `source_history` assertion.
- `cargo test --locked -p bunny-cli`: 131 passed, five ignored. Includes complete diagnostic preservation and diagnostic-free message/hint/failure signaling.
- `cargo test --locked -p bunny-cli --test preview_host -- --ignored --nocapture`: passed. An actual compiled body fails with E0425 and its exact source span; both live instances retain the old generation and independent counts 1/0. A corrected save updates both without restart and retains the counts and sizes. Existing source-provenance, sibling reload and snapshot checks also pass.
- `cargo clippy --locked -p bunny-cli --all-targets -- -D warnings`: passed.
- Scoped rustfmt on the new test module, `git diff --check` and packaged-template checks: passed. Existing compact runtime/test formatting is preserved.

Framework base: `6d964ed3dda1031376583de32e9b99ad6d469aa8`; the tested implementation and assertions are committed with this report. The two earlier failed harness runs are retained: the first targeted a label already replaced by the sibling-crate scenario; the second still expected the previous snapshot height after the new responsive-host probe resized it. Neither is a product failure.

These tests exercise the compiled host and event transport. They are not physical-window interaction, editor Undo/Redo, final Studio performance acceptance or human approval. The CLI is native tooling; no web code changed. Trinity integration and the T159 native recovery scenario remain separate.
