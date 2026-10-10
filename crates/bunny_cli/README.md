# bunny

The command-line tool of [bunny-ui](https://github.com/thebunnylab/bunny_ui):
create an app, and run it on macOS, iOS, Windows, Linux, Android and the web.

```bash
cargo install bunny-cli
bunny new my_app
cd my_app
cargo run
```

## `bunny new`

`bunny new my_app` creates a cargo package whose `src/lib.rs` holds the app and
ends in `bunny_ui::app!` — the one line that starts it on every platform — and
one folder per platform (`android/`, `ios/`, `macos/`, `web/`) with that
platform's own files. Those folders are yours to edit: `bunny` fills their
`@BUNNY_…@` markers from `Cargo.toml` on every build and never writes over
them.

| Option | |
| --- | --- |
| `--name "My App"` | The name people read (default: from the folder's name) |
| `--org com.yourcompany` | The prefix of the app's id (default: `com.example`) |
| `--id com.yourcompany.app` | The whole id, instead of `ORG.CRATE` |
| `--platforms android,web` | Only these platform folders |

Run inside an app that already exists (`bunny new .`), it adds the platform
folders that are missing and touches nothing else.

The project `bunny new` writes is entirely yours: the templates are licensed
under the Zero-Clause BSD License (`templates/LICENSE`).
