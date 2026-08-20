//! Builds the OFFICIAL harness entry: the keyed page, rendered at
//! build time, wearing the harness's own stylesheet. The page paints
//! at HTML-parse time; the wasm boots after and adopts it in silence.
//!
//! ```sh
//! cargo run --release -p bench-web --example render_krausest \
//!   > ../../js-framework-benchmark/frameworks/keyed/bunny-ui/index.html
//! ```

use bunny_ui::layout::Size;

fn main() {
    let page = bunny_ui::ssr::render(
        &bench_web::keyed::app(),
        Size { width: 1200.0, height: 800.0 },
    );
    println!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n    <meta charset=\"utf-8\"/>\n    \
         <title>bunny_ui-\"keyed\"</title>\n    \
         <link href=\"/css/currentStyle.css\" rel=\"stylesheet\"/>\n    \
         <style>\n#app{{position:relative;width:1200px;min-height:800px;overflow:visible}}\n{css}\n</style>\n</head>\n<body>\n\
         {html}\n<script>\n  window.BUNNY_WASM = \"bench_web.wasm\";\n  window.BUNNY_START = \"start_keyed\";\n</script>\n\
         <script src=\"glue_dom.js\"></script>\n</body>\n</html>",
        css = page.css,
        html = page.html,
    );
}
