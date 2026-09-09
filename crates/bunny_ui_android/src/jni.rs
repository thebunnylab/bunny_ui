//! JNI by hand — the function table, a handful of typed calls, and
//! the few things the shell must ask Java for: the window's insets
//! (the bars, the cutout, the keyboard), the reduce-motion setting,
//! the clipboard, and an edge-to-edge window. Not a single dependency:
//! `JNIEnv` is a pointer to a table of 233 function pointers, and the
//! table is declared here slot by slot, typed where the shell calls
//! and opaque everywhere else.
//!
//! The env is the UI thread's own, handed over with the activity, and
//! every call here runs on that thread — the only one that runs the
//! shell's code. Every call is followed by an exception check: a
//! pending Java exception poisons the thread, so it is described to
//! logcat, cleared, and answered as "nothing" here. Every function
//! that touches Java opens a local frame, so the references it made
//! die with it — the table has room for a few hundred, and the text
//! engine calls thousands of times a frame.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::null_mut;

pub type JObject = *mut c_void;
pub type JClass = JObject;
pub type JString = JObject;
pub type JMethodId = *mut c_void;
pub type JFieldId = *mut c_void;
type JBoolean = u8;
type JInt = i32;
type JFloat = f32;

/// A call argument — one slot of the `…A` variants' array.
#[repr(C)]
#[derive(Clone, Copy)]
pub union JValue {
    pub z: u8,
    pub b: i8,
    pub c: u16,
    pub s: i16,
    pub i: i32,
    pub j: i64,
    pub f: f32,
    pub d: f64,
    pub l: JObject,
}

pub type JniEnv = *const JniNativeInterface;

type Fn1<A, R> = unsafe extern "C" fn(*mut JniEnv, A) -> R;
type Fn2<A, B, R> = unsafe extern "C" fn(*mut JniEnv, A, B) -> R;
type Fn3<A, B, C, R> = unsafe extern "C" fn(*mut JniEnv, A, B, C) -> R;
type CallA<R> = Fn3<JObject, JMethodId, *const JValue, R>;
type MemberId = Fn3<JClass, *const c_char, *const c_char, JMethodId>;

