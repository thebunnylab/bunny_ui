//! The Linux shell's side of the Vulkan tier: the registry that holds
//! the one presenter, the two doors (`wl_surface`, xcb window) spoken
//! as a [`SurfaceSource`], the ack road's hooks around a present, and
//! the step down to the gl tier when the device is lost. The tier
//! itself — stack, swapchain, atlas, the parity oracle — lives in
//! `bunny_ui_vulkan`, shared with the Android shell.

use std::cell::{Cell, RefCell};

use bunny_ui::image_engine::ImageEngine;
use bunny_ui::layout::{Color, DisplayList, Size};
use bunny_ui::text_engine::TextEngine;
pub use bunny_ui_vulkan::OffscreenVk;
use bunny_ui_vulkan::{Presented, SurfaceSource, VkPresenter};

thread_local! {
    /// One presenter per WINDOW, by the window's address — a swapchain
    /// belongs to the surface it was made for.
    static PRESENTER: RefCell<std::collections::HashMap<usize, VkPresenter>> =
        RefCell::new(std::collections::HashMap::new());
    static RECREATE_SPENT: Cell<bool> = const { Cell::new(false) };
}

/// The window the shell stands in front of, spoken as the tier's
/// source — and whether it is a scene (the corner mask).
fn source(window: usize) -> Option<(SurfaceSource, bool)> {
    use crate::ffi::GpuTargets;
    Some(match crate::ffi::gpu_targets(window)? {
        GpuTargets::Wayland { display, surface, scene } => {
            (SurfaceSource::Wayland { display, surface }, scene)
        }
        GpuTargets::X11 { connection, window, scene } => {
            (SurfaceSource::Xcb { connection, window }, scene)
        }
    })
}

fn install(window: usize) -> Option<VkPresenter> {
    let (source, scene) = source(window)?;
    let (width, height) = crate::ffi::gpu_buffer_size(window);
    let presenter = VkPresenter::install(source, (width as u32, height as u32), scene)?;
    // the compositor must never grow the window past what the device
    // renders — the same ceiling the gl tier declares
    crate::ffi::gpu_limit_size(window, presenter.max_texture_size() as usize);
    Some(presenter)
}

/// The front of the ladder: vulkan if it fully comes up, else the
/// caller steps down to gl. `BUNNY_PRESENT=gl|cpu` skips this tier
/// before any loader touch.
pub(crate) fn try_install(window: usize) -> bool {
    match std::env::var("BUNNY_PRESENT").ok().as_deref() {
        Some("cpu") | Some("gl") => return false,
        _ => {}
    }
    let Some(presenter) = install(window) else {
        return false;
    };
    PRESENTER.with(|slot| slot.borrow_mut().insert(window, presenter));
    true
}

pub(crate) fn active(window: usize) -> bool {
    PRESENTER.with(|slot| slot.borrow().contains_key(&window))
}

/// The ack road's skip-breaker, same contract as the gl tier's.
pub(crate) fn invalidate(window: usize) {
    PRESENTER.with(|slot| {
        if let Some(presenter) = slot.borrow_mut().get_mut(&window) {
            presenter.invalidate();
        }
    });
}

/// One present inside the shell's envelope: the pre-present (buffer
/// scale, frame callback, "is the window configured") rides before the
/// WSI commit, the note after — the same envelope the CPU commit wears.
fn present_with(
    window: usize,
    presenter: &mut VkPresenter,
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) -> Presented {
    presenter.present(
        display,
        size,
        scale,
        canvas,
        text,
        images,
        &mut |scale| crate::ffi::gpu_pre_present(window, scale),
        &mut || crate::ffi::gpu_note_present(window),
    )
}

