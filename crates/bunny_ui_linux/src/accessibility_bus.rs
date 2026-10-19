//! Owned libdbus messages at the existing Linux FFI boundary.
//!
//! The bus owns encoding and validation. Connections and messages never leave
//! their UI thread; the shell polls their descriptor alongside its display.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr::NonNull;

use super::super::{DbusError, DbusIter};
use super::super::{
    dbus_bus_get_private, dbus_connection_close, dbus_connection_send_with_reply_and_block,
    dbus_connection_unref, dbus_error_free, dbus_error_init, dbus_message_iter_append_basic,
    dbus_message_iter_get_arg_type, dbus_message_iter_get_basic, dbus_message_iter_init,
    dbus_message_iter_init_append, dbus_message_iter_recurse, dbus_message_new_method_call,
    dbus_message_unref,
};

#[link(name = "dbus-1")]
unsafe extern "C" {
    fn dbus_connection_open_private(address: *const c_char, error: *mut DbusError) -> *mut c_void;
    fn dbus_bus_register(connection: *mut c_void, error: *mut DbusError) -> c_int;
    fn dbus_bus_get_unique_name(connection: *mut c_void) -> *const c_char;
    fn dbus_connection_set_exit_on_disconnect(connection: *mut c_void, exit: c_int);
    fn dbus_connection_get_unix_fd(connection: *mut c_void, fd: *mut c_int) -> c_int;
    fn dbus_connection_get_outgoing_size(connection: *mut c_void) -> std::ffi::c_long;
    fn dbus_connection_read_write(connection: *mut c_void, timeout: c_int) -> c_int;
    fn dbus_connection_pop_message(connection: *mut c_void) -> *mut c_void;
    fn dbus_connection_send(
        connection: *mut c_void,
        message: *mut c_void,
        serial: *mut u32,
    ) -> c_int;
    fn dbus_message_new_method_return(message: *mut c_void) -> *mut c_void;
    fn dbus_message_new_error(
        message: *mut c_void,
        name: *const c_char,
        text: *const c_char,
    ) -> *mut c_void;
    fn dbus_message_new_signal(
        path: *const c_char,
        interface: *const c_char,
        member: *const c_char,
    ) -> *mut c_void;
    fn dbus_message_get_type(message: *mut c_void) -> c_int;
    fn dbus_message_get_path(message: *mut c_void) -> *const c_char;
    fn dbus_message_get_interface(message: *mut c_void) -> *const c_char;
    fn dbus_message_get_member(message: *mut c_void) -> *const c_char;
    fn dbus_message_get_reply_serial(message: *mut c_void) -> u32;
    fn dbus_message_iter_next(iter: *mut DbusIter) -> c_int;
    fn dbus_message_iter_open_container(
        iter: *mut DbusIter,
        kind: c_int,
        signature: *const c_char,
        child: *mut DbusIter,
    ) -> c_int;
    fn dbus_message_iter_close_container(iter: *mut DbusIter, child: *mut DbusIter) -> c_int;
}

fn iter() -> DbusIter {
    DbusIter { opaque: [0; 16] }
}

struct Error(DbusError);
impl Error {
    fn new() -> Self {
        let mut result = Self(DbusError {
            name: std::ptr::null(),
            message: std::ptr::null(),
            dummy: [0; 2],
            padding: std::ptr::null_mut(),
        });
        unsafe { dbus_error_init(&mut result.0) };
        result
    }
    fn reason(&self) -> String {
        unsafe { string(self.0.message) }
    }
}
impl Drop for Error {
    fn drop(&mut self) {
        unsafe { dbus_error_free(&mut self.0) };
    }
}

