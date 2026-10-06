//! Builds the landing page: the same scene the wasm runs, rendered to
//! HTML at build time and poured into `page.html`. The page paints
//! before one byte of wasm arrives, and the boot adopts it — at the
//! size it was laid out in, then at the reader's.
//!
//! ```sh
//! cargo run --release -p landing-web --example render > apps/landing_web/web/index.html
//! ```
//!
//! `LANDING_LOCALE` names the locale the page is built in (a BCP-47
//! list, `en` when unset): the page is laid out in its direction, and
//! the document says the language on its root and its mount.

use bunny_ui::layout::Size;
use bunny_ui::prelude::Locale;

/// The box the page is laid out in: a common desktop window. A reader
/// at another size gets the difference as patches once the wasm runs.
const SERVED: Size = Size { width: 1440.0, height: 900.0 };

fn main() {
    let locale = Locale::parse(&std::env::var("LANDING_LOCALE").unwrap_or_default());
    let page = bunny_ui::ssr::render_in(&landing_web::landing(), SERVED, &locale);
    let version = std::env::var("LANDING_VERSION").unwrap_or_else(|_| "1".into());
    let html = include_str!("../page.html")
        .replace("{css}", &page.css)
        .replace("{html}", &page.html)
        .replace("{lang}", &page.lang)
        .replace("{dir}", page.dir)
        .replace("{version}", &version);
    print!("{html}");
}
