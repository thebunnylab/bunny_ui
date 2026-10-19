//! Native parent requests and single-use Wayland activation tokens.
use super::{
    Client, Iface, Msg, Proxy, Target, arg_n, arg_o, arg_s, arg_u, construct, destroy, is_x11,
    no_args, request, window_of_surface, window_ref, with_client, wl_display_flush,
    wl_display_get_error,
};
use bunny_ui::window::{WindowError, WindowParentKind};
use std::{collections::HashMap, ffi::CStr};

pub(super) const ACTIVATION_TAG_BASE: usize = 0x2000_0000;

pub(super) struct ActivationRequest {
    proxy: *mut Proxy,
    owner: usize,
    surface: usize,
}

#[derive(Default)]
pub(super) struct WindowOps {
    pub(super) activation: *mut Proxy,
    pub(super) input: Option<(u32, usize)>,
    pub(super) keyboard_surface: usize,
    next: usize,
    pending: HashMap<usize, ActivationRequest>,
    parents: HashMap<usize, usize>,
    applied: HashMap<usize, Option<usize>>,
}

impl WindowOps {
    pub(super) fn new(activation: *mut Proxy) -> Self {
        Self {
            activation,
            ..Self::default()
        }
    }

    fn take_owner(&mut self, owner: usize) -> Vec<ActivationRequest> {
        self.pending
            .extract_if(|_, request| request.owner == owner)
            .map(|(_, request)| request)
            .collect()
    }

    fn reserve_id(&mut self) -> Result<usize, WindowError> {
        let id = self
            .next
            .checked_add(1)
            .filter(|id| *id < 0x1000_0000)
            .ok_or(WindowError::Unsupported(
                "Wayland activation identity space exhausted",
            ))?;
        self.next = id;
        Ok(id)
    }

    fn clear_source(&mut self, surface: usize) {
        if self.input.is_some_and(|(_, source)| source == surface) {
            self.input = None;
        }
        if self.keyboard_surface == surface {
            self.keyboard_surface = 0;
        }
    }
}

