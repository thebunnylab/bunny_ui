# bunny

The command-line tool of [bunny-ui](https://github.com/thebunnylab/bunny_ui):
create an app, and run it on macOS, iOS, Windows, Linux, Android and the web.

```bash
cargo install bunny-cli
bunny new my_app
cd my_app
bunny run            # on this computer
bunny run -d ios     # in the iOS Simulator
bunny run -d android # on an Android emulator or phone
bunny run -d web     # in the browser
bunny build macos    # a signed app and its disk image, to hand out
```

| Command | |
| --- | --- |
| `bunny new` | Create an app with every platform's files in place |
| `bunny run` | Build the app for a device and run it, its output here; a save reloads it hot, `R` restarts, `q` stops |
| `bunny build` | Build the app to ship: a site for the web, a signed and notarized app for macOS |
| `bunny doctor` | Check what this machine needs for each platform — and offer to install what it can |
| `bunny setup android` | Install the Android toolchain — SDK, NDK, emulator, a JDK — without Android Studio |
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
target. While the app runs, `R` rebuilds and restarts it, `q` stops it; when
the app exits on its own, `bunny run` exits with its code.

### Hot reload

A debug run reloads hot — on this computer (macOS, Linux, Windows), in the iOS
Simulator and on Android: save a file and the running app takes the new code
and keeps its state — the counter keeps its count, the text field its text, a
`view_model` its model, a `.task` keeps running. `r` reloads at once, without a
save. In the browser, a save rebuilds the page and reloads it, its state
starting over.

`bunny` builds the framework once per session as one shared library
(`bunny-ui-dylib`, the `hot` feature of `bunny-ui`), and after each save it
builds only the app's library, which the app loads next to the code it has.
The app calls `bunny run` back on a socket — on the loopback, or on Android
through `adb reverse` — and takes each new build there: by its path, or, on
Android, as bytes it writes to its own folder first.

- An edit inside function bodies keeps all the state. An edit that reaches a
  type — a field, a signature, a new item — gives the new build new types: the
  state that holds the app's own types (`State<Vec<Todo>>`) starts over, the
  state of other types (`State<i32>`, `State<String>`) stays.
- A change to `Cargo.toml`, `build.rs` or `src/main.rs` restarts the app; so
  does a reload that finds another crate of the workspace changed.
- What the old code started keeps the old code: a running `.task`, a closure a
  state holds. `R` starts them over with the new code.
- A build error leaves the app on the code it had. A crash waits for the fix:
  save it, and the app starts again.

`--no-hot` turns it off. On macOS, each new build is checked by the system the
first time it loads (about 0.2 s); adding the terminal under System Settings ›
Privacy & Security › Developer Tools skips the check.

On macOS, an app with a `macos/` folder runs inside a bundle (`<Name>.app`
under `target/bunny/`), so notifications and its name in the menu bar work. On
the iOS Simulator, `bunny` assembles the `.app` from `ios/Info.plist`, installs
it and streams its console; `BUNNY_*` and `RUST_BACKTRACE` reach the app.

In the browser (`-d web`), `bunny` builds the app's library for wasm, puts the
page together — the project's `web/` with `index.html` filled in, and the glue
of the very `bunny-ui-web` cargo resolved, its ABI checked against the
framework's — and serves it at `http://localhost:8080/` (`--web-port`,
`--web-hostname`, `--no-open`). A new build reloads the page by itself, and the
page's console errors, a wasm panic among them, show in the terminal.

On Android (`-d android`, or an emulator's name), `bunny` links the library with
the NDK for the device's CPU, writes `android/local.properties` with the app's
names and paths, lets the project's Gradle pack the APK, installs it with adb
and starts it, following the log of the app's process and the system's crash
reports.

## `bunny build`

`bunny build <platform>` builds the app for release and packs it the way the
platform hands it out, under `build/<platform>/`. Next to the package,
`build-info.json` says what each file is and how it was made, and `build.log`
holds every command that made them, with their answers. The version and the
build number come from `Cargo.toml`; `--build-name` and `--build-number`
override them for one build.

- **`web`** — the page as a folder any static host serves. The wasm comes from
  the `web` profile when the workspace has one, and goes through `wasm-opt -Oz`
  when binaryen is installed. The files `bunny` adds to the page (the wasm and
  the glue) are named after their content, and a `_headers` file (Netlify,
  Cloudflare Pages) keeps them cached for a year while `index.html` is asked
  for every time.
- **`macos`** — `<Name>.app` and `<Name>-<version>.dmg`. The bundle carries
  `macos/Info.plist` filled in and the icon from `macos/AppIcon.icns`, or from
  `macos/AppIcon.png` at 1024 × 1024. A Developer ID Application identity from
  the keychain signs it with the hardened runtime (`--sign` names one, and
  `macos/entitlements.plist` is used when it exists); without one the app is
  signed ad hoc and runs only on this Mac. With a notarytool keychain profile
  (`xcrun notarytool store-credentials`, then `--notary-profile` or
  `BUNNY_MACOS_NOTARY_PROFILE`) or an App Store Connect API key
  (`BUNNY_MACOS_NOTARY_KEY`, `_KEY_ID`, `_ISSUER`), the disk image is notarized
  and the ticket stapled to it and to the app. `--universal` builds for Apple
  silicon and Intel, `--no-dmg` stops at the app, `--no-codesign` leaves it
  unsigned.

## `bunny setup android`

Android without Android Studio: `bunny setup android` fetches Google's Android
CLI, which installs the platform tools, the emulator, the platform and the NDK
and creates an emulator, and a JDK for Gradle (Temurin, checked against its
SHA-256). The Android SDK's terms are shown for you to accept before anything is
installed (`--accept-android-terms` for CI); Google's usage metrics stay off.

Coming next: iPhones, and `bunny build` for iOS, Android, Windows and Linux.
