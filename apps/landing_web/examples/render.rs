//! Builds the landing page: the same scene the wasm runs, rendered to
//! HTML at build time and poured into `page.html`. The page paints
//! before one byte of wasm arrives, and the boot adopts it — at the
//! size it was laid out in, then at the reader's.
//!
//! ```sh
//! cargo run --release -p landing-web --example render > apps/landing_web/web/index.html
//! ```

use bunny_ui::layout::Size;

/// The box the page is laid out in: a common desktop window. A reader
/// at another size gets the difference as patches once the wasm runs.
const SERVED: Size = Size { width: 1440.0, height: 900.0 };

fn main() {
    let page = bunny_ui::ssr::render(&landing_web::landing(), SERVED);
    let version = std::env::var("LANDING_VERSION").unwrap_or_else(|_| "1".into());
    let html = include_str!("../page.html")
        .replace("{css}", &page.css)
        .replace("{html}", &page.html)
        .replace("{version}", &version);
    print!("{html}");
}