pub(crate) fn present_window(
    window: usize,
    display: &DisplayList,
    size: Size,
    scale: usize,
    canvas: Color,
    text: &dyn TextEngine,
    images: &dyn ImageEngine,
) {
    let outcome = PRESENTER.with(|slot| {
        slot.borrow_mut().get_mut(&window).map(|presenter| {
            present_with(window, presenter, display, size, scale, canvas, text, images)
        })
    });
    match outcome {
        None | Some(Presented::Ok) => return,
        // on the desktop a lost surface is a lost window: both losses
        // rebuild the presenter once, then step down
        Some(Presented::SurfaceLost) | Some(Presented::DeviceLost) => {}
    }
    teardown(window);
    if !RECREATE_SPENT.with(|spent| spent.replace(true)) {
        if let Some(mut presenter) = install(window) {
            present_with(window, &mut presenter, display, size, scale, canvas, text, images);
            PRESENTER.with(|slot| slot.borrow_mut().insert(window, presenter));
            return;
        }
    }
    // the gl tier below catches the window for the rest of its life
    eprintln!("bunny_ui vk: the device is lost — stepping down the ladder");
    let _ = crate::gl::try_install(window);
}

/// Lets a window's presenter go — swapchain, surface, device and
/// instance: the window is closing.
pub(crate) fn teardown(window: usize) {
    PRESENTER.with(|slot| drop(slot.borrow_mut().remove(&window)));
}

/// Every window's presenter, at the road's end.
pub(crate) fn teardown_all() {
    PRESENTER.with(|slot| slot.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::prelude::*;
    use bunny_ui::raster::rasterize_with;

    /// One probe, cached: a machine without a Vulkan device skips
    /// honestly.
    fn device_present() -> bool {
        static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let present = *PRESENT.get_or_init(|| OffscreenVk::new(4, 4).is_some());
        if !present {
            eprintln!("no vulkan device — skipping");
        }
        present
    }

    fn assert_close(gpu: &[u8], cpu: &[u8], max_delta: u8, label: &str) {
        assert_eq!(gpu.len(), cpu.len(), "{label}: byte lengths differ");
        let mut worst = 0u8;
        let mut beyond_one = 0usize;
        for (a, b) in gpu.iter().zip(cpu.iter()) {
            let delta = a.abs_diff(*b);
            worst = worst.max(delta);
            if delta > 1 {
                beyond_one += 1;
            }
        }
        assert!(worst <= max_delta, "{label}: worst channel delta {worst} (allowed {max_delta})");
        let share = beyond_one as f64 / gpu.len() as f64;
        assert!(
            share <= 0.01,
            "{label}: {beyond_one} channels beyond one step ({:.3}% > 1%)",
            share * 100.0
        );
    }

    #[test]
    fn freetype_runs_match_within_tolerance() {
        if !device_present() {
            return;
        }
        // the real engine, SAME instance on both sides: identical run
        // rasters in, so only blend rounding may differ
        use std::rc::Rc;
        let engine = crate::text::FreeTypeEngine::new();
        let logical = Size { width: 260.0, height: 100.0 };
        let scale = 2usize;
        let physical = (520, 200);
        let runtime = Runtime::new().text_engine(Rc::new(crate::text::FreeTypeEngine::new()));
        let root = vstack((
            text("Fjord glyphs vex quick waltz"),
            text("bunny_ui presents by vulkan").foreground_color(Color::hex(0x3B82F6)),
        ))
        .padding_length(10.0)
        .background_color(Color::hex(0xFFFFFF))
        .corner_radius(9.0);
        let display = runtime.display_frame(&root, logical);
        let cpu = rasterize_with(
            &display,
            physical.0,
            physical.1,
            scale,
            Color::CANVAS,
            &engine,
            &RawImages::default(),
        )
        .to_rgba_bytes();
        let mut gpu = OffscreenVk::new(physical.0, physical.1).expect("offscreen gpu");
        gpu.present_wait(&display, scale, Color::CANVAS, &engine, &RawImages::default());
        assert_close(&gpu.read_rgba(), &cpu, 2, "freetype runs");
    }
}
