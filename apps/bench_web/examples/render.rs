//! Builds the bench page: the same scene the wasm runs, rendered to
//! HTML at build time. The output goes to web/index.html — the page
//! paints before one byte of wasm arrives, and the boot adopts it.
//!
//! ```sh
//! cargo run --release -p bench-web --example render > web/index.html
//! ```

use bunny_ui::layout::Size;

fn main() {
    let page = bunny_ui::ssr::render_document(
        &bench_web::bench(),
        Size { width: 760.0, height: 640.0 },
        "bench_web.wasm",
        "glue_dom.js?v=5",
    );
    println!("{page}");
}