pub(super) fn activation_spec() -> [(&'static CStr, u32, Vec<Msg>, Vec<Msg>); 2] {
    use Iface::{ActivationToken, CoreSeat, CoreSurface};
    [
        (
            c"xdg_activation_v1",
            1,
            vec![
                Msg(c"destroy", c"", &[]),
                Msg(c"get_activation_token", c"n", &[Some(ActivationToken)]),
                Msg(c"activate", c"so", &[None, Some(CoreSurface)]),
            ],
            vec![],
        ),
        (
            c"xdg_activation_token_v1",
            1,
            vec![
                Msg(c"set_serial", c"uo", &[None, Some(CoreSeat)]),
                Msg(c"set_app_id", c"s", &[None]),
                Msg(c"set_surface", c"o", &[Some(CoreSurface)]),
                Msg(c"commit", c"", &[]),
                Msg(c"destroy", c"", &[]),
            ],
            vec![Msg(c"done", c"s", &[None])],
        ),
    ]
}

pub(super) fn pointer_input(client: &mut Client, serial: u32) {
    let surface = match client.pointer_focus {
        Target::Window(window) => window,
        Target::Panel(index) => client
            .panels
            .get(index)
            .and_then(Option::as_ref)
            .map_or(0, |panel| panel.surface as usize),
        Target::None => 0,
    };
    client.window_ops.input = (surface != 0).then_some((serial, surface));
}

/// Requests the native relationship after validating both live endpoints.
pub fn set_window_parent(
    window: usize,
    parent: Option<usize>,
) -> Result<WindowParentKind, WindowError> {
    if is_x11() {
        return crate::x11::set_window_parent(
            u32::try_from(window).map_err(|_| WindowError::StaleWindow)?,
            parent
                .map(u32::try_from)
                .transpose()
                .map_err(|_| WindowError::StaleWindow)?,
        );
    }
    with_client(|client| {
        if window_ref(client, window).is_none()
            || parent.is_some_and(|id| window_ref(client, id).is_none())
        {
            return Err(WindowError::StaleWindow);
        }
        connection_ready(client, "xdg_toplevel.set_parent")?;
        let native = parent
            .and_then(|id| window_ref(client, id))
            .filter(|win| win.map.mapped())
            .map_or(std::ptr::null_mut(), |win| win.toplevel);
        let child = window_ref(client, window)
            .ok_or(WindowError::StaleWindow)?
            .toplevel;
        if child.is_null() {
            return Err(WindowError::NativeFailure {
                operation: "xdg_toplevel.set_parent: role unavailable after failed remap",
                code: -1,
            });
        }
        unsafe {
            request(child, 1, &mut [arg_o(native)]);
            wl_display_flush(client.display);
        }
        connection_ready(client, "xdg_toplevel.set_parent")?;
        match parent {
            Some(parent) => {
                client.window_ops.parents.insert(window, parent);
            }
            None => {
                client.window_ops.parents.remove(&window);
            }
        }
        client
            .window_ops
            .applied
            .insert(window, parent.filter(|_| !native.is_null()));
        Ok(WindowParentKind::WaylandParent)
    })
}

/// An unmapped parent loses its native relation. Reapply after it maps again.
pub(super) fn restore_parents(client: &mut Client) {
    let changes: Vec<_> = client
        .window_ops
        .parents
        .iter()
        .filter_map(|(&child, &parent)| {
            let child_window = window_ref(client, child)?;
            if child_window.toplevel.is_null() {
                return None;
            }
            let parent_window = window_ref(client, parent).filter(|win| win.map.mapped());
            let value = parent_window.map(|_| parent);
            (client.window_ops.applied.get(&child) != Some(&value)).then_some((
                child,
                child_window.toplevel,
                value,
                parent_window.map_or(std::ptr::null_mut(), |win| win.toplevel),
            ))
        })
        .collect();
    if changes.is_empty() {
        return;
    }
    for (child, toplevel, value, parent) in changes {
        unsafe {
            request(toplevel, 1, &mut [arg_o(parent)]);
        }
        client.window_ops.applied.insert(child, value);
    }
    unsafe {
        wl_display_flush(client.display);
    }
}

/// Requests activation under platform policy, preserving modal eligibility.
pub fn focus_window(window: usize) -> Result<(), WindowError> {
    if is_x11() {
        return crate::x11::focus_window(
            u32::try_from(window).map_err(|_| WindowError::StaleWindow)?,
        );
    }
    with_client(|client| {
        let win = window_ref(client, window).ok_or(WindowError::StaleWindow)?;
        connection_ready(client, "xdg_activation_v1")?;
        if win.map.hidden() {
            return Err(WindowError::HiddenWindow);
        }
        if client.window_ops.activation.is_null() {
            return Err(WindowError::Unsupported(
                "compositor does not advertise xdg_activation_v1",
            ));
        }
        let (serial, source) = client
            .window_ops
            .input
            .filter(|(_, surface)| window_of_surface(client, *surface) != 0)
            .ok_or(WindowError::Unsupported(
                "Wayland activation requires an input or focus event",
            ))?;
        if client.seat.is_null() {
            return Err(WindowError::Unsupported(
                "Wayland activation requires a seat",
            ));
        }
        // Pool creation order is not modal stacking order. Do not activate an
        // underlying scene until this adapter exposes its authoritative stack.
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
        let surface = window;
        let id = client.window_ops.reserve_id()?;
        // One pending token per target; IDs never reuse a destroyed proxy's address.
        cancel(&mut client.window_ops, window);
        let token = unsafe {
            construct(
                client.window_ops.activation,
                1,
                client.protocols.activation_token,
                &mut [arg_n()],
                ACTIVATION_TAG_BASE + id,
            )
        };
        if token.is_null() {
            return Err(WindowError::NativeFailure {
                operation: "get_activation_token",
                code: -1,
            });
        }
        client.window_ops.pending.insert(
            id,
            ActivationRequest {
                proxy: token,
                owner: window,
                surface,
            },
        );
        unsafe {
            request(token, 0, &mut [arg_u(serial), arg_o(client.seat)]);
            request(token, 2, &mut [arg_o(source as *mut Proxy)]);
            request(token, 3, &mut no_args());
            wl_display_flush(client.display);
        }
        if let Err(error) = connection_ready(client, "xdg_activation_v1") {
            cancel(&mut client.window_ops, window);
            return Err(error);
        }
        Ok(())
    })
}

fn connection_ready(client: &Client, operation: &'static str) -> Result<(), WindowError> {
    let code = unsafe { wl_display_get_error(client.display) };
    if code == 0 {
        Ok(())
    } else {
        Err(WindowError::NativeFailure {
            operation,
            code: i64::from(code),
        })
    }
}

pub(super) fn activate(client: &mut Client, id: usize, token: &CStr) {
    let Some(pending) = client.window_ops.pending.remove(&id) else {
        return;
    };
    let live = window_ref(client, pending.owner).is_some_and(|win| !win.map.hidden())
        && window_of_surface(client, pending.surface) == pending.owner;
    unsafe {
        if live {
            request(
                client.window_ops.activation,
                2,
                &mut [arg_s(token), arg_o(pending.surface as *mut Proxy)],
            );
        }
        destroy(pending.proxy, 4);
        wl_display_flush(client.display);
    }
}

pub(super) fn cancel(state: &mut WindowOps, window: usize) {
    for request in state.take_owner(window) {
        unsafe {
            destroy(request.proxy, 4);
        }
    }
}

/// Forget event provenance before native addresses can be reused.
pub(super) fn forget_surface(state: &mut WindowOps, owner: usize, surface: usize) {
    cancel(state, owner);
    state.clear_source(surface);
}

pub(super) fn unmap(state: &mut WindowOps, window: usize) {
    cancel(state, window);
    state.applied.remove(&window);
}

pub(super) fn retire(client: &mut Client, window: usize) {
    forget_surface(&mut client.window_ops, window, window);
    client
        .window_ops
        .parents
        .retain(|&child, parent| child != window && *parent != window);
    client
        .window_ops
        .applied
        .retain(|&child, parent| child != window && *parent != Some(window));
}

pub(super) fn teardown(state: WindowOps) {
    unsafe {
        for request in state.pending.into_values() {
            destroy(request.proxy, 4);
        }
        if !state.activation.is_null() {
            destroy(state.activation, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_activation_ids_cannot_address_a_reopened_surface() {
        let mut state = WindowOps::default();
        let old = state.reserve_id().unwrap();
        let independent = state.reserve_id().unwrap();
        for (id, owner) in [(old, 10), (independent, 20)] {
            state.pending.insert(
                id,
                ActivationRequest {
                    proxy: std::ptr::null_mut(),
                    owner,
                    surface: owner,
                },
            );
        }
        assert_eq!(state.take_owner(10).len(), 1);
        let reopened = state.reserve_id().unwrap();
        state.pending.insert(
            reopened,
            ActivationRequest {
                proxy: std::ptr::null_mut(),
                owner: 10,
                surface: 10,
            },
        );
        assert!(
            !state.pending.contains_key(&old),
            "late done must not activate reused native addresses"
        );
        assert!(state.pending.contains_key(&independent));
        assert!(state.pending.remove(&reopened).is_some());
        assert!(
            state.pending.remove(&reopened).is_none(),
            "tokens are single-use"
        );
        state.next = 0x0fff_ffff;
        assert!(matches!(
            state.reserve_id(),
            Err(WindowError::Unsupported(_))
        ));
    }

    #[test]
    fn destroyed_surface_loses_its_input_provenance() {
        let mut state = WindowOps {
            input: Some((7, 10)),
            keyboard_surface: 10,
            ..WindowOps::default()
        };
        state.clear_source(20);
        assert_eq!(state.input, Some((7, 10)));
        state.clear_source(10);
        assert_eq!(state.input, None);
        assert_eq!(state.keyboard_surface, 0);
    }

    fn attribute<'a>(tag: &'a str, name: &str) -> &'a str {
        tag.split_once(&format!("{name}=\""))
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap()
    }

    // Vendored upstream protocol, with its original license, so the ABI check
    // never silently skips on hosts lacking a staging protocol installation.
    // https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/xdg-activation/xdg-activation-v1.xml
    #[test]
    fn activation_abi_matches_upstream_xml() {
        let xml = include_str!("../tests/fixtures/xdg-activation-v1.xml");
        for (name, version, methods, events) in activation_spec() {
            let start = xml
                .find(&format!("<interface name=\"{}\"", name.to_str().unwrap()))
                .unwrap();
            let block = xml[start..].split("</interface>").next().unwrap();
            assert_eq!(
                attribute(block.split('>').next().unwrap(), "version"),
                version.to_string()
            );
            for (kind, rows) in [("request", methods), ("event", events)] {
                let blocks: Vec<_> = block.split(&format!("<{kind} ")).skip(1).collect();
                assert_eq!(blocks.len(), rows.len(), "every opcode must be present");
                for (block, Msg(name, signature, interfaces)) in blocks.into_iter().zip(rows) {
                    let message = block.split(&format!("</{kind}>")).next().unwrap();
                    assert_eq!(attribute(message, "name"), name.to_str().unwrap());
                    let args: Vec<_> = message
                        .split("<arg ")
                        .skip(1)
                        .map(|arg| arg.split('>').next().unwrap())
                        .collect();
                    let mut expected = String::new();
                    for (arg, iface) in args.iter().zip(interfaces) {
                        expected.push(match attribute(arg, "type") {
                            "uint" => 'u',
                            "string" => 's',
                            "object" => 'o',
                            "new_id" => 'n',
                            other => panic!("unexpected activation argument {other}"),
                        });
                        let expected_interface = match iface {
                            Some(Iface::ActivationToken) => Some("xdg_activation_token_v1"),
                            Some(Iface::CoreSeat) => Some("wl_seat"),
                            Some(Iface::CoreSurface) => Some("wl_surface"),
                            None => None,
                            _ => panic!("unexpected activation interface"),
                        };
                        assert_eq!(arg.contains("interface="), expected_interface.is_some());
                        if let Some(expected) = expected_interface {
                            assert_eq!(attribute(arg, "interface"), expected);
                        }
                    }
                    assert_eq!(args.len(), interfaces.len());
                    assert_eq!(signature.to_str().unwrap(), expected);
                }
            }
        }
    }
}