/// `JNINativeInterface`: 233 slots in the header's order. The slot
/// numbers in the comments are the ones the shell counted in `jni.h`;
/// the assert below keeps the shape honest.
#[repr(C)]
pub struct JniNativeInterface {
    reserved: [*const c_void; 4],
    get_version: *const c_void,
    define_class: *const c_void,
    find_class: Fn1<*const c_char, JClass>, // 6
    slots_7_14: [*const c_void; 8],
    exception_occurred: Fn1<(), JObject>, // 15 (takes only the env)
    exception_describe: unsafe extern "C" fn(*mut JniEnv), // 16
    exception_clear: unsafe extern "C" fn(*mut JniEnv),    // 17
    fatal_error: *const c_void,
    push_local_frame: Fn1<JInt, JInt>,   // 19
    pop_local_frame: Fn1<JObject, JObject>, // 20
    new_global_ref: Fn1<JObject, JObject>,  // 21
    delete_global_ref: Fn1<JObject, ()>,    // 22
    delete_local_ref: Fn1<JObject, ()>,     // 23
    slots_24_29: [*const c_void; 6],
    new_object_a: CallA<JObject>,           // 30
    get_object_class: Fn1<JObject, JClass>, // 31
    slot_32: *const c_void,
    get_method_id: MemberId, // 33
    slots_34_35: [*const c_void; 2],
    call_object_method_a: CallA<JObject>, // 36
    slots_37_38: [*const c_void; 2],
    call_boolean_method_a: CallA<JBoolean>, // 39
    slots_40_50: [*const c_void; 11],
    call_int_method_a: CallA<JInt>, // 51
    slots_52_53: [*const c_void; 2],
    call_long_method_a: CallA<i64>, // 54
    slots_55_56: [*const c_void; 2],
    call_float_method_a: CallA<JFloat>, // 57
    slots_58_62: [*const c_void; 5],
    call_void_method_a: CallA<()>, // 63
    slots_64_93: [*const c_void; 30],
    get_field_id: Fn3<JClass, *const c_char, *const c_char, JFieldId>, // 94
    slots_95_99: [*const c_void; 5],
    get_int_field: Fn2<JObject, JFieldId, JInt>, // 100
    slot_101: *const c_void,
    get_float_field: Fn2<JObject, JFieldId, JFloat>, // 102
    slots_103_112: [*const c_void; 10],
    get_static_method_id: MemberId, // 113
    slots_114_115: [*const c_void; 2],
    call_static_object_method_a: CallA<JObject>, // 116
    slots_117_130: [*const c_void; 14],
    call_static_int_method_a: CallA<JInt>, // 131
    slots_132_136: [*const c_void; 5],
    call_static_float_method_a: CallA<JFloat>, // 137
    slots_138_142: [*const c_void; 5],
    call_static_void_method_a: CallA<()>, // 143
    get_static_field_id: Fn3<JClass, *const c_char, *const c_char, JFieldId>, // 144
    get_static_object_field: Fn2<JClass, JFieldId, JObject>,                 // 145
    slots_146_149: [*const c_void; 4],
    get_static_int_field: Fn2<JClass, JFieldId, JInt>, // 150
    slots_151_166: [*const c_void; 16],
    new_string_utf: Fn1<*const c_char, JString>, // 167
    slot_168: *const c_void,
    get_string_utf_chars: Fn2<JString, *mut JBoolean, *const c_char>, // 169
    release_string_utf_chars: Fn2<JString, *const c_char, ()>,        // 170
    get_array_length: Fn1<JObject, JInt>,                             // 171
    slots_172_175: [*const c_void; 4],
    new_byte_array: Fn1<JInt, JObject>, // 176
    slots_177_180: [*const c_void; 4],
    new_float_array: Fn1<JInt, JObject>, // 181
    slots_182_202: [*const c_void; 21],
    get_int_array_region: unsafe extern "C" fn(*mut JniEnv, JObject, JInt, JInt, *mut JInt), // 203
    slot_204: *const c_void,
    get_float_array_region: unsafe extern "C" fn(*mut JniEnv, JObject, JInt, JInt, *mut JFloat), // 205
    slots_206_207: [*const c_void; 2],
    set_byte_array_region: unsafe extern "C" fn(*mut JniEnv, JObject, JInt, JInt, *const i8), // 208
    slots_209_227: [*const c_void; 19],
    exception_check: unsafe extern "C" fn(*mut JniEnv) -> JBoolean, // 228
    slots_229_232: [*const c_void; 4],
}

const _: () = assert!(
    std::mem::size_of::<JniNativeInterface>() == 233 * std::mem::size_of::<*const c_void>()
);

thread_local! {
    /// The UI thread's env, handed over with the activity.
    static ENV: Cell<*mut JniEnv> = const { Cell::new(null_mut()) };
    /// The activity instance — a global reference the framework owns.
    static ACTIVITY: Cell<JObject> = const { Cell::new(null_mut()) };
    /// Classes by name, as global references: found once.
    static CLASSES: RefCell<HashMap<&'static CStr, JClass>> = RefCell::new(HashMap::new());
}

/// Hands the shell the UI thread's env and the activity — from the
/// entry, before anything asks Java for a thing.
pub fn install(env: *mut c_void, activity: JObject) {
    ENV.with(|slot| slot.set(env.cast()));
    ACTIVITY.with(|slot| slot.set(activity));
    CLASSES.with(|classes| classes.borrow_mut().clear());
}

/// The env, on the thread that owns it.
#[derive(Clone, Copy)]
pub struct Env {
    raw: *mut JniEnv,
}

// the calls the text and image engines will make are declared with the
// rest, ahead of their first caller
#[allow(dead_code)]
impl Env {
    pub fn current() -> Option<Env> {
        let raw = ENV.with(Cell::get);
        (!raw.is_null()).then_some(Env { raw })
    }

    /// The activity instance.
    pub fn activity(&self) -> JObject {
        ACTIVITY.with(Cell::get)
    }

    /// The `JNIEnv*` itself, for the platform's C functions that take one.
    pub fn raw(&self) -> *mut c_void {
        self.raw.cast()
    }

    fn table(&self) -> &JniNativeInterface {
        unsafe { &**self.raw }
    }

