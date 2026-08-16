//! Shell macOS do bunny-ui: janela nativa, eventos e o ciclo vivo —
//! clique → hit-test → ação → estado → render incremental → repaint →
//! blit. Sem uma dependência sequer.
//!
//! O `unsafe` do projeto mora SÓ aqui (a FFI de [`ffi`]), embrulhado nesta
//! API segura. O core e a fachada seguem `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

mod ffi;

use std::cell::RefCell;
use std::rc::Rc;

use bunny_ui::layout::{Proposal, Rect, Size, hit_test};
use bunny_ui::prelude::Runtime;
use bunny_ui::raster::rasterize_scaled;
use bunny_ui::view::View;

use ffi::AppEvent;

/// Abre a janela e entra no ciclo vivo. Retorna quando o app encerra
/// (fechar a janela encerra).
pub fn run_window(title: &str, size: Size, root: impl View) {
    let window = ffi::create_window(title, size.width, size.height);
    let runtime = Runtime::new();
    runtime.render_stable(&root);

    // os alvos de interação do último frame — o mapa do hit-test
    let hits: Rc<RefCell<Vec<(String, Rect)>>> = Rc::default();

    let repaint = {
        let hits = Rc::clone(&hits);
        move |runtime: &Runtime, root: &_| {
            let (width, height) = window.content_size();
            let scale = window.scale();
            let result = runtime.layout(
                root,
                Proposal::exact(Size { width, height }),
            );
            let bitmap = rasterize_scaled(
                &result.display,
                (width.round() as usize) * scale,
                (height.round() as usize) * scale,
                scale,
                bunny_ui::layout::Color::WHITE,
            );
            window.set_image(bitmap.width(), bitmap.height(), &bitmap.to_rgba_bytes());
            *hits.borrow_mut() = result.hits;
        }
    };

    ffi::set_handler(Box::new(move |event| match event {
        AppEvent::Redraw => {
            runtime.render_stable(&root);
            repaint(&runtime, &root);
        }
        AppEvent::Click { x, y } => {
            let target = hit_test(&hits.borrow(), x, y).map(str::to_string);
            if let Some(target) = target {
                // ação → estado sujo → o próximo pass re-roda SÓ quem leu
                runtime.activate(&target);
                runtime.render_stable(&root);
                repaint(&runtime, &root);
            }
        }
    }));

    // primeiro frame, e o run loop assume
    ffi::dispatch(AppEvent::Redraw);
    ffi::run();
}
