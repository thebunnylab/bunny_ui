# The updater

*Status: a desktop app checks a feed, downloads the new build, verifies
its Ed25519 signature and puts it in place on macOS, Linux and Windows.
The proofs:*

- *`cargo test -p bunny-ui-updater` covers the check, the download, the
  three install strategies and the service on a driven UI thread.*
- *On a Mac, a real disk image is made, mounted and installed over a
  bundle.*
- *A check and a 285 MB download against a release server in production
  verified that server's real signature with the std-only Ed25519.*

*The Linux and Windows file work runs in CI on their runners. A whole
app that updates itself under `bunny build` is not proven yet. The
ready-made update control is not here yet.*

An app on the desktop asks a feed whether a newer build is there. When
the person wants it, the updater downloads it, checks that its bytes
are the ones the publisher signed, and puts it where the running build
is. A restart runs it. The app writes none of this. It turns on a
feature, says where its releases are, and shows the state.

## Turn it on

1. Make a key pair. Commit the `.pub` file. Keep the `.key` file out of
   git and in your CI's secrets.

   ```sh
   bunny signer generate --out keys --name updater
   ```

2. Turn on the feature, and say where the releases are:

   ```toml
   [dependencies]
   bunny-ui = { version = "0.5", features = ["updater"] }

   [package.metadata.bunny.updater]
   feed = "https://dl.example.com/my-app/{channel}/latest.json"
   pubkeys = ["keys/updater.pub"]
   url = "https://dl.example.com/my-app/{version}/{file}"
   ```

3. Build with the key. `bunny build` signs the artifact and writes
   `latest.json` beside it:

   ```sh
   BUNNY_SIGNING_KEY="$(cat keys/updater.key)" bunny build macos
   ```

4. Upload the artifact, its `.sig` and `latest.json` where `url` and
   `feed` say. When you build on more than one machine,
   `bunny updater merge` joins the `latest.json` files.

The build fixes all of it: the feed, the keys, the product and the
channel. None of it comes from a setting at run time. An address that a
user can change is an address that an attacker can change.

## The feeds

| Feed | The app asks | The answer |
|---|---|---|
| `feed = "…"` (static) | `GET` the URL, with `{product}`, `{channel}`, `{target}`, `{os}`, `{arch}`, `{version}` filled in | a `ReleaseManifest`: one version, one artifact for each platform |
| `server = "…"` | `GET {server}/releases/{product}/{channel}/{os}/{arch}/{version}` | `200` with a `PlatformRelease`, or `204` when up to date |

A static feed is a file on any host: a bucket, the assets of a GitHub
release. A server can answer per platform and keep a channel pointer.
The types are in `bunny-ui-release`, the one contract that the
publisher, the server and the app share.

Each answer can carry `min_supported`. A build below it is **retired**:
the updater says so, and the app goes read-only until it updates.

## In the app

```rust
use bunny_ui::prelude::*;
use bunny_ui::updater::UpdateState;

/// Nothing when the build has no updater, or when there is nothing to say.
fn update_button() -> Option<impl View> {
    let label = match bunny_ui::updater::state()?.wrappedValue() {
        UpdateState::Available(offer) => format!("Update to {}", offer.version),
        UpdateState::ReadyToRestart { .. } => "Restart to update".to_owned(),
        _ => return None,
    };
    Some(button(text(label), bunny_ui::updater::click))
}
```

- `bunny_ui::updater::state()` is the `State<UpdateState>` a view reads.
  It is `None` when the build has no updater.
- `bunny_ui::updater::click()` does what the state asks:
  - **an offer:** it installs;
  - **an installed build:** it restarts;
  - **a failure:** it shows the log.
- `bunny_ui::updater::set_before_restart(|| …)` runs before a restart. A
  `false` keeps the app open, for "save your work?". The restart still
  waits.
- `AppConfig::check_for_updates(false)` stops the checks on a schedule.
  The app then calls `bunny_ui::updater::check_now()` itself.

| State | It means | A click |
|---|---|---|
| `Idle` | nothing to say, or up to date | nothing |
| `Available(offer)` | a newer build | installs it |
| `Installing(stage)` | downloading (with its percent), verifying, or applying | nothing |
| `ReadyToRestart { outcome, .. }` | installed; the old build still runs | restarts |
| `Failed` | the last attempt failed; the reason is in the log | shows the log |
| `Retired { minimum, upgrade }` | below the floor of the channel | installs the upgrade, when there is one |

The checks start 20 seconds after launch and come back every 6 hours.
A return to the app (a window brought forward, a machine that wakes)
asks again when the last answer is 30 minutes old.

## How each system installs

| System | Formats | How | The restart |
|---|---|---|---|
| macOS | `dmg` | mounts it read-only, then `rsync`s its `.app` over the running bundle | starts the same path again |
| Linux | `tar.gz` | unpacks inside the install folder and renames, the binary last. The `bin/` tarball of `bunny build linux` replaces the installed binary alone. | starts the new path |
| Windows | `zip`, NSIS, WiX | a copy of the app waits until the app and its sidecars are free, then unpacks or runs the installer | only quits; the helper starts the app |

On Linux, a copy that a package manager, a store or an AppImage owns is
refused before the download, with advice for the user.

## Security

- The publisher signs the **bytes of the artifact** with Ed25519. The app
  accepts an artifact only when one of its built-in keys verifies it.
  Give two keys, so you can change the signing key and keep the builds
  that you already shipped updating.
- A release for another product is refused **before** the download. A
  signature proves who published, not which app it is for.
- The SHA-256 of the feed is checked when it is there.
- The updater follows `https`, `http` and `file` URLs only. A URL that
  starts with `-` never reaches `curl`.

## Production gotchas

- **The feed JSON is not signed.** The `url`, the `format` and
  `min_supported` come through TLS and the server. A bad feed cannot make
  the app install bytes that you did not sign. But it can offer an old
  signed build, or retire a build that is good.
- **No rollback on macOS and Linux.** A failure before the Linux binary's
  rename leaves the old binary with new support files, which it can run.
- **The failure is in the log, not on screen.** Each failure adds one
  line to `update.log` in the app's cache folder. `click()` on `Failed`
  shows it. A Mac app started from the Finder has no stderr.
- **A hot run (`bunny run`) never updates itself.**
- **The network is `curl`**, which every Mac, every Linux desktop and
  Windows 10 and later has. It uses the system's certificates and
  `HTTPS_PROXY`. An app with its own HTTP client implements
  `bunny_ui::updater::Fetch` and calls `start_with`.
- **Ed25519 is the crate's own, on std alone.** The `updater-dalek`
  feature uses ed25519-dalek instead. The tests hold the two to the same
  answers.

## An app with its own `main`

`app!` runs the Windows update helper before any window. An app that
starts its own `platform::App` calls these first:

```rust
fn main() {
    if bunny_ui::updater::run_helper_if_asked() {
        return; // this launch was the update helper
    }
    // … then, with a configuration of its own:
    // bunny_ui::updater::start(config, bunny_ui::updater::Schedule::Automatic);
}
```
