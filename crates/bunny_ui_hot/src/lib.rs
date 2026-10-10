//! Hot reload — the part of it that runs inside the app.
//!
//! `bunny run` builds the framework once per session as one shared
//! library, `bunny-ui-dylib`, and the app's binary links it. After each
//! save it builds only the app's library again, as a shared library of
//! its own: a *generation*. This crate loads each generation next to the
//! ones before it and moves the views to its code. The state stays where
//! it was: it lives in the framework, and every generation links the
//! one copy of the framework the app runs.
//!
//! An old generation is never unloaded. The closures, the tasks and the
//! tables its code made point into it, and they keep running until the
//! views that hold them are built again.
//!
//! `bunny run` talks to the app in lines of text. On the app's standard
//! input:
//!
//! - `load <path>` — load the generation at `path` and swap to it.
//!
//! On the app's standard output, each answer is one line that starts
//! with `@@bunny-hot`:
//!
//! - `@@bunny-hot loaded <n> <ms>` — generation `n` runs; loading it took
//!   `ms` milliseconds;
//! - `@@bunny-hot failed <why>` — the app keeps the code it had.
//!
//! An application does not use this crate itself: `bunny_ui::app!` does,
//! in a build with the `hot` feature of `bunny-ui`, which `bunny run`
//! turns on.

#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::ffi::CStr;
use std::io::{BufRead, Write};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicU8;
use std::time::Instant;

use bunny_ui::view::{Component, Many, NodeList, View};
use motor::state::Context;

/// The shape of [`Entry`]. The app refuses a generation built for
/// another shape, and reads none of it but this number.
pub const ABI: u32 = 1;

/// The symbol every generation exports: a function, in the Rust ABI,
/// that answers the generation's [`Entry`].
pub const ENTRY_SYMBOL: &CStr = c"bunny_hot_entry_v1";

/// What a generation gives the app: how to build its first view, and how
/// to tell which framework it was built against.
#[repr(C)]
pub struct Entry {
    /// [`ABI`], as the generation knew it. It comes first: of a
    /// generation of another shape, only this field is read.
    pub abi: u32,
    /// [`anchor`], as the generation calls it.
    pub anchor: fn() -> usize,
    /// Builds the app's first view with the generation's code.
    pub root: fn() -> Root,
}

impl Entry {
    /// The entry of a generation whose first view `root` builds.
    pub const fn new(root: fn() -> Root) -> Entry {
        Entry { abi: ABI, anchor, root }
    }
}

/// An address inside the framework the app runs. A generation must call
/// into that one copy: a generation that carries a framework of its own
/// answers another address, and its state would live apart from the
/// app's.
pub fn anchor() -> usize {
    // interior mutability: the static has an address of its own, never
    // one shared with an equal constant
    static ANCHOR: AtomicU8 = AtomicU8::new(0);
    &ANCHOR as *const AtomicU8 as usize
}

/// The app's first view, whatever its shape — one view or several.
#[derive(Clone)]
pub struct Root(Rc<dyn Fn(&Context, &mut NodeList)>);

impl Root {
    pub fn new<V: View>(view: V) -> Root {
        Root(Rc::new(move |context: &Context, out: &mut NodeList| view.render_into(context, out)))
    }
}

impl View for Root {
    type Arity = Many;

    fn render_into(&self, context: &Context, out: &mut NodeList) {
        (self.0)(context, out)
    }
}

/// The root view of a hot app: the first view of the generation that
/// runs — or, before one is loaded, the view the binary was built with.
#[derive(Clone)]
pub struct HotRoot {
    built_in: Rc<dyn Fn() -> Root>,
}

impl Component for HotRoot {
    fn body(self) -> impl View {
        // the first view is built HERE, inside the pass: the `State`s it
        // makes anchor under this view, and the root of the next
        // generation finds them again by their place
        match CURRENT.with(|current| current.borrow().as_ref().map(|generation| generation.entry.root)) {
            Some(root) => root(),
            None => (self.built_in)(),
        }
    }
}

struct Generation {
    number: u32,
    entry: &'static Entry,
}

thread_local! {
    /// The generation that runs.
    static CURRENT: RefCell<Option<Generation>> = const { RefCell::new(None) };
}

/// Starts hot reload, before the app's window opens, and answers the
/// root view to open the window with.
///
/// When `bunny run` starts the app, `BUNNY_HOT_GENERATION` names the
/// first generation. It loads now, so the first frame already runs its
/// code, and the app listens on its standard input for the next ones.
/// Without it — the binary started by hand — the app shows `built_in`,
/// the view it was built with.
pub fn start(built_in: impl Fn() -> Root + 'static) -> HotRoot {
    let root = HotRoot { built_in: Rc::new(built_in) };
    let Some(first) = std::env::var_os("BUNNY_HOT_GENERATION") else {
        return root;
    };
    end_on_panic();
    swap(Path::new(&first));
    listen();
    root
}