/// Values used by the AT-SPI interfaces. Container signatures describe their
/// elements, including the empty-array case; structs derive their signature.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    String(String),
    Path(String),
    I32(i32),
    U32(u32),
    I16(i16),
    Bool(bool),
    F64(f64),
    Struct(Vec<Self>),
    DictEntry(Vec<Self>),
    Array(&'static CStr, Vec<Self>),
    Variant(Box<Self>),
}
impl Value {
    pub(super) fn text(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }
    pub(super) fn reference(bus: &str, path: &str) -> Self {
        Self::Struct(vec![Self::text(bus), Self::Path(path.into())])
    }
    pub(super) fn signature(&self) -> String {
        match self {
            Self::String(_) => "s".into(),
            Self::Path(_) => "o".into(),
            Self::I32(_) => "i".into(),
            Self::U32(_) => "u".into(),
            Self::I16(_) => "n".into(),
            Self::Bool(_) => "b".into(),
            Self::F64(_) => "d".into(),
            Self::Variant(_) => "v".into(),
            Self::Array(kind, _) => format!("a{}", kind.to_string_lossy()),
            Self::DictEntry(values) => format!(
                "{{{}}}",
                values.iter().map(Self::signature).collect::<String>()
            ),
            Self::Struct(values) => format!(
                "({})",
                values.iter().map(Self::signature).collect::<String>()
            ),
        }
    }
    fn append(&self, target: &mut DbusIter) -> Result<(), String> {
        let mut basic =
            |kind, pointer| unsafe { dbus_message_iter_append_basic(target, kind, pointer) != 0 };
        let success = match self {
            // D-Bus cannot encode embedded NUL. Replace that scalar explicitly,
            // preserving the remaining label instead of truncating its suffix.
            Self::String(value) | Self::Path(value) => {
                let bytes = CString::new(value.replace('\0', "�")).map_err(|e| e.to_string())?;
                let pointer = bytes.as_ptr();
                basic(
                    if matches!(self, Self::Path(_)) {
                        111
                    } else {
                        115
                    },
                    (&raw const pointer).cast(),
                )
            }
            Self::I32(value) => basic(105, std::ptr::from_ref(value).cast()),
            Self::U32(value) => basic(117, std::ptr::from_ref(value).cast()),
            Self::I16(value) => basic(110, std::ptr::from_ref(value).cast()),
            Self::F64(value) => basic(100, std::ptr::from_ref(value).cast()),
            Self::Bool(value) => {
                let value = u32::from(*value);
                basic(98, (&raw const value).cast())
            }
            Self::Struct(values) => return container(target, 114, None, values),
            Self::DictEntry(values) => return container(target, 101, None, values),
            Self::Array(signature, values) => {
                return container(target, 97, Some(signature), values);
            }
            Self::Variant(value) => {
                let signature = CString::new(value.signature()).map_err(|e| e.to_string())?;
                return container(target, 118, Some(&signature), std::slice::from_ref(value));
            }
        };
        success
            .then_some(())
            .ok_or_else(|| "D-Bus allocation failed".into())
    }
}
fn container(
    parent: &mut DbusIter,
    kind: c_int,
    signature: Option<&CStr>,
    values: &[Value],
) -> Result<(), String> {
    let mut child = iter();
    if unsafe {
        dbus_message_iter_open_container(
            parent,
            kind,
            signature.map_or(std::ptr::null(), CStr::as_ptr),
            &mut child,
        )
    } == 0
    {
        return Err("D-Bus container allocation failed".into());
    }
    for value in values {
        value.append(&mut child)?;
    }
    if unsafe { dbus_message_iter_close_container(parent, &mut child) } == 0 {
        return Err("D-Bus container allocation failed".into());
    }
    Ok(())
}