    /// `true` when the thread is clean; a pending exception is
    /// described to logcat and cleared, and answers `false`.
    pub fn check(&self) -> bool {
        unsafe {
            if (self.table().exception_check)(self.raw) == 0 {
                return true;
            }
            (self.table().exception_describe)(self.raw);
            (self.table().exception_clear)(self.raw);
        }
        false
    }

    /// A class by its JNI name (`android/view/View`), found once and
    /// kept as a global reference.
    pub fn class(&self, name: &'static CStr) -> Option<JClass> {
        if let Some(&class) = CLASSES.with(|classes| classes.borrow().get(name).copied()).as_ref() {
            return Some(class);
        }
        let local = unsafe { (self.table().find_class)(self.raw, name.as_ptr()) };
        if !self.check() || local.is_null() {
            aerr!("jni: no class {}", name.to_string_lossy());
            return None;
        }
        let global = unsafe { (self.table().new_global_ref)(self.raw, local) };
        unsafe { (self.table().delete_local_ref)(self.raw, local) };
        if global.is_null() {
            return None;
        }
        CLASSES.with(|classes| classes.borrow_mut().insert(name, global));
        Some(global)
    }

    pub fn method(&self, class: JClass, name: &CStr, signature: &CStr) -> Option<JMethodId> {
        let id = unsafe { (self.table().get_method_id)(self.raw, class, name.as_ptr(), signature.as_ptr()) };
        (self.check() && !id.is_null()).then_some(id)
    }

    pub fn static_method(&self, class: JClass, name: &CStr, signature: &CStr) -> Option<JMethodId> {
        let id = unsafe {
            (self.table().get_static_method_id)(self.raw, class, name.as_ptr(), signature.as_ptr())
        };
        (self.check() && !id.is_null()).then_some(id)
    }

    pub fn field(&self, class: JClass, name: &CStr, signature: &CStr) -> Option<JFieldId> {
        let id = unsafe { (self.table().get_field_id)(self.raw, class, name.as_ptr(), signature.as_ptr()) };
        (self.check() && !id.is_null()).then_some(id)
    }

    pub fn static_field(&self, class: JClass, name: &CStr, signature: &CStr) -> Option<JFieldId> {
        let id = unsafe {
            (self.table().get_static_field_id)(self.raw, class, name.as_ptr(), signature.as_ptr())
        };
        (self.check() && !id.is_null()).then_some(id)
    }

    /// An object-returning call; `None` on an exception, and for a
    /// `null` answer.
    pub fn call_object(&self, object: JObject, method: JMethodId, args: &[JValue]) -> Option<JObject> {
        let answer = unsafe { (self.table().call_object_method_a)(self.raw, object, method, args.as_ptr()) };
        (self.check() && !answer.is_null()).then_some(answer)
    }

    pub fn call_int(&self, object: JObject, method: JMethodId, args: &[JValue]) -> Option<i32> {
        let answer = unsafe { (self.table().call_int_method_a)(self.raw, object, method, args.as_ptr()) };
        self.check().then_some(answer)
    }

    pub fn call_long(&self, object: JObject, method: JMethodId, args: &[JValue]) -> Option<i64> {
        let answer = unsafe { (self.table().call_long_method_a)(self.raw, object, method, args.as_ptr()) };
        self.check().then_some(answer)
    }

    pub fn call_float(&self, object: JObject, method: JMethodId, args: &[JValue]) -> Option<f32> {
        let answer = unsafe { (self.table().call_float_method_a)(self.raw, object, method, args.as_ptr()) };
        self.check().then_some(answer)
    }

    pub fn call_bool(&self, object: JObject, method: JMethodId, args: &[JValue]) -> Option<bool> {
        let answer = unsafe { (self.table().call_boolean_method_a)(self.raw, object, method, args.as_ptr()) };
        self.check().then_some(answer != 0)
    }

    pub fn call_void(&self, object: JObject, method: JMethodId, args: &[JValue]) -> bool {
        unsafe { (self.table().call_void_method_a)(self.raw, object, method, args.as_ptr()) };
        self.check()
    }

    pub fn call_static_object(&self, class: JClass, method: JMethodId, args: &[JValue]) -> Option<JObject> {
        let answer =
            unsafe { (self.table().call_static_object_method_a)(self.raw, class, method, args.as_ptr()) };
        (self.check() && !answer.is_null()).then_some(answer)
    }

