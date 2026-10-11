//! Checked X11 content placement, using root pixels and current work-area hints.
use super::{
    ClientMessageEvent, Connection, Cookie, GenericError, GenericEvent, PROP_MODE_REPLACE,
    SUBSTRUCTURE_MASKS, XCB_CLIENT_MESSAGE, XClient, free, window_ops, window_ref, with_x,
    xcb_connection_has_error, xcb_get_property, xcb_get_property_reply, xcb_get_property_value,
    xcb_get_property_value_length, xcb_intern_atom, xcb_intern_atom_reply,
    xcb_translate_coordinates, xcb_translate_coordinates_reply,
};
use bunny_ui::window::{
    DesktopPosition, DesktopSpace, WindowError, WindowGeometry, WindowPlacement, WindowSize,
};
use std::ffi::{c_char, c_void};

#[repr(C)]
#[derive(Clone, Copy)]
struct GeometryReply {
    response_type: u8,
    depth: u8,
    sequence: u16,
    length: u32,
    root: u32,
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    border_width: u16,
    pad: [u8; 2],
}

#[repr(C)]
struct TreeReply {
    response_type: u8,
    pad0: u8,
    sequence: u16,
    length: u32,
    root: u32,
    parent: u32,
    children_len: u16,
    pad1: [u8; 14],
}

#[link(name = "xcb")]
unsafe extern "C" {
    fn xcb_query_tree(connection: *mut Connection, window: u32) -> Cookie;
    fn xcb_query_tree_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut TreeReply;
    fn xcb_get_geometry(connection: *mut Connection, drawable: u32) -> Cookie;
    fn xcb_get_geometry_reply(
        connection: *mut Connection,
        cookie: Cookie,
        error: *mut *mut GenericEvent,
    ) -> *mut GeometryReply;
    fn xcb_configure_window_checked(
        connection: *mut Connection,
        window: u32,
        mask: u16,
        values: *const u32,
    ) -> Cookie;
    fn xcb_send_event_checked(
        connection: *mut Connection,
        propagate: u8,
        destination: u32,
        mask: u32,
        event: *const c_char,
    ) -> Cookie;
    fn xcb_change_property_checked(
        connection: *mut Connection,
        mode: u8,
        window: u32,
        property: u32,
        kind: u32,
        format: u8,
        length: u32,
        data: *const c_void,
    ) -> Cookie;
}

fn undecorated_extents(client: &XClient, window: u32) -> Result<[f64; 4], WindowError> {
    unsafe {
        let mut error = std::ptr::null_mut();
        let cookie = xcb_query_tree(client.connection, window);
        let raw = xcb_query_tree_reply(client.connection, cookie, &raw mut error);
        reply(client, raw, error, "QueryTree", |value| {
            if value.parent == client.root {
                Ok([0.0; 4])
            } else {
                Err(WindowError::Unsupported(
                    "reparented X11 window has no frame-extents hint",
                ))
            }
        })
    }
}

// Every caller owns the malloc'd XCB reply/error pair and reads within its
// protocol-defined length before this helper releases it.
unsafe fn reply<T, R>(
    client: &XClient,
    raw: *mut T,
    error: *mut GenericEvent,
    operation: &'static str,
    read: impl FnOnce(&T) -> Result<R, WindowError>,
) -> Result<R, WindowError> {
    let result = if !error.is_null() {
        let code = i64::from(unsafe { (*error.cast::<GenericError>()).error_code });
        Err(WindowError::NativeFailure { operation, code })
    } else if raw.is_null() {
        let code = unsafe { xcb_connection_has_error(client.connection) };
        Err(WindowError::NativeFailure {
            operation,
            code: if code == 0 { -1 } else { i64::from(code) },
        })
    } else {
        read(unsafe { &*raw })
    };
    unsafe {
        free(raw.cast());
        free(error.cast());
    }
    result
}

fn geometry(client: &XClient, window: u32) -> Result<GeometryReply, WindowError> {
    unsafe {
        let mut error = std::ptr::null_mut();
        let cookie = xcb_get_geometry(client.connection, window);
        let raw = xcb_get_geometry_reply(client.connection, cookie, &raw mut error);
        reply(client, raw, error, "GetGeometry", |value| Ok(*value))
    }
}

