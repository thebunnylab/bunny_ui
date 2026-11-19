//! A sign-in window that asks the system for a saved password as it
//! appears, and shows what the system answered — the AutoFill door
//! (`bunny_ui::autofill`) beside boxes that say what they hold.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example sign_in_window
//! ```
//!
//! A bare binary is associated with no domain, so the system refuses
//! the request by name; a bundle whose entitlements carry
//! `webcredentials:` for a domain is offered the accounts saved for it.
//! With `BUNNY_EXIT_AFTER_ANSWER` set the window quits once the answer
//! is printed — the shape a scripted check runs it in.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

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
                .autocomplete(Autocomplete::Username),
            text_field("Password", password.binding())
                .secret(true)
                .autocomplete(Autocomplete::CurrentPassword),
            text_field("Code", code.binding())
                .keyboard_type(KeyboardType::Digits)
                .autocomplete(Autocomplete::OneTimeCode),
            text!("{}", answer),
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
            println!("autofill: {said}");
            answer.set(said);
            if std::env::var_os("BUNNY_EXIT_AFTER_ANSWER").is_some() {
                std::process::exit(0);
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn main() {
    let form = SignIn {
        email: State::new(String::new()),
        password: State::new(String::new()),
        code: State::new(String::new()),
        answer: State::new(String::from("asking the system…")),
    };
    bunny_ui_macos::run_window("Sign in", Size { width: 320.0, height: 220.0 }, form);
}

#[cfg(not(target_os = "macos"))]
fn main() {} // this example is macOS-only
