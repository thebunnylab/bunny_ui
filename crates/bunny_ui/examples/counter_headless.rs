//! O counter pintado headless — o primeiro elemento "na tela" antes de
//! existir janela: body → identidade → layout → display list → bitmap,
//! impresso como retrato ascii (cada caractere ≈ um bloco de 2×2 px).
//!
//! ```sh
//! cargo run -p bunny-ui --example counter_headless
//! ```

use bunny_ui::layout::Size;
use bunny_ui::prelude::*;
use bunny_ui::raster::Bitmap;

#[derive(Clone, Copy)]
struct Counter {
    count: State<i32>,
}

impl Component for Counter {
    fn body(&self, _ctx: &Context) -> impl View {
        let this = *self;
        vstack((
            text(format!("count: {}", self.count.get())),
            spacer(),
            button(text("tap!"), move || this.count.update(|n| *n += 1)),
        ))
        .alignment(HorizontalAlignment::Leading)
        .padding()
    }
}

fn portrait(bitmap: &Bitmap) -> String {
    let white = bitmap.pixel(0, 0).unwrap();
    let mut out = String::new();
    for y in (0..bitmap.height()).step_by(2) {
        for x in (0..bitmap.width()).step_by(2) {
            let ink = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .any(|&(dx, dy)| {
                    bitmap.pixel(x + dx, y + dy).is_some_and(|pixel| pixel != white)
                });
            out.push(if ink { '#' } else { '.' });
        }
        out.push('\n');
    }
    out
}

fn main() {
    let counter = Counter { count: State::new(0) };
    let runtime = Runtime::new();
    runtime.render_stable(&counter);

    let size = Size { width: 160.0, height: 72.0 };
    println!("── count = 0 ──");
    print!("{}", portrait(&runtime.paint(&counter, size)));

    // três taps depois (o body reconstruído revive o botão pela identidade)
    counter.count.update(|n| *n += 3);
    println!("── count = 3 (após interação) ──");
    print!("{}", portrait(&runtime.paint(&counter, size)));
}