fn origin(client: &XClient, window: u32) -> Result<(i16, i16), WindowError> {
    unsafe {
        let mut error = std::ptr::null_mut();
        let cookie = xcb_translate_coordinates(client.connection, window, client.root, 0, 0);
        let raw = xcb_translate_coordinates_reply(client.connection, cookie, &raw mut error);
        reply(client, raw, error, "TranslateCoordinates", |value| {
            if value.same_screen == 0 {
                return Err(WindowError::GeometryUnavailable);
            }
            Ok((value.dst_x, value.dst_y))
        })
    }
}

fn observed(client: &XClient, window: u32, scale: usize) -> Result<WindowGeometry, WindowError> {
    let native = geometry(client, window)?;
    let (x, y) = origin(client, window)?;
    let scale = u32::try_from(scale)
        .map(f64::from)
        .map_err(|_| WindowError::GeometryUnavailable)?;
    let size = WindowSize::new(
        f64::from(native.width) / scale,
        f64::from(native.height) / scale,
    )
    .map_err(|_| WindowError::GeometryUnavailable)?;
    let position = DesktopPosition::new(f64::from(x), f64::from(y), DesktopSpace::X11Pixels)?;
    WindowGeometry::new(size, Some(position), scale)
}

/// Queries the actual server content size and root-space origin.
pub fn window_geometry(window: u32) -> Result<WindowGeometry, WindowError> {
    with_x(|client| {
        observed(
            client,
            window,
            window_ref(client, window)
                .ok_or(WindowError::StaleWindow)?
                .scale,
        )
    })
}

fn atom(client: &XClient, name: &std::ffi::CStr) -> Result<u32, WindowError> {
    let length = u16::try_from(name.to_bytes().len())
        .map_err(|_| WindowError::InvalidGeometry("atom name is too long"))?;
    unsafe {
        let mut error = std::ptr::null_mut();
        let cookie = xcb_intern_atom(client.connection, 0, length, name.as_ptr());
        let raw = xcb_intern_atom_reply(client.connection, cookie, &raw mut error);
        reply(client, raw, error, "InternAtom", |value| Ok(value.atom))
    }
}

