# the landing page

bunny_ui's own landing page, written in bunny_ui. One scene, lowered to
elements: the browser lays the page out, selects its text, scrolls it
and follows its links at home, and the engine only says what the page
is — the same `vstack`, `hstack`, `text` and `image` a desktop window
is made of.

The width decides the shape. The page reads the `Viewport` and picks
two columns or one, how many targets to a row, the gutter that centres
the column and the hero's size; only that body runs again when the
window moves.

The build renders the same scene to HTML (`examples/render.rs`, poured
into `page.html`), so the page paints before the wasm arrives — and a
reader without it, or a crawler, gets every heading, section and link
as real tags. The boot adopts the served elements at the size they were
laid out in and lays out at the reader's from there.

What the page brings itself, in `page.html`: the web fonts (Space
Grotesk, Instrument Sans, IBM Plex Mono), and the hero texture's bleed
and slow drift — a decoration with no geometry the scene needs to know.

Build and run:

```
cargo build --profile web -p landing-web --target wasm32-unknown-unknown
cp ../../target/wasm32-unknown-unknown/web/landing_web.wasm web/
cargo run --release -p landing-web --example render > web/index.html
python3 -m http.server 8873 --directory web
```

Then open http://localhost:8873.

Deploy (Firebase Hosting, project `bunny-ui`, from this directory after
the build above):

```
firebase deploy --only hosting
```

It serves `web/` at https://bunny-ui.web.app. Every file revalidates
(`no-cache`): the served page, the glue and the wasm must be one build,
or the boot adopts a page another scene drew.
