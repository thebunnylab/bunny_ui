# bunny

The command-line tool of [bunny-ui](https://github.com/thebunnylab/bunny_ui):
create an app, and run it on macOS, iOS, Windows, Linux, Android and the web.

```bash
cargo install bunny-cli
bunny new my_app
cd my_app
bunny run            # on this computer
bunny run -d ios     # in the iOS Simulator
```

| Command | |
| --- | --- |
| `bunny new` | Create an app with every platform's files in place |
| `bunny run` | Build the app for a device and run it, its output here; `r` restarts, `q` stops |
| `bunny doctor` | Check what this machine needs for each platform — and offer to install what it can |
| `bunny devices` | List where the app can run: this computer, the browser, simulators, emulators, phones |
| `bunny emulators` | List the simulators and emulators, and start one (`--launch`) |

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

## `bunny run`

`bunny run` builds the app and starts it on this computer; `-d` picks another
device — an id or a name from `bunny devices`, or a platform (`-d ios` boots
the iPhone simulator on the newest iOS if none is running). Before building it
checks the machine as `bunny doctor` does, and offers to install a missing Rust
target. While the app runs, `r` or `R` rebuilds and restarts it, `q` stops it;
when the app exits on its own, `bunny run` exits with its code.

On macOS, an app with a `macos/` folder runs inside a bundle (`<Name>.app`
under `target/bunny/`), so notifications and its name in the menu bar work. On
the iOS Simulator, `bunny` assembles the `.app` from `ios/Info.plist`, installs
it and streams its console; `BUNNY_*` and `RUST_BACKTRACE` reach the app.

Coming next: the web and Android in `bunny run`, hot reload on `r`, iPhones,
and `bunny build` for release packages.