pub(super) struct Message(NonNull<c_void>);
impl Message {
    fn own(pointer: *mut c_void) -> Result<Self, String> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(|| "D-Bus message allocation failed".into())
    }
    pub(super) fn call(
        bus: &CStr,
        path: &CStr,
        interface: &CStr,
        member: &CStr,
    ) -> Result<Self, String> {
        Self::own(unsafe {
            dbus_message_new_method_call(
                bus.as_ptr(),
                path.as_ptr(),
                interface.as_ptr(),
                member.as_ptr(),
            )
        })
    }
    pub(super) fn signal(path: &str, interface: &CStr, member: &CStr) -> Result<Self, String> {
        let path = CString::new(path).map_err(|e| e.to_string())?;
        Self::own(unsafe {
            dbus_message_new_signal(path.as_ptr(), interface.as_ptr(), member.as_ptr())
        })
    }
    pub(super) fn reply(&self) -> Result<Self, String> {
        Self::own(unsafe { dbus_message_new_method_return(self.0.as_ptr()) })
    }
    pub(super) fn error(&self, name: &CStr, text: &CStr) -> Result<Self, String> {
        Self::own(unsafe { dbus_message_new_error(self.0.as_ptr(), name.as_ptr(), text.as_ptr()) })
    }
    pub(super) fn append(&self, values: &[Value]) -> Result<(), String> {
        let mut target = iter();
        unsafe { dbus_message_iter_init_append(self.0.as_ptr(), &mut target) };
        for value in values {
            value.append(&mut target)?;
        }
        Ok(())
    }
    pub(super) fn kind(&self) -> c_int {
        unsafe { dbus_message_get_type(self.0.as_ptr()) }
    }
    pub(super) fn reply_serial(&self) -> u32 {
        unsafe { dbus_message_get_reply_serial(self.0.as_ptr()) }
    }
    pub(super) fn path(&self) -> String {
        unsafe { string(dbus_message_get_path(self.0.as_ptr())) }
    }
    pub(super) fn interface(&self) -> String {
        unsafe { string(dbus_message_get_interface(self.0.as_ptr())) }
    }
    pub(super) fn member(&self) -> String {
        unsafe { string(dbus_message_get_member(self.0.as_ptr())) }
    }
    pub(super) fn args(&self) -> Result<Vec<Value>, String> {
        let mut source = iter();
        if unsafe { dbus_message_iter_init(self.0.as_ptr(), &mut source) } == 0 {
            return Ok(Vec::new());
        }
        read_values(&mut source, 0)
    }
}
impl Drop for Message {
    fn drop(&mut self) {
        unsafe { dbus_message_unref(self.0.as_ptr()) };
    }
}

fn read_values(source: &mut DbusIter, depth: usize) -> Result<Vec<Value>, String> {
    if depth > 8 {
        return Err("D-Bus argument nesting exceeds supported methods".into());
    }
    let mut values = Vec::new();
    loop {
        let kind = unsafe { dbus_message_iter_get_arg_type(source) };
        let value = match kind {
            0 => break,
            115 | 111 => {
                let mut pointer: *const c_char = std::ptr::null();
                unsafe { dbus_message_iter_get_basic(source, (&raw mut pointer).cast()) };
                let text = unsafe { string(pointer) };
                if kind == 115 {
                    Value::String(text)
                } else {
                    Value::Path(text)
                }
            }
            105 => {
                let mut n = 0i32;
                unsafe { dbus_message_iter_get_basic(source, (&raw mut n).cast()) };
                Value::I32(n)
            }
            117 => {
                let mut n = 0u32;
                unsafe { dbus_message_iter_get_basic(source, (&raw mut n).cast()) };
                Value::U32(n)
            }
            98 => {
                let mut n = 0u32;
                unsafe { dbus_message_iter_get_basic(source, (&raw mut n).cast()) };
                Value::Bool(n != 0)
            }
            114 | 118 => {
                let mut child = iter();
                unsafe { dbus_message_iter_recurse(source, &mut child) };
                let nested = read_values(&mut child, depth + 1)?;
                if kind == 118 {
                    let [only]: [Value; 1] = nested.try_into().map_err(|_| "invalid variant")?;
                    Value::Variant(Box::new(only))
                } else {
                    Value::Struct(nested)
                }
            }
            _ => return Err("D-Bus argument type is not supported by this method".into()),
        };
        values.push(value);
        if values.len() > 16 {
            return Err("too many D-Bus arguments".into());
        }
        if unsafe { dbus_message_iter_next(source) } == 0 {
            break;
        }
    }
    Ok(values)
}

