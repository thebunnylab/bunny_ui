//! Shell macOS do bunny-ui: janela nativa, eventos de ponteiro e o ciclo
//! vivo — hover/press → repaint por evento; ação no up-inside → estado →
//! render incremental → blit. Sem uma dependência sequer.
//!
//! O `unsafe` do projeto mora SÓ aqui (a FFI de [`ffi`]), embrulhado nesta
//! API segura. O core e a fachada seguem `#![forbid(unsafe_code)]`.

#![cfg(target_os = "macos")]

mod ffi;
mod text;

use std::rc::Rc;

use bunny_ui::layout::{Color, Size};
use bunny_ui::prelude::Runtime;
use bunny_ui::view::View;

use ffi::AppEvent;
pub use text::CoreTextEngine;

/// Abre a janela e entra no ciclo vivo. Retorna quando o app encerra
/// (fechar a janela encerra).
pub fn run_window(title: &str, size: Size, root: impl View) {
    // texto de verdade: o engine da plataforma entra no lugar do PixelFont
    let runtime = Runtime::new().text_engine(Rc::new(CoreTextEngine::new()));
    run_window_with(title, size, runtime, root)
}

/// Como [`run_window`], mas com o `Runtime` montado pelo caller — o
/// caminho de apps com environment próprio (o engine de texto ainda é
/// responsabilidade de quem monta).
pub fn run_window_with(title: &str, size: Size, runtime: Runtime, root: impl View) {
    let window = ffi::create_window(title, size.width, size.height);

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
        AppEvent::Wheel { x, y, dx, dy } => {
            // offset é estado do engine: repaint sem render (zero bodies)
            if runtime.wheel(x, y, dx, dy) {
                blit(&runtime, &root);
            }
        }
    }));

    // primeiro frame, e o run loop assume
    ffi::dispatch(AppEvent::Redraw);
    ffi::run();
}
