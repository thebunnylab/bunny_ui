//! A sign-in form in the element mode, mounted by someone else's loader.
//!
//! Every other page in this repository instantiates its own wasm
//! (`glue.js`, `glue_dom.js`): no bindgen, the border written by hand.
//! This one is built by trunk and loaded by wasm-bindgen — the toolchain
//! an app picks when it also wants `web-sys`, `fetch` and the crates
//! written against them — so the loader is not ours, and the border is
//! the module glue (`glue/esm/bunny.js`). `attachDom` takes the instance
//! wasm-bindgen made and borrows the element shell whole: the same patch
//! applier, element registry and delegated events `glue_dom.js` runs for
//! a page it loads itself.
//!
//! ## What this ships
//!
//! - `boot`, the page's entry: `attachDom` calls it with the mount's box
//!   and the device ratio, and it reaches `bunny_ui_web::start_dom`.
//! - The form. Each box says what it holds ([`Autocomplete`]) and what a
//!   reader calls it (`.accessibility_label`), and the password box is a
//!   secret — so the browser sees `autocomplete="username"` on a text
//!   input and `autocomplete="current-password"` on a password input,
//!   which is the shape a password manager fills.
//!
//! ## Wiring
//!
//! `web/index.html` copies the three glue files beside the page
//! (`bunny.js`, `bunny_gpu.js`, `glue_dom.js`) and boots on trunk's
//! `TrunkApplicationStarted`. `README.md` has the build.
//!
//! [`Autocomplete`]: bunny_ui::text_input::Autocomplete

/// The browser's `main` is empty on purpose: wasm-bindgen's loader runs
/// it while instantiating the module, before the glue holds the instance.
#[cfg(target_family = "wasm")]
fn main() {}

#[cfg(not(target_family = "wasm"))]
fn main() {
    eprintln!("signin-web runs in a browser: `trunk serve` in apps/signin_web/web");
}

#[cfg(target_family = "wasm")]
mod page {
    use std::sync::Arc;

    use bunny_ui::prelude::*;
    use wasm_bindgen::prelude::wasm_bindgen;

    const FIELD_WIDTH: f64 = 296.0;

    /// The form's whole state: what the two boxes hold, whether the
    /// password shows, and the line the page answers a submit with.
    #[derive(Clone, Copy)]
    struct SignIn {
        email: State<String>,
        password: State<String>,
        revealed: State<bool>,
        answer: State<Arc<str>>,
    }

    impl SignIn {
        fn new() -> Self {
            Self {
                email: State::new(String::new()),
                password: State::new(String::new()),
                revealed: State::new(false),
                answer: State::new(Arc::from("Not signed in.")),
            }
        }

        /// What a submit says back — the proof that what was typed into
        /// the browser's inputs reached the app's bindings. The password
        /// is counted, never shown.
        fn submit(self) {
            let email = self.email.get();
            let answer = if email.is_empty() {
                "Type an email to sign in.".to_owned()
            } else {
                let length = self.password.get().chars().count();
                format!("Signed in as {email} with a password of {length} characters.")
            };
            self.answer.set(Arc::from(answer));
        }
    }

    impl Component for SignIn {
        fn body(self) -> impl View {
            vstack!(
                text("Sign in").font_size(22.0).bold(),
                vstack!(
                    text("Email").foreground_color(theme::fg_secondary()),
                    text_field("", self.email.binding())
                        .keyboard_type(KeyboardType::Email)
                        .autocomplete(Autocomplete::Username)
                        // Enter in either box signs in, and the page
                        // opens with the keyboard in its first one
                        .on_submit(move || self.submit())
                        .auto_focus()
                        .accessibility_label("Email")
                        .frame_width(FIELD_WIDTH),
                )
                .alignment(HorizontalAlignment::Leading)
                .spacing(6.0),
                vstack!(
                    text("Password").foreground_color(theme::fg_secondary()),
                    text_field("", self.password.binding())
                        // the reveal a password box offers is the flag, off
                        .secret(!self.revealed.get())
                        .autocomplete(Autocomplete::CurrentPassword)
                        .on_submit(move || self.submit())
                        .accessibility_label("Password")
                        .frame_width(FIELD_WIDTH),
                )
                .alignment(HorizontalAlignment::Leading)
                .spacing(6.0),
                checkbox(text("Show password"), self.revealed.binding()),
                button(text("Sign in"), move || self.submit()),
                text(self.answer.get()).foreground_color(theme::fg_secondary()),
            )
            .alignment(HorizontalAlignment::Leading)
            .spacing(16.0)
            // the mount is the card: the page rounds it, the theme grounds it
            .padding_length(24.0)
        }
    }

    /// The page's entry: `attachDom` calls it with the mount's box in CSS
    /// px and the device ratio, once the element shell holds the instance.
    #[wasm_bindgen]
    pub fn boot(width: f64, height: f64, scale: f64) {
        bunny_ui_web::start_dom(width, height, scale, SignIn::new());
    }
}
