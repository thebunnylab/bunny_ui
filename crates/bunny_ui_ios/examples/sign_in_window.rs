//! A sign-in form on the phone that asks the system for a saved password
//! as it appears — the twin of the Mac's `sign_in_window`. Each box says
//! what it holds, so the keyboard's QuickType bar offers Passwords over
//! the address and the password, and the code over the one-time code.
//!
//! ```sh
//! crates/bunny_ui_ios/simulator/run-sim.sh sign_in_window_ios
//! ```
//!
//! What to check by hand: tap the password box — the keyboard rises with
//! the Passwords key, and the typed characters stay bullets; the line
//! under the boxes says what the system answered (an unsigned example is
//! associated with no domain, so it is refused by name).
//!
//! To see a fill, the Simulator needs what a phone already has: a saved
//! password (the Passwords app, `+`) and an enrolled face (Features ▸
//! Face ID ▸ Enrolled). Without Simulator.app, run `xcrun simctl spawn
//! booted notifyutil -s com.apple.BiometricKit.enrollmentChanged 1`, then
//! the same command with `-p` in place of `-s 1`. Without both, the
//! Passwords key does nothing, in Safari as here. With them, the key asks
//! for the face (Features ▸ Face ID ▸ Matching Face, or `notifyutil -p
//! com.apple.BiometricKit_Sim.pearl.match`). The account you pick fills
//! the box that holds the keyboard: the address box takes the account's
//! name and the password box its password, one pick each.

#![cfg_attr(not(target_os = "ios"), allow(dead_code, unused_imports))]

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::text_input::KeyboardType;

#[derive(Clone, Copy)]
struct SignIn {
    email: State<String>,
    password: State<String>,
    code: State<String>,
    answer: State<String>,
}

impl Component for SignIn {
    fn body(self) -> impl View {
        let Self { email, password, code, answer } = self;
        vstack!(
            text_field("Email", email.binding())
                .keyboard_type(KeyboardType::Email)
                .autocomplete(Autocomplete::Username)
                .id("email"),
            text_field("Password", password.binding())
                .secret(true)
                .autocomplete(Autocomplete::CurrentPassword)
                .id("password"),
            text_field("Code", code.binding())
                .keyboard_type(KeyboardType::Digits)
                .autocomplete(Autocomplete::OneTimeCode)
                .id("code"),
            text!("{}", answer),
            spacer(),
        )
        .padding()
        .task(async move || {
            let said = match bunny_ui::autofill::saved_password().await {
                Ok(saved) => {
                    email.set(saved.user().to_owned());
                    password.set(saved.password().to_owned());
                    format!("filled from Passwords: {}", saved.user())
                }
                Err(error) => format!("{error:?}"),
            };
            answer.set(said);
        })
    }
}

#[cfg(target_os = "ios")]
fn main() {
    let form = SignIn {
        email: State::new(String::new()),
        password: State::new(String::new()),
        code: State::new(String::new()),
        answer: State::new(String::from("asking the system…")),
    };
    bunny_ui_ios::run_window("Sign in", Size { width: 320.0, height: 480.0 }, form);
}

#[cfg(not(target_os = "ios"))]
fn main() {} // this example is iOS-only