/// A panic on the UI thread ends the app at once, after its message.
/// The bodies run inside the platform's callbacks, where a panic cannot
/// unwind and aborts — and the system files a crash report for an edit
/// that `bunny run` meets with "save a fix". The exit says the same
/// thing without one. A panic on another thread is that thread's.
fn end_on_panic() {
    let report = std::panic::take_hook();
    let ui = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        report(info);
        if std::thread::current().id() == ui {
            std::process::exit(101);
        }
    }));
}

/// Reads the orders of `bunny run` on a thread of its own and carries
/// each one to the UI thread, where the views are. A send wakes the UI
/// thread through the framework's task channel.
fn listen() {
    let (sender, receiver) = motor::task::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                return;
            }
        }
        // the input closed: `bunny run` is gone, and the app goes with it
        std::process::exit(0);
    });
    motor::task::spawn(async move {
        while let Some(line) = receiver.recv().await {
            match line.trim().split_once(' ') {
                Some(("load", path)) => swap(Path::new(path.trim())),
                _ => say(format_args!("failed the order {line:?} is unknown")),
            }
        }
    })
    .detach();
}

/// Loads a generation and moves the views to it — or says why not, and
/// keeps the code the app has.
fn swap(path: &Path) {
    let began = Instant::now();
    match load(path) {
        Ok(entry) => {
            let number = CURRENT.with(|current| {
                let mut current = current.borrow_mut();
                let number = current.as_ref().map_or(1, |generation| generation.number + 1);
                *current = Some(Generation { number, entry });
                number
            });
            bunny_ui::code_changed();
            say(format_args!("loaded {number} {}", began.elapsed().as_millis()));
        }
        Err(why) => say(format_args!("failed {why}")),
    }
}

/// The entry of the generation at `path`, checked against this app.
fn load(path: &Path) -> Result<&'static Entry, String> {
    let entry = native::open(path)?;
    if entry.abi != ABI {
        return Err(format!(
            "{} was built for another version of bunny-ui (its entry is {}, the app's {ABI})",
            path.display(),
            entry.abi
        ));
    }
    if (entry.anchor)() != anchor() {
        return Err(format!(
            "{} carries a copy of the framework of its own: its state would live apart from the app's",
            path.display()
        ));
    }
    Ok(entry)
}

fn say(line: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "@@bunny-hot {line}");
    let _ = out.flush();
}

#[cfg(unix)]
mod native {
    use std::ffi::{CStr, CString, c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{ENTRY_SYMBOL, Entry};

    #[cfg_attr(target_os = "linux", link(name = "dl"))]
    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *const c_char;
    }

    const RTLD_NOW: c_int = 0x2;
    // a generation's own symbols stay out of the global namespace, so
    // the next generation cannot bind to them by accident
    #[cfg(target_os = "linux")]
    const RTLD_LOCAL: c_int = 0;
    #[cfg(not(target_os = "linux"))]
    const RTLD_LOCAL: c_int = 0x4;

    /// Loads the library at `path` for as long as the app runs, and
    /// calls its entry.
    pub fn open(path: &Path) -> Result<&'static Entry, String> {
        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("{} has a NUL byte in its path", path.display()))?;
        // SAFETY: `name` is a NUL-terminated path. Loading runs the
        // library's initializers, and a generation is the app's own
        // code, built by `bunny run` against the framework this app runs.
        let handle = unsafe { dlopen(name.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
        if handle.is_null() {
            return Err(last_error());
        }
        // SAFETY: the handle is open and the symbol's name is NUL-terminated.
        let symbol = unsafe { dlsym(handle, ENTRY_SYMBOL.as_ptr()) };
        if symbol.is_null() {
            return Err(format!(
                "{} has no `{}`: it is not a hot build of a bunny-ui app",
                path.display(),
                ENTRY_SYMBOL.to_string_lossy()
            ));
        }
        // SAFETY: the symbol is the function `bunny_ui::app!` writes in
        // every generation, `fn() -> &'static Entry` in the Rust ABI. The
        // generation links the framework's shared library, which only
        // the compiler that built it can link: the ABI is the same. The
        // library is never closed, so the entry lives as long as the app.
        let entry = unsafe { std::mem::transmute::<*mut c_void, fn() -> &'static Entry>(symbol) };
        Ok(entry())
    }

    fn last_error() -> String {
        // SAFETY: dlerror answers NULL, or a NUL-terminated message that
        // stays valid until the next dl call on this thread.
        let message = unsafe { dlerror() };
        if message.is_null() {
            return String::from("the library did not load");
        }
        // SAFETY: as above — not NULL, NUL-terminated, still valid.
        unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
    }
}

#[cfg(not(unix))]
mod native {
    use std::path::Path;

    use super::Entry;

    pub fn open(path: &Path) -> Result<&'static Entry, String> {
        Err(format!("{}: hot reload does not load new code on this platform yet", path.display()))
    }
}
