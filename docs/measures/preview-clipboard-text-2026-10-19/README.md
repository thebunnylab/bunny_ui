# Preview clipboard text

T176 corrects the process-boundary paste gap reproduced in Trinity's actual Live preview. A literal new-kind payload test fails against the old decoder/host; the same test passes after the negotiated input adapter. The original native captures and exact app/source binding are retained in Trinity PR1796.

Capability256 enables legacy Paste kind86 and additive instance command tag12, retaining the original key/modifiers and a whole UTF-8 commit. App handlers get the shortcut first; Runtime paste history separates the commit from typing and other pastes. The host reads no OS clipboard. Existing Key/Text and version1 goldens stay unchanged.

Verification on production754f1e360:1232 core tests(1 existing ignored),93 preview tests,131 CLI tests(5 existing environment ignores); strict native all-feature/all-target and CI-exact nightly wasm lint. Tests cover replacement, Unicode/multiline commits, Undo/Redo boundaries, custom handler priority and per-instance/variant isolation. Changed Rust hunks match rustfmt without reformatting inherited unrelated code. No framework just/check recipe exists.

Trinity pin/session/native canvas integration and final physical proof are separate remaining work. These technical results do not supply authentic approval or complete T73.

Packaged text logs omit trailing empty lines; command output and results are otherwise retained.