    pub fn call_static_int(&self, class: JClass, method: JMethodId, args: &[JValue]) -> Option<i32> {
        let answer = unsafe { (self.table().call_static_int_method_a)(self.raw, class, method, args.as_ptr()) };
        self.check().then_some(answer)
    }

    pub fn call_static_float(&self, class: JClass, method: JMethodId, args: &[JValue]) -> Option<f32> {
        let answer =
            unsafe { (self.table().call_static_float_method_a)(self.raw, class, method, args.as_ptr()) };
        self.check().then_some(answer)
    }

    pub fn call_static_void(&self, class: JClass, method: JMethodId, args: &[JValue]) -> bool {
        unsafe { (self.table().call_static_void_method_a)(self.raw, class, method, args.as_ptr()) };
        self.check()
    }

    pub fn new_object(&self, class: JClass, constructor: JMethodId, args: &[JValue]) -> Option<JObject> {
        let object = unsafe { (self.table().new_object_a)(self.raw, class, constructor, args.as_ptr()) };
        (self.check() && !object.is_null()).then_some(object)
    }

    pub fn int_field(&self, object: JObject, field: JFieldId) -> Option<i32> {
        let value = unsafe { (self.table().get_int_field)(self.raw, object, field) };
        self.check().then_some(value)
    }

    pub fn float_field(&self, object: JObject, field: JFieldId) -> Option<f32> {
        let value = unsafe { (self.table().get_float_field)(self.raw, object, field) };
        self.check().then_some(value)
    }

    pub fn static_object_field(&self, class: JClass, field: JFieldId) -> Option<JObject> {
        let value = unsafe { (self.table().get_static_object_field)(self.raw, class, field) };
        (self.check() && !value.is_null()).then_some(value)
    }

    pub fn static_int_field(&self, class: JClass, field: JFieldId) -> Option<i32> {
        let value = unsafe { (self.table().get_static_int_field)(self.raw, class, field) };
        self.check().then_some(value)
    }

    /// A Java string from a Rust one — a local reference.
    pub fn string(&self, text: &str) -> Option<JString> {
        let text = CString::new(text.replace('\0', "\u{FFFD}")).ok()?;
        let string = unsafe { (self.table().new_string_utf)(self.raw, text.as_ptr()) };
        (self.check() && !string.is_null()).then_some(string)
    }

    /// A Rust string from a Java one.
    pub fn to_string(&self, string: JString) -> Option<String> {
        if string.is_null() {
            return None;
        }
        let chars = unsafe { (self.table().get_string_utf_chars)(self.raw, string, null_mut()) };
        if !self.check() || chars.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(chars) }.to_string_lossy().into_owned();
        unsafe { (self.table().release_string_utf_chars)(self.raw, string, chars) };
        Some(text)
    }

    /// A global reference to `object`, which outlives every frame.
    pub fn global(&self, object: JObject) -> Option<JObject> {
        let global = unsafe { (self.table().new_global_ref)(self.raw, object) };
        (!global.is_null()).then_some(global)
    }

    pub fn delete_global(&self, object: JObject) {
        if !object.is_null() {
            unsafe { (self.table().delete_global_ref)(self.raw, object) };
        }
    }

    pub fn delete_local(&self, object: JObject) {
        if !object.is_null() {
            unsafe { (self.table().delete_local_ref)(self.raw, object) };
        }
    }

    pub fn array_length(&self, array: JObject) -> Option<i32> {
        let length = unsafe { (self.table().get_array_length)(self.raw, array) };
        self.check().then_some(length)
    }

    pub fn new_byte_array(&self, bytes: &[u8]) -> Option<JObject> {
        let array = unsafe { (self.table().new_byte_array)(self.raw, bytes.len() as JInt) };
        if !self.check() || array.is_null() {
            return None;
        }
        unsafe {
            (self.table().set_byte_array_region)(self.raw, array, 0, bytes.len() as JInt, bytes.as_ptr().cast())
        };
        self.check().then_some(array)
    }

    pub fn float_array_region(&self, array: JObject, out: &mut [f32]) -> bool {
        unsafe {
            (self.table().get_float_array_region)(self.raw, array, 0, out.len() as JInt, out.as_mut_ptr())
        };
        self.check()
    }

    pub fn int_array_region(&self, array: JObject, out: &mut [i32]) -> bool {
        unsafe { (self.table().get_int_array_region)(self.raw, array, 0, out.len() as JInt, out.as_mut_ptr()) };
        self.check()
    }
}

