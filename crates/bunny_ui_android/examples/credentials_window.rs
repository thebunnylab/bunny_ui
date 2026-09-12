//! The secret store on the phone — and the one thing a store must
//! prove, which is that it survives the app.
//!
//! Run it TWICE. The first run finds nothing, writes a secret and says
//! so. Every run after it finds that secret and shows it back, because
//! the key that opens it belongs to the keystore and the ciphertext to
//! the app's own preferences.
//!
//! ```sh
//! crates/bunny_ui_android/android/run-emu.sh credentials_window_android
//! crates/bunny_ui_android/android/run-emu.sh credentials_window_android
//! ```
//!
//! To see the other half — the store erased, the app asking again —
//! clear the app's data between runs:
//!
//! ```sh
//! adb shell pm clear com.bunnylab.example
//! ```

#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_imports, unused_variables))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;

/// The pair this example files its secret under.
const SERVICE: &str = "example.bunnylab.com";
const ACCOUNT: &str = "session";

#[derive(Clone)]
struct Store {
    /// What the store answered on this run, and what this run did about
    /// it.
    found: State<String>,
    told: State<String>,
}

impl Component for Store {
    fn body(self, _ctx: &Context) -> impl View {
        let clear = self.clone();
        let again = self.clone();
        vstack!(
            text("The secret store").font(Font::Title),
            text(self.found.get()).font(Font::Body),
            text(self.told.get()).font(Font::Caption),
            spacer(),
            button(text("Write a new secret"), move || again.put()),
            button(text("Erase it"), move || clear.erase()),
        )
        .alignment(HorizontalAlignment::Leading)
        .spacing(12.0)
        .padding()
        // The store blocks and the read decides what the screen says, so
        // it runs once when the view appears and not inside the body.
        .on_appear(move || self.look())
    }
}

impl Store {
    /// What the store holds for this pair, said plainly either way.
    #[cfg(target_os = "android")]
    fn look(&self) {
        match bunny_ui_android::credentials::read(SERVICE, ACCOUNT) {
            Some(secret) => {
                self.found.set(format!("found: {secret}"));
                self.told.set("This run opened what an earlier run wrote.".to_owned());
            }
            None => {
                self.found.set("found: nothing".to_owned());
                self.put();
            }
        }
    }

    /// Writes a secret that names the moment, so a later run can show
    /// which run wrote it.
    #[cfg(target_os = "android")]
    fn put(&self) {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let secret = format!("written at {stamp}");
        if bunny_ui_android::credentials::write(SERVICE, ACCOUNT, &secret) {
            self.found.set(format!("found: {secret}"));
            self.told.set("Stored. Run the app again and it opens this.".to_owned());
        } else {
            // The refusal is the message: nothing was stored, and no
            // plaintext was written anywhere instead.
            self.told.set("The platform refused to store it.".to_owned());
        }
    }

    #[cfg(target_os = "android")]
    fn erase(&self) {
        let gone = bunny_ui_android::credentials::delete(SERVICE, ACCOUNT);
        self.found.set("found: nothing".to_owned());
        self.told.set(if gone {
            "Erased. The next run finds nothing.".to_owned()
        } else {
            "The platform refused to erase it.".to_owned()
        });
    }

    #[cfg(not(target_os = "android"))]
    fn look(&self) {}
    #[cfg(not(target_os = "android"))]
    fn put(&self) {}
    #[cfg(not(target_os = "android"))]
    fn erase(&self) {}
}

fn main() {
    let store = Store { found: State::new(String::new()), told: State::new(String::new()) };
    #[cfg(target_os = "android")]
    bunny_ui_android::run_window("bunny_ui", Size { width: 320.0, height: 240.0 }, store);
}

// the activity's entry point, in the app's own shared object
#[cfg(target_os = "android")]
bunny_ui_android::activity!(main);