unsafe fn string(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

pub(super) struct Connection(NonNull<c_void>);
impl Connection {
    fn own(pointer: *mut c_void, error: &Error) -> Result<Self, String> {
        let connection = NonNull::new(pointer).ok_or_else(|| error.reason())?;
        unsafe { dbus_connection_set_exit_on_disconnect(connection.as_ptr(), 0) };
        Ok(Self(connection))
    }
    pub(super) fn accessibility() -> Result<Self, String> {
        let address = match std::env::var("AT_SPI_BUS_ADDRESS") {
            Ok(address) if !address.is_empty() => address,
            _ => {
                let mut error = Error::new();
                let session = Self::own(unsafe { dbus_bus_get_private(0, &mut error.0) }, &error)?;
                let request = Message::call(
                    c"org.a11y.Bus",
                    c"/org/a11y/bus",
                    c"org.a11y.Bus",
                    c"GetAddress",
                )?;
                let pointer = unsafe {
                    dbus_connection_send_with_reply_and_block(
                        session.0.as_ptr(),
                        request.0.as_ptr(),
                        500,
                        &mut error.0,
                    )
                };
                let response = NonNull::new(pointer)
                    .map(Message)
                    .ok_or_else(|| error.reason())?;
                let [Value::String(address)]: [Value; 1] = response
                    .args()?
                    .try_into()
                    .map_err(|_| "invalid accessibility bus address")?
                else {
                    return Err("invalid accessibility bus address".into());
                };
                address
            }
        };
        let address = CString::new(address).map_err(|e| e.to_string())?;
        let mut error = Error::new();
        let result = Self::own(
            unsafe { dbus_connection_open_private(address.as_ptr(), &mut error.0) },
            &error,
        )?;
        if unsafe { dbus_bus_register(result.0.as_ptr(), &mut error.0) } == 0 {
            return Err(error.reason());
        }
        Ok(result)
    }
    pub(super) fn name(&self) -> String {
        unsafe { string(dbus_bus_get_unique_name(self.0.as_ptr())) }
    }
    pub(super) fn fd(&self) -> Option<c_int> {
        let mut fd = -1;
        (unsafe { dbus_connection_get_unix_fd(self.0.as_ptr(), &mut fd) } != 0).then_some(fd)
    }
    pub(super) fn wants_write(&self) -> bool {
        unsafe { dbus_connection_get_outgoing_size(self.0.as_ptr()) > 0 }
    }
    pub(super) fn pump(&self) -> bool {
        unsafe { dbus_connection_read_write(self.0.as_ptr(), 0) != 0 }
    }
    pub(super) fn pop(&self) -> Option<Message> {
        NonNull::new(unsafe { dbus_connection_pop_message(self.0.as_ptr()) }).map(Message)
    }
    pub(super) fn send(&self, message: &Message) -> Result<u32, String> {
        let mut serial = 0;
        if unsafe { dbus_connection_send(self.0.as_ptr(), message.0.as_ptr(), &mut serial) } == 0 {
            Err("D-Bus send allocation failed".into())
        } else {
            Ok(serial)
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            dbus_connection_close(self.0.as_ptr());
            dbus_connection_unref(self.0.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_codec_round_trips_method_arguments() {
        let message = Message::call(
            c"org.a11y.Bus",
            c"/org/a11y/bus",
            c"org.a11y.Bus",
            c"GetAddress",
        )
        .unwrap();
        let values = vec![
            Value::text("Dinner 👩‍🚀"),
            Value::I32(-1),
            Value::U32(9),
            Value::Bool(true),
            Value::reference(":1.7", "/org/a11y/atspi/accessible/root"),
            Value::Variant(Box::new(Value::I32(8))),
        ];
        message.append(&values).unwrap();
        assert_eq!(message.args().unwrap(), values);
    }

    #[test]
    fn embedded_nul_preserves_the_label_suffix() {
        let message = Message::call(
            c"org.a11y.Bus",
            c"/org/a11y/bus",
            c"org.a11y.Bus",
            c"GetAddress",
        )
        .unwrap();
        message.append(&[Value::text("before\0after")]).unwrap();
        assert_eq!(message.args().unwrap(), vec![Value::text("before�after")]);
    }
}
