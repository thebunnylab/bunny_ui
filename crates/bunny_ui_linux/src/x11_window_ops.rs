//! ICCCM parent hints and EWMH activation requests.
use super::{
    ATOM_WINDOW, ATOM_WM_TRANSIENT_FOR, ClientMessageEvent, Connection, Cookie, GenericError,
    PROP_MODE_REPLACE, SUBSTRUCTURE_MASKS, XCB_CLIENT_MESSAGE, free, window_ref, with_x,
    xcb_connection_has_error,
};
use bunny_ui::window::{WindowError, WindowParentKind};
use std::ffi::{c_char, c_void};

#[link(name = "xcb")]
unsafe extern "C" {
    fn xcb_change_property_checked(
        connection: *mut Connection,
        mode: u8,
        window: u32,
        property: u32,
        kind: u32,
        format: u8,
        len: u32,
        data: *const c_void,
    ) -> Cookie;
    fn xcb_delete_property_checked(
        connection: *mut Connection,
        window: u32,
        property: u32,
    ) -> Cookie;
    fn xcb_send_event_checked(
        connection: *mut Connection,
        propagate: u8,
        destination: u32,
        mask: u32,
        event: *const c_char,
    ) -> Cookie;
    fn xcb_request_check(connection: *mut Connection, cookie: Cookie) -> *mut GenericError;
}

pub(super) fn checked(
    connection: *mut Connection,
    cookie: Cookie,
    operation: &'static str,
) -> Result<(), WindowError> {
    unsafe {
        let error = xcb_request_check(connection, cookie);
        if !error.is_null() {
            let code = i64::from((*error).error_code);
            free(error.cast());
            return Err(WindowError::NativeFailure { operation, code });
        }
        let code = xcb_connection_has_error(connection);
        if code != 0 {
            return Err(WindowError::NativeFailure {
                operation,
                code: i64::from(code),
            });
        }
    }
    Ok(())
}

/// Requests the native relationship after validating both live endpoints.
pub fn set_window_parent(
    window: u32,
    parent: Option<u32>,
) -> Result<WindowParentKind, WindowError> {
    with_x(|client| {
        if window_ref(client, window).is_none()
            || parent.is_some_and(|id| window_ref(client, id).is_none())
        {
            return Err(WindowError::StaleWindow);
        }
        let cookie = unsafe {
            match parent {
                Some(parent) => xcb_change_property_checked(
                    client.connection,
                    PROP_MODE_REPLACE,
                    window,
                    ATOM_WM_TRANSIENT_FOR,
                    ATOM_WINDOW,
                    32,
                    1,
                    (&raw const parent).cast(),
                ),
                None => {
                    xcb_delete_property_checked(client.connection, window, ATOM_WM_TRANSIENT_FOR)
                }
            }
        };
        checked(client.connection, cookie, "WM_TRANSIENT_FOR")?;
        Ok(WindowParentKind::X11Transient)
    })
}

/// Requests activation under platform policy, preserving modal eligibility.
pub fn focus_window(window: u32) -> Result<(), WindowError> {
    with_x(|client| {
        let win = window_ref(client, window).ok_or(WindowError::StaleWindow)?;
        if !win.mapped {
            return Err(WindowError::HiddenWindow);
        }
        if client
            .panels
            .iter()
            .flatten()
            .any(|panel| panel.owner == window && panel.role.dialog().is_some())
        {
            return Err(WindowError::Unsupported(
                "Linux modal activation is not yet supported",
            ));
        }
        // The WM is the activation authority; never force X input focus.
        let message = ClientMessageEvent {
            response_type: XCB_CLIENT_MESSAGE,
            format: 32,
            sequence: 0,
            window,
            kind: client.atoms.net_active_window,
            data32: [1, client.last_time, client.keyboard_focus, 0, 0],
        };
        let cookie = unsafe {
            xcb_send_event_checked(
                client.connection,
                0,
                client.root,
                SUBSTRUCTURE_MASKS,
                (&raw const message).cast(),
            )
        };
        checked(client.connection, cookie, "_NET_ACTIVE_WINDOW")
    })
}

#[cfg(test)]
mod tests {
    use super::super::{
        close_top_level, create_window, xcb_get_property, xcb_get_property_reply,
        xcb_get_property_value, xcb_get_property_value_length,
    };
    use super::*;

    fn read_parent(window: u32) -> Option<u32> {
        with_x(|client| unsafe {
            let cookie = xcb_get_property(
                client.connection,
                0,
                window,
                ATOM_WM_TRANSIENT_FOR,
                ATOM_WINDOW,
                0,
                1,
            );
            let reply = xcb_get_property_reply(client.connection, cookie, std::ptr::null_mut());
            assert!(!reply.is_null());
            let result = if (*reply).kind == ATOM_WINDOW {
                assert_eq!((*reply).format, 32);
                assert_eq!(xcb_get_property_value_length(reply), 4);
                Some(std::ptr::read_unaligned(
                    xcb_get_property_value(reply).cast::<u32>(),
                ))
            } else {
                assert_eq!((*reply).kind, 0);
                None
            };
            free(reply.cast());
            result
        })
    }

    #[test]
    #[ignore = "Requires an isolated X server; run by check-linux-accessibility.sh"]
    fn relationship_property_round_trip() {
        let options = crate::ffi::WindowOptions {
            scene: false,
            resizable: true,
            minimizable: true,
            min: None,
        };
        let parent = create_window("Relationship property parent", 200.0, 100.0, options);
        let child = create_window("Relationship property child", 200.0, 100.0, options);
        assert_eq!(read_parent(child), None);
        assert_eq!(
            set_window_parent(child, Some(parent)),
            Ok(WindowParentKind::X11Transient)
        );
        assert_eq!(read_parent(child), Some(parent));
        assert_eq!(
            set_window_parent(child, None),
            Ok(WindowParentKind::X11Transient)
        );
        assert_eq!(read_parent(child), None);
        close_top_level(child);
        close_top_level(parent);
        assert_eq!(
            set_window_parent(child, Some(parent)),
            Err(WindowError::StaleWindow)
        );
        println!(
            "X server confirmed WM_TRANSIENT_FOR creation and deletion; compositor behavior unqualified"
        );
    }
}
