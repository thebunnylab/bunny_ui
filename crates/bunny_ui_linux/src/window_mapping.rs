//! Recreate XDG roles on remap without replacing the presentation surface.
//!
//! Weston 13 retains its configured/added state after a null-buffer unmap and
//! does not answer the next empty commit. A fresh role also gives conforming
//! compositors an unambiguous initial configure boundary. The hidden roundtrip
//! has already collected old role events; discard them before proxy addresses
//! can be reused. The `wl_surface`, buffers and public window lifetime survive.

use super::{
    Client, EVQ, Ev, MapState, TAG_DECORATION, TAG_TOPLEVEL, TAG_XDG_SURFACE, WlArgument, arg_i,
    arg_n, arg_o, arg_s, arg_u, construct, destroy, geometry_ops, no_args, request, window_at,
    window_ops,
};
use bunny_ui::window::WindowError;

#[derive(Clone, Copy)]
struct RetiredRole {
    xdg: usize,
    toplevel: usize,
    decoration: usize,
}

impl RetiredRole {
    const fn owns(self, event: &Ev) -> bool {
        match event {
            Ev::SurfaceConfigure { xdg_ptr, .. } => self.xdg != 0 && *xdg_ptr == self.xdg,
            Ev::ToplevelConfigure { toplevel_ptr, .. } | Ev::ToplevelClose { toplevel_ptr } => {
                self.toplevel != 0 && *toplevel_ptr == self.toplevel
            }
            Ev::DecorationMode { decoration_ptr, .. } => {
                self.decoration != 0 && *decoration_ptr == self.decoration
            }
            _ => false,
        }
    }
}

/// A failed allocation leaves the window hidden and safe to retry or close.
pub(super) fn remap(client: &mut Client, window: usize) -> Result<(), WindowError> {
    let wm_base = client.wm_base;
    let xdg_interface = client.protocols.xdg_surface;
    let toplevel_interface = client.protocols.toplevel;
    let decoration_manager = client.decoration_manager;
    let decoration_interface = client.protocols.decoration;
    window_ops::unmap(&mut client.window_ops, window);
    let win = window_at(client, window).ok_or(WindowError::StaleWindow)?;
    let retired = RetiredRole {
        xdg: win.xdg_surface as usize,
        toplevel: win.toplevel as usize,
        decoration: win.decoration as usize,
    };
    EVQ.with(|queue| queue.borrow_mut().retain(|event| !retired.owns(event)));
    win.observed_size = None;
    win.pending_size = None;
    win.decoration_mode = None;
    win.size_authority = geometry_ops::SizeAuthority::Client;
    win.maximized = false;
    win.resizing = false;
    unsafe {
        for proxy in [&mut win.decoration, &mut win.toplevel, &mut win.xdg_surface] {
            if !proxy.is_null() {
                destroy(std::mem::replace(proxy, std::ptr::null_mut()), 0);
            }
        }
        win.xdg_surface = construct(
            wm_base,
            2,
            xdg_interface,
            &mut [arg_n(), arg_o(win.surface)],
            TAG_XDG_SURFACE,
        );
        if win.xdg_surface.is_null() {
            return Err(allocation_error("xdg_wm_base.get_xdg_surface"));
        }
        win.toplevel = construct(
            win.xdg_surface,
            1,
            toplevel_interface,
            &mut [arg_n()],
            TAG_TOPLEVEL,
        );
        if win.toplevel.is_null() {
            return Err(allocation_error("xdg_surface.get_toplevel"));
        }
        request(
            win.toplevel,
            2,
            &mut [WlArgument {
                s: win.title.as_ptr(),
            }],
        );
        request(win.toplevel, 3, &mut [arg_s(&crate::life::app_id())]);
        if let Some((w, h)) = win.min_size {
            request(win.toplevel, 8, &mut [arg_i(w), arg_i(h)]);
        }
        if let Some((w, h)) = win.max_size {
            request(win.toplevel, 7, &mut [arg_i(w), arg_i(h)]);
        }
        if !decoration_manager.is_null() {
            win.decoration = construct(
                decoration_manager,
                1,
                decoration_interface,
                &mut [arg_n(), arg_o(win.toplevel)],
                TAG_DECORATION,
            );
            if win.decoration.is_null() {
                return Err(allocation_error(
                    "zxdg_decoration_manager_v1.get_toplevel_decoration",
                ));
            }
            request(
                win.decoration,
                1,
                &mut [arg_u(if win.scene { 1 } else { 2 })],
            );
        }
        win.map = MapState::AwaitingConfigure;
        // The new role must receive a real configure before any buffer attaches.
        request(win.surface, 6, &mut no_args());
    }
    Ok(())
}

const fn allocation_error(operation: &'static str) -> WindowError {
    WindowError::NativeFailure {
        operation,
        code: -1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_roles_drop_only_their_queued_events_before_address_reuse() {
        let retired = RetiredRole {
            xdg: 11,
            toplevel: 12,
            decoration: 13,
        };
        assert!(retired.owns(&Ev::SurfaceConfigure {
            xdg_ptr: 11,
            serial: 1
        }));
        assert!(retired.owns(&Ev::ToplevelClose { toplevel_ptr: 12 }));
        assert!(retired.owns(&Ev::ToplevelConfigure {
            toplevel_ptr: 12,
            width: 400,
            height: 260,
            states: vec![],
        }));
        assert!(retired.owns(&Ev::DecorationMode {
            decoration_ptr: 13,
            mode: 2
        }));
        assert!(!retired.owns(&Ev::SurfaceConfigure {
            xdg_ptr: 21,
            serial: 2
        }));
        assert!(!retired.owns(&Ev::ToplevelClose { toplevel_ptr: 22 }));
        assert!(!retired.owns(&Ev::FrameDone { callback_ptr: 12 }));
        assert!(!retired.owns(&Ev::SurfaceEnter {
            surface_ptr: 11,
            output_ptr: 13
        }));
    }
}
