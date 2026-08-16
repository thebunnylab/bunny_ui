//! Shell macOS do bunny-ui: janela nativa, eventos de ponteiro e o ciclo
//! vivo — hover/press → repaint por evento; ação no up-inside → estado →
//! render incremental → blit. Sem uma dependência sequer.
//!
//! O `unsafe` do projeto mora SÓ aqui (a FFI de [`ffi`]), embrulhado nesta
//! API segura. O core e a fachada seguem `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

mod ffi;

use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::Runtime;
use bunny_ui::view::View;

use ffi::AppEvent;

/// Abre a janela e entra no ciclo vivo. Retorna quando o app encerra
/// (fechar a janela encerra).
pub fn run_window(title: &str, size: Size, root: impl View) {
    let window = ffi::create_window(title, size.width, size.height);
    let runtime = Runtime::new();

    // um frame completo: o Runtime estabiliza, faz layout, retém os hits
    // para os eventos de ponteiro e rasteriza — o shell só blita e alinha
    // o cursor
    let blit = move |runtime: &Runtime, root: &_| {
        let (width, height) = window.content_size();
        let bitmap = runtime.frame(root, Size { width, height }, window.scale(), Color::CANVAS);
        window.set_image(bitmap.width(), bitmap.height(), &bitmap.to_rgba_bytes());
        window.set_cursor_pointing(runtime.interaction().hovered.is_some());
    };

    ffi::set_handler(Box::new(move |event| match event {
        AppEvent::Redraw => blit(&runtime, &root),
        AppEvent::MouseMoved { x, y } => {
            if runtime.pointer_moved(x, y) {
                blit(&runtime, &root);
            }
        }
        AppEvent::MouseDown { x, y } => {
            if runtime.pointer_pressed(x, y) {
                blit(&runtime, &root);
            }
        }
        AppEvent::MouseUp { x, y } => {
            // dispara no up-inside; o visual de pressed limpa sempre
            let _ = runtime.pointer_released(x, y);
            blit(&runtime, &root);
        }
        AppEvent::MouseExited => {
            if runtime.pointer_exited() {
                blit(&runtime, &root);
            }
        }
    }));

    // primeiro frame, e o run loop assume
    ffi::dispatch(AppEvent::Redraw);
    ffi::run();
}