fn property(
    client: &XClient,
    window: u32,
    name: &std::ffi::CStr,
    kind: u32,
) -> Result<Option<Vec<u32>>, WindowError> {
    let property = atom(client, name)?;
    unsafe {
        let mut error = std::ptr::null_mut();
        let cookie = xcb_get_property(client.connection, 0, window, property, kind, 0, 4096);
        let raw = xcb_get_property_reply(client.connection, cookie, &raw mut error);
        reply(client, raw, error, "GetProperty", |value| {
            if value.kind == 0 {
                return Ok(None);
            }
            let bytes = xcb_get_property_value_length(raw);
            if value.kind != kind
                || value.format != 32
                || value.bytes_after != 0
                || bytes < 0
                || bytes % 4 != 0
            {
                return Err(WindowError::NativeFailure {
                    operation: "invalid geometry property",
                    code: -1,
                });
            }
            let count = usize::try_from(bytes / 4).map_err(|_| WindowError::GeometryUnavailable)?;
            let values = xcb_get_property_value(raw).cast::<u32>();
            Ok(Some(
                (0..count)
                    .map(|index| std::ptr::read_unaligned(values.add(index)))
                    .collect(),
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn work_area(client: &XClient) -> Result<Bounds, WindowError> {
    const CARDINAL: u32 = 6;
    let root = geometry(client, client.root)?;
    let screen = Bounds {
        x: 0.0,
        y: 0.0,
        width: f64::from(root.width),
        height: f64::from(root.height),
    };
    let Some(areas) = property(client, client.root, c"_NET_WORKAREA", CARDINAL)? else {
        return Ok(screen);
    };
    let Some(desktop) = property(client, client.root, c"_NET_CURRENT_DESKTOP", CARDINAL)? else {
        return Ok(screen);
    };
    let [desktop] = desktop.as_slice() else {
        return Err(WindowError::GeometryUnavailable);
    };
    let index = usize::try_from(*desktop)
        .ok()
        .and_then(|index| index.checked_mul(4))
        .ok_or(WindowError::GeometryUnavailable)?;
    let [x, y, width, height] = areas
        .get(index..index.saturating_add(4))
        .ok_or(WindowError::GeometryUnavailable)?
    else {
        return Err(WindowError::GeometryUnavailable);
    };
    let left = f64::from(*x).max(screen.x);
    let top = f64::from(*y).max(screen.y);
    let right = (f64::from(*x) + f64::from(*width)).min(screen.width);
    let bottom = (f64::from(*y) + f64::from(*height)).min(screen.height);
    if right <= left || bottom <= top {
        return Err(WindowError::GeometryUnavailable);
    }
    Ok(Bounds {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn fit(
    content: Bounds,
    work: Bounds,
    extents: [f64; 4],
    minimum: (f64, f64),
    maximum: (f64, f64),
) -> Result<Bounds, WindowError> {
    let [left, right, top, bottom] = extents;
    let width = content
        .width
        .min(work.width - left - right)
        .min(maximum.0)
        .max(minimum.0);
    let height = content
        .height
        .min(work.height - top - bottom)
        .min(maximum.1)
        .max(minimum.1);
    let x = (content.x - left).clamp(
        work.x,
        (work.x + work.width - width - left - right).max(work.x),
    ) + left;
    let y = (content.y - top).clamp(
        work.y,
        (work.y + work.height - height - top - bottom).max(work.y),
    ) + top;
    if !(1.0..=f64::from(u16::MAX)).contains(&width)
        || !(1.0..=f64::from(u16::MAX)).contains(&height)
        || [x, y]
            .iter()
            .any(|value| !(f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(value))
    {
        return Err(WindowError::InvalidGeometry(
            "placement exceeds X11 protocol bounds",
        ));
    }
    Ok(Bounds {
        x,
        y,
        width,
        height,
    })
}

/// Issues a checked placement request, preserving ICCCM size hints and gravity.
pub fn restore_window_placement(
    window: u32,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    with_x(|client| {
        restore(
            client,
            window,
            window_ref(client, window)
                .ok_or(WindowError::StaleWindow)?
                .scale,
            placement,
        )
    })
}

pub fn dialog_geometry(index: usize) -> Result<WindowGeometry, WindowError> {
    with_x(|client| {
        let (window, scale) = dialog(client, index)?;
        observed(client, window, scale)
    })
}

pub fn restore_dialog_placement(
    index: usize,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    with_x(|client| {
        let (window, scale) = dialog(client, index)?;
        restore(client, window, scale, placement)
    })
}

fn dialog(client: &XClient, index: usize) -> Result<(u32, usize), WindowError> {
    let panel = client
        .panels
        .get(index)
        .and_then(Option::as_ref)
        .ok_or(WindowError::StaleWindow)?;
    panel.role.dialog().ok_or(WindowError::StaleWindow)?;
    let scale = window_ref(client, panel.owner)
        .ok_or(WindowError::StaleWindow)?
        .scale;
    Ok((panel.window, scale))
}

fn restore(
    client: &XClient,
    window: u32,
    scale: usize,
    placement: WindowPlacement,
) -> Result<(), WindowError> {
    let current = observed(client, window, scale)?;
    if placement
        .position()
        .is_some_and(|position| position.space() != DesktopSpace::X11Pixels)
    {
        return Err(WindowError::InvalidGeometry(
            "position is not in X11 root pixels",
        ));
    }
    let position = placement
        .position()
        .or_else(|| current.position())
        .ok_or(WindowError::GeometryUnavailable)?;
    let scale = current.scale_factor();
    let size_hints = atom(client, c"WM_SIZE_HINTS")?;
    let mut hints =
        property(client, window, c"WM_NORMAL_HINTS", size_hints)?.unwrap_or_else(|| vec![0; 18]);
    if hints.len() < 15 {
        return Err(WindowError::GeometryUnavailable);
    }
    hints.resize(18, 0);
    let minimum = if hints[0] & (1 << 4) != 0 {
        (f64::from(hints[5].max(1)), f64::from(hints[6].max(1)))
    } else {
        (1.0, 1.0)
    };
    let maximum = if hints[0] & (1 << 5) != 0 {
        (f64::from(hints[7]), f64::from(hints[8]))
    } else {
        (f64::from(u16::MAX), f64::from(u16::MAX))
    };
    if minimum.0 > maximum.0 || minimum.1 > maximum.1 {
        return Err(WindowError::GeometryUnavailable);
    }
    let extents = match property(client, window, c"_NET_FRAME_EXTENTS", 6)? {
        None => undecorated_extents(client, window)?,
        Some(values) => {
            let [left, right, top, bottom] = values.as_slice() else {
                return Err(WindowError::GeometryUnavailable);
            };
            [*left, *right, *top, *bottom].map(f64::from)
        }
    };
    let target = fit(
        Bounds {
            x: position.x().round(),
            y: position.y().round(),
            width: (placement.size().width() * scale).round(),
            height: (placement.size().height() * scale).round(),
        },
        work_area(client)?,
        extents,
        minimum,
        maximum,
    )?;
    send_placement(client, window, target, hints, size_hints)
}

fn send_placement(
    client: &XClient,
    window: u32,
    target: Bounds,
    mut hints: Vec<u32>,
    size_hints: u32,
) -> Result<(), WindowError> {
    let moveresize = atom(client, c"_NET_MOVERESIZE_WINDOW")?;
    let supported = property(client, client.root, c"_NET_SUPPORTED", 4)?
        .is_some_and(|values| values.contains(&moveresize));
    let values = [
        u32::from_ne_bytes(native_integer(target.x)?.to_ne_bytes()),
        u32::from_ne_bytes(native_integer(target.y)?.to_ne_bytes()),
        u32::try_from(native_integer(target.width)?)
            .map_err(|_| WindowError::GeometryUnavailable)?,
        u32::try_from(native_integer(target.height)?)
            .map_err(|_| WindowError::GeometryUnavailable)?,
    ];
    unsafe {
        if supported {
            // StaticGravity, all four fields, source=application. A manager
            // can still decline the request; only observations report facts.
            let message = ClientMessageEvent {
                response_type: XCB_CLIENT_MESSAGE,
                format: 32,
                sequence: 0,
                window,
                kind: moveresize,
                data32: [
                    0b1010 | (0b1111 << 8) | (1 << 12),
                    values[0],
                    values[1],
                    values[2],
                    values[3],
                ],
            };
            let cookie = xcb_send_event_checked(
                client.connection,
                0,
                client.root,
                SUBSTRUCTURE_MASKS,
                (&raw const message).cast(),
            );
            window_ops::checked(client.connection, cookie, "_NET_MOVERESIZE_WINDOW")
        } else {
            // ICCCM fallback: StaticGravity makes the requested origin refer
            // to the client, including when a legacy manager reparents it.
            hints[0] |= 1 << 9;
            hints[17] = 10;
            let property = atom(client, c"WM_NORMAL_HINTS")?;
            let cookie = xcb_change_property_checked(
                client.connection,
                PROP_MODE_REPLACE,
                window,
                property,
                size_hints,
                32,
                18,
                hints.as_ptr().cast(),
            );
            window_ops::checked(client.connection, cookie, "WM_NORMAL_HINTS gravity")?;
            let cookie =
                xcb_configure_window_checked(client.connection, window, 15, values.as_ptr());
            window_ops::checked(client.connection, cookie, "ConfigureWindow")
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "Only finite integral values within i32 bounds reach the native protocol conversion"
)]
fn native_integer(value: f64) -> Result<i32, WindowError> {
    if value.is_finite()
        && value.fract() == 0.0
        && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value)
    {
        Ok(value as i32)
    } else {
        Err(WindowError::InvalidGeometry(
            "non-integral or out-of-range native coordinate",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_display_and_oversized_content_keep_caption_in_work_area() {
        let work = Bounds {
            x: 0.0,
            y: 30.0,
            width: 1000.0,
            height: 700.0,
        };
        let saved = Bounds {
            x: -1800.0,
            y: -200.0,
            width: 1600.0,
            height: 1000.0,
        };
        assert_eq!(
            fit(
                saved,
                work,
                [5.0, 5.0, 25.0, 5.0],
                (1.0, 1.0),
                (65535.0, 65535.0)
            )
            .unwrap(),
            Bounds {
                x: 5.0,
                y: 55.0,
                width: 990.0,
                height: 670.0
            }
        );
        let constrained = fit(
            saved,
            work,
            [5.0, 5.0, 25.0, 5.0],
            (1200.0, 800.0),
            (65535.0, 65535.0),
        )
        .unwrap();
        assert_eq!((constrained.x, constrained.y), (5.0, 55.0));
        assert_eq!((constrained.width, constrained.height), (1200.0, 800.0));
    }
}
