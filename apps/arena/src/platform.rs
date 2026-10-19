//! Each arena window opens through its platform's production shell.
//!
//! Selecting the shell by target changes nothing about a scene: the same
//! view and the same runtime reach the platform's own text engine, image
//! engine and presenter. The macOS scenes keep their shell and their face;
//! Windows opens the same logical client through DirectWrite and Direct3D.

use std::rc::Rc;

use bunny_ui::prelude::*;
#[cfg(target_os = "macos")]
use bunny_ui_macos::{
    App, CoreGraphicsImageEngine as ImageEngine, CoreTextEngine as TextEngine, WindowSpec,
};
#[cfg(target_os = "windows")]
use bunny_ui_windows::{
    App, DirectWriteEngine as TextEngine, WicImageEngine as ImageEngine, WindowSpec,
};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!("the arena's apps open a macOS or a Windows window");

/// A runtime with the platform's ordinary text and image engines — the
/// runtime the window renders with and the scene observer reads.
#[must_use]
pub fn runtime() -> Rc<Runtime> {
    Rc::new(
        Runtime::new()
            .text_engine(Rc::new(TextEngine::new()))
            .image_engine(Rc::new(ImageEngine::new())),
    )
}

/// Opens the arena's window at its logical size and runs the event loop.
pub fn open(title: &str, runtime: Rc<Runtime>, root: impl View) {
    let app = App::new();
    app.open(
        WindowSpec::titled(title).size(crate::WINDOW.0, crate::WINDOW.1),
        runtime,
        root,
    );
    app.run();
}