/// A local reference frame: every reference made while it stands dies
/// when it drops.
pub struct Frame {
    env: Env,
}

impl Frame {
    pub fn new(env: Env, capacity: i32) -> Option<Frame> {
        let pushed = unsafe { (env.table().push_local_frame)(env.raw, capacity) };
        if pushed != 0 {
            let _ = env.check();
            return None;
        }
        Some(Frame { env })
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe { (self.env.table().pop_local_frame)(self.env.raw, null_mut()) };
    }
}

pub fn object(value: JObject) -> JValue {
    JValue { l: value }
}

pub fn int(value: i32) -> JValue {
    JValue { i: value }
}

pub fn float(value: f32) -> JValue {
    JValue { f: value }
}

pub fn boolean(value: bool) -> JValue {
    JValue { z: value as u8 }
}

// MARK: - What the shell asks Java for

/// Four insets in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// What the window's insets say: the system bars and the cutout, the
/// keyboard's inset and whether it shows. `None` until the decor view
/// is attached — the caller keeps the last good answer.
pub struct WindowInsets {
    pub bars: Insets,
    pub ime: Insets,
    pub ime_visible: bool,
}

fn decor_view(env: Env) -> Option<JObject> {
    let activity = env.activity();
    let activity_class = env.class(c"android/app/Activity")?;
    let get_window = env.method(activity_class, c"getWindow", c"()Landroid/view/Window;")?;
    let window = env.call_object(activity, get_window, &[])?;
    let window_class = env.class(c"android/view/Window")?;
    let get_decor = env.method(window_class, c"getDecorView", c"()Landroid/view/View;")?;
    env.call_object(window, get_decor, &[])
}

fn insets_of(env: Env, insets: JObject, kind: i32) -> Option<Insets> {
    let insets_class = env.class(c"android/view/WindowInsets")?;
    let get_insets = env.method(insets_class, c"getInsets", c"(I)Landroid/graphics/Insets;")?;
    let value = env.call_object(insets, get_insets, &[int(kind)])?;
    let value_class = env.class(c"android/graphics/Insets")?;
    let read = |name: &CStr| env.int_field(value, env.field(value_class, name, c"I")?);
    Some(Insets { left: read(c"left")?, top: read(c"top")?, right: read(c"right")?, bottom: read(c"bottom")? })
}

/// The window's insets, through the decor view's own — the one road
/// that reports the bars under an edge-to-edge window (the content
/// rect reports zeros there).
pub fn window_insets() -> Option<WindowInsets> {
    let env = Env::current()?;
    let _frame = Frame::new(env, 24)?;
    let decor = decor_view(env)?;
    let view_class = env.class(c"android/view/View")?;
    let get_root = env.method(view_class, c"getRootWindowInsets", c"()Landroid/view/WindowInsets;")?;
    let insets = env.call_object(decor, get_root, &[])?;
    let type_class = env.class(c"android/view/WindowInsets$Type")?;
    let kind = |name: &CStr| env.call_static_int(type_class, env.static_method(type_class, name, c"()I")?, &[]);
    let bars_kind = kind(c"systemBars")? | kind(c"displayCutout")?;
    let ime_kind = kind(c"ime")?;
    let insets_class = env.class(c"android/view/WindowInsets")?;
    let is_visible = env.method(insets_class, c"isVisible", c"(I)Z")?;
    Some(WindowInsets {
        bars: insets_of(env, insets, bars_kind)?,
        ime: insets_of(env, insets, ime_kind)?,
        ime_visible: env.call_bool(insets, is_visible, &[int(ime_kind)])?,
    })
}

