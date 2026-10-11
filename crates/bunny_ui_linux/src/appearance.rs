//! Live portal appearance on the UI thread, shared by X11 and Wayland.
//!
//! Subscription precedes the initial Read. The existing display poll drives
//! libdbus watches; no thread or timer samples desktop settings. Service loss
//! clears the observation, while name reacquisition refreshes the current value.
//! A missing/disconnected session bus stays unavailable until process restart.
//! Protocol: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html>

use super::{
    AppEvent, PollFd,
    bus::{Connection, Message, Value},
};
use bunny_ui::theme::{self, Appearance};
use std::cell::RefCell;
use std::ffi::CString;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

struct Monitor {
    connection: Connection,
    owner: String,
    appearance: Option<Appearance>,
}
impl Monitor {
    fn new() -> Result<Self, String> {
        let connection = Connection::session()?;
        for rule in [
            "type='signal',sender='org.freedesktop.portal.Desktop',path='/org/freedesktop/portal/desktop',interface='org.freedesktop.portal.Settings',member='SettingChanged'",
            "type='signal',sender='org.freedesktop.DBus',path='/org/freedesktop/DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.portal.Desktop'",
        ] {
            let call = Message::call(
                c"org.freedesktop.DBus",
                c"/org/freedesktop/DBus",
                c"org.freedesktop.DBus",
                c"AddMatch",
            )?;
            call.append(&[Value::text(rule)])?;
            connection.call(&call)?;
        }
        let query = Message::call(
            c"org.freedesktop.DBus",
            c"/org/freedesktop/DBus",
            c"org.freedesktop.DBus",
            c"GetNameOwner",
        )?;
        query.append(&[Value::text(PORTAL)])?;
        let owner = connection
            .call(&query)
            .and_then(|reply| reply.args())
            .ok()
            .and_then(|values| match values.as_slice() {
                [Value::String(owner)] => Some(owner.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let mut result = Self {
            connection,
            owner,
            appearance: None,
        };
        if result.owner.is_empty() {
            // The first Read may activate an installed portal. Later name loss
            // remains None until the bus announces an owner, avoiding retries.
            if let Ok(call) = Message::call(
                c"org.freedesktop.portal.Desktop",
                c"/org/freedesktop/portal/desktop",
                c"org.freedesktop.portal.Settings",
                c"Read",
            ) && call
                .append(&[Value::text(NAMESPACE), Value::text(KEY)])
                .is_ok()
                && let Ok(reply) = result.connection.call(&call)
            {
                result.owner = reply.sender();
                result.appearance = reply.args().ok().and_then(|values| read_scheme(&values));
            }
        } else {
            result.refresh();
        }
        Ok(result)
    }
    fn refresh(&mut self) {
        self.appearance = None;
        if self.owner.is_empty() {
            return;
        }
        let value = (|| {
            let owner = CString::new(self.owner.as_str()).map_err(|e| e.to_string())?;
            let call = Message::call(
                &owner,
                c"/org/freedesktop/portal/desktop",
                c"org.freedesktop.portal.Settings",
                c"Read",
            )?;
            call.append(&[Value::text(NAMESPACE), Value::text(KEY)])?;
            self.connection.call(&call)?.args()
        })();
        match value {
            Ok(values) => self.appearance = read_scheme(&values),
            Err(error) => eprintln!("bunny_ui_linux: portal appearance unavailable: {error}"),
        }
    }
    fn receive(&mut self, message: &Message) {
        if message.kind() != 4 {
            return;
        }
        let Ok(args) = message.args() else {
            return;
        };
        if message.sender() == "org.freedesktop.DBus"
            && message.path() == "/org/freedesktop/DBus"
            && message.interface() == "org.freedesktop.DBus"
            && message.member() == "NameOwnerChanged"
        {
            if let [Value::String(name), Value::String(_), Value::String(owner)] = args.as_slice()
                && name == PORTAL
                && *owner != self.owner
            {
                self.owner.clone_from(owner);
                self.refresh();
            }
        } else if !self.owner.is_empty()
            && message.sender() == self.owner
            && message.path() == PATH
            && message.interface() == SETTINGS
            && message.member() == "SettingChanged"
            && let Some(scheme) = changed_scheme(&args)
        {
            self.appearance = decode(scheme);
        }
    }
}
const fn decode(value: u32) -> Option<Appearance> {
    match value {
        1 => Some(Appearance::Dark),
        2 => Some(Appearance::Light),
        _ => None,
    }
}
fn read_scheme(values: &[Value]) -> Option<Appearance> {
    let [Value::Variant(value)] = values else {
        return None;
    };
    // Read historically returns two variant layers; accept the intended single
    // layer as well, without interpreting unrelated types as integer storage.
    let value = match value.as_ref() {
        Value::Variant(inner) => inner.as_ref(),
        value => value,
    };
    match value {
        Value::U32(value) => decode(*value),
        _ => None,
    }
}
fn changed_scheme(values: &[Value]) -> Option<u32> {
    let [
        Value::String(namespace),
        Value::String(key),
        Value::Variant(value),
    ] = values
    else {
        return None;
    };
    if namespace != NAMESPACE || key != KEY {
        return None;
    }
    match value.as_ref() {
        Value::U32(value) => Some(*value),
        _ => None,
    }
}
thread_local! { static MONITOR: RefCell<Option<Result<Monitor, String>>> = const { RefCell::new(None) }; }

/// Subscribes and observes before the first native frame, once per UI thread.
pub fn install() {
    let appearance = MONITOR.with_borrow_mut(|slot| {
        let monitor = slot.get_or_insert_with(|| {
            Monitor::new().inspect_err(|error| {
                eprintln!("bunny_ui_linux: portal observation unavailable: {error}");
            })
        });
        monitor.as_ref().ok().and_then(|monitor| monitor.appearance)
    });
    theme::observe_system_appearance(appearance);
}
/// Adds current libdbus watches to the existing display poll.
pub fn prepare(fds: &mut Vec<PollFd>) {
    MONITOR.with_borrow(|slot| {
        if let Some(Ok(monitor)) = slot {
            monitor.connection.prepare(fds);
        }
    });
}
/// Delivers readiness to libdbus without dispatching application code.
pub fn after(fds: &[PollFd]) {
    MONITOR.with_borrow(|slot| {
        if let Some(Ok(monitor)) = slot {
            monitor.connection.after(fds);
        }
    });
}
/// Drains a bounded batch and delivers the latest observation outside bus borrows.
pub fn pump() {
    let appearance = MONITOR.with_borrow_mut(|slot| {
        let Some(Ok(monitor)) = slot else {
            return None;
        };
        if !monitor.connection.pump() {
            *slot = Some(Err("session bus disconnected".into()));
            return None;
        }
        for _ in 0..64 {
            let Some(message) = monitor.connection.pop() else {
                return monitor.appearance;
            };
            monitor.receive(&message);
        }
        super::wake_from_any_thread();
        monitor.appearance
    });
    // No bus/runtime/client borrow crosses user rendering or window creation.
    if theme::system_appearance() != appearance {
        super::dispatch_at(0, AppEvent::AppearanceChanged(appearance));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_values_preserve_unavailable_and_reject_wrong_shapes() {
        for (number, expected) in [
            (0, None),
            (1, Some(Appearance::Dark)),
            (2, Some(Appearance::Light)),
            (99, None),
        ] {
            let value = Value::Variant(Box::new(Value::U32(number)));
            assert_eq!(read_scheme(std::slice::from_ref(&value)), expected);
            assert_eq!(
                read_scheme(&[Value::Variant(Box::new(value.clone()))]),
                expected
            );
            assert_eq!(
                changed_scheme(&[Value::text(NAMESPACE), Value::text(KEY), value]),
                Some(number)
            );
        }
        assert_eq!(read_scheme(&[Value::text("1")]), None);
        for args in [
            vec![],
            vec![Value::text(NAMESPACE), Value::text(KEY), Value::U32(1)],
            vec![
                Value::text(NAMESPACE),
                Value::text(KEY),
                Value::Variant(Box::new(Value::text("1"))),
            ],
            vec![
                Value::text("other"),
                Value::text(KEY),
                Value::Variant(Box::new(Value::U32(1))),
            ],
        ] {
            assert_eq!(changed_scheme(&args), None);
        }
    }
}