/// Lays the window out under the bars: the surface is the whole
/// screen, and the insets say where the bars stand.
pub fn edge_to_edge() {
    let Some(env) = Env::current() else { return };
    let Some(_frame) = Frame::new(env, 8) else { return };
    let activity = env.activity();
    let Some(activity_class) = env.class(c"android/app/Activity") else { return };
    let Some(get_window) = env.method(activity_class, c"getWindow", c"()Landroid/view/Window;") else {
        return;
    };
    let Some(window) = env.call_object(activity, get_window, &[]) else { return };
    let Some(window_class) = env.class(c"android/view/Window") else { return };
    if let Some(fit) = env.method(window_class, c"setDecorFitsSystemWindows", c"(Z)V") {
        env.call_void(window, fit, &[boolean(false)]);
    }
    // the bars stay in the picture: translucent over the scene
    for name in [c"setStatusBarColor", c"setNavigationBarColor"] {
        if let Some(paint) = env.method(window_class, name, c"(I)V") {
            env.call_void(window, paint, &[int(0)]);
        }
    }
}

/// The system's animation scale is zero — the person asked for no
/// motion.
pub fn reduce_motion() -> bool {
    let Some(env) = Env::current() else { return false };
    let Some(_frame) = Frame::new(env, 8) else { return false };
    let read = || -> Option<f32> {
        let activity = env.activity();
        let context_class = env.class(c"android/content/Context")?;
        let get_resolver =
            env.method(context_class, c"getContentResolver", c"()Landroid/content/ContentResolver;")?;
        let resolver = env.call_object(activity, get_resolver, &[])?;
        let settings = env.class(c"android/provider/Settings$Global")?;
        let get_float = env.static_method(
            settings,
            c"getFloat",
            c"(Landroid/content/ContentResolver;Ljava/lang/String;F)F",
        )?;
        let name = env.string("animator_duration_scale")?;
        env.call_static_float(settings, get_float, &[object(resolver), object(name), float(1.0)])
    };
    read().is_some_and(|scale| scale == 0.0)
}

fn clipboard_manager(env: Env) -> Option<JObject> {
    let activity = env.activity();
    let context_class = env.class(c"android/content/Context")?;
    let get_service =
        env.method(context_class, c"getSystemService", c"(Ljava/lang/String;)Ljava/lang/Object;")?;
    let name = env.string("clipboard")?;
    env.call_object(activity, get_service, &[object(name)])
}

/// Puts `text` on the system clipboard.
pub fn clipboard_write(text: &str) -> bool {
    let Some(env) = Env::current() else { return false };
    let Some(_frame) = Frame::new(env, 8) else { return false };
    let write = || -> Option<()> {
        let manager = clipboard_manager(env)?;
        let data_class = env.class(c"android/content/ClipData")?;
        let plain = env.static_method(
            data_class,
            c"newPlainText",
            c"(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
        )?;
        let label = env.string("bunny_ui")?;
        let content = env.string(text)?;
        let clip = env.call_static_object(data_class, plain, &[object(label), object(content)])?;
        let manager_class = env.class(c"android/content/ClipboardManager")?;
        let set = env.method(manager_class, c"setPrimaryClip", c"(Landroid/content/ClipData;)V")?;
        env.call_void(manager, set, &[object(clip)]).then_some(())
    };
    write().is_some()
}

/// The system clipboard's text, if any.
pub fn clipboard_read() -> Option<String> {
    let env = Env::current()?;
    let _frame = Frame::new(env, 8)?;
    let manager = clipboard_manager(env)?;
    let manager_class = env.class(c"android/content/ClipboardManager")?;
    let get = env.method(manager_class, c"getPrimaryClip", c"()Landroid/content/ClipData;")?;
    let clip = env.call_object(manager, get, &[])?;
    let data_class = env.class(c"android/content/ClipData")?;
    let item_at = env.method(data_class, c"getItemAt", c"(I)Landroid/content/ClipData$Item;")?;
    let item = env.call_object(clip, item_at, &[int(0)])?;
    let item_class = env.class(c"android/content/ClipData$Item")?;
    let coerce = env.method(item_class, c"coerceToText", c"(Landroid/content/Context;)Ljava/lang/CharSequence;")?;
    let text = env.call_object(item, coerce, &[object(env.activity())])?;
    let object_class = env.class(c"java/lang/Object")?;
    let to_string = env.method(object_class, c"toString", c"()Ljava/lang/String;")?;
    let string = env.call_object(text, to_string, &[])?;
    env.to_string(string)
}
