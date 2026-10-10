//! Hot reload — the part of it that runs inside the app.
//!
//! `bunny run` builds the framework once per session as one shared
//! library, `bunny-ui-dylib`, and the app links it. After each save it
//! builds only the app's library again, as a shared library of its own:
//! a *generation*. This crate loads each generation next to the ones
//! before it and moves the views to its code. The state stays where it
//! was: it lives in the framework, and every generation links the one
//! copy of the framework the app runs.
//!
//! An old generation is never unloaded. The closures, the tasks and the
//! tables its code made point into it, and they keep running until the
//! views that hold them are built again.
//!
//! The app calls `bunny run` back on a socket: on the computer's
//! loopback at `BUNNY_HOT_PORT` (the desktop, the iOS Simulator), or on
//! Android at the abstract socket `bunny-hot`, which `adb reverse`
//! carries to the computer. The two talk in lines of text. `bunny run`
//! sends orders:
//!
//! - `load <path>` — load the generation at `path`, a file the app can
//!   read where it is;
//! - `take <size>` and then `size` bytes — a generation for an app that
//!   cannot read the computer's files: it is written to the app's own
//!   folder first, and loaded from there.
//!
//! The app answers each order, and says first how it started:
//!
//! - `loaded <n> <ms>` — generation `n` runs; loading it took `ms`
//!   milliseconds;
//! - `failed <why>` — the app keeps the code it had;
//! - `ready` — the first answer of an app that started on the code it
//!   was built with.
//!
//! An application does not use this crate itself: `bunny_ui::app!` does,
//! in a build with the `hot` feature of `bunny-ui`, which `bunny run`
//! turns on.

#![deny(unsafe_op_in_unsafe_fn)]

use std::cell::RefCell;
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Instant;

use bunny_ui::view::{Component, Many, NodeList, View};
use motor::state::Context;

/// The shape of [`Entry`]. The app refuses a generation built for
/// another shape, and reads none of it but this number.
pub const ABI: u32 = 1;

/// The symbol every generation exports: a function, in the Rust ABI,
/// that answers the generation's [`Entry`].
pub const ENTRY_SYMBOL: &CStr = c"bunny_hot_entry_v1";

/// The abstract socket an Android app calls `bunny run` back on.
pub const ANDROID_SOCKET: &str = "bunny-hot";

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

/// How a [`Root`] draws: its view's own render, behind the erasure.
type Render = dyn Fn(&Context, &mut NodeList);

/// The app's first view, whatever its shape — one view or several.
#[derive(Clone)]
pub struct Root(Rc<Render>);

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
/// root view to open the window with. `scratch` is a folder the app may
/// write, for the generations `bunny run` sends as bytes; without one,
/// the system's temporary folder.
///
/// When `bunny run` started the app, `BUNNY_HOT_GENERATION` may name the
/// first generation. It loads now, so the first frame already runs its
/// code. Then the app calls `bunny run` back and waits for the next
/// ones. An app nobody calls back — the binary started by hand — shows
/// `built_in`, the view it was built with.
///
/// Only the first call starts anything: an Android process may create
/// one activity after another, and they all show the generation that
/// runs.
pub fn start(built_in: impl Fn() -> Root + 'static, scratch: Option<PathBuf>) -> HotRoot {
    let root = HotRoot { built_in: Rc::new(built_in) };
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return root;
    }
    let first = std::env::var_os("BUNNY_HOT_GENERATION").map(|first| swap(Path::new(&first)));
    if let Some(next) = std::env::var_os("BUNNY_HOT_CHECK") {
        check(&root, first, Path::new(&next));
    }
    let Some(stream) = transport::connect() else {
        return root;
    };
    end_on_panic();
    listen(stream, first, scratch.unwrap_or_else(std::env::temp_dir));
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
/// each one to the UI thread, where the views are; the UI thread swaps
/// and answers. A send wakes the UI thread through the framework's task
/// channel.
fn listen(stream: transport::Stream, first: Option<Answer>, scratch: PathBuf) {
    let Ok(reading) = stream.try_clone() else { return };
    let mut writing = stream;
    let hello = first.map_or_else(|| String::from("ready"), |answer| answer.line());
    if writeln!(writing, "{hello}").and_then(|()| writing.flush()).is_err() {
        return;
    }
    let (sender, receiver) = motor::task::channel::<Result<PathBuf, String>>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reading);
        let mut taken = 0u32;
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
            let order = match line.trim().split_once(' ') {
                Some(("load", path)) => Ok(PathBuf::from(path.trim())),
                Some(("take", size)) => {
                    taken += 1;
                    take(&mut reader, size.trim(), &scratch, taken)
                }
                _ => Err(format!("the order {:?} is unknown", line.trim())),
            };
            line.clear();
            if sender.send(order).is_err() {
                return;
            }
        }
        // the line closed: `bunny run` is gone. A desktop app goes with
        // it, the way it went when `bunny run` was its parent; a phone's
        // app stays, on the code it has.
        if cfg!(not(any(target_os = "ios", target_os = "android"))) {
            std::process::exit(0);
        }
    });
    motor::task::spawn(async move {
        while let Some(order) = receiver.recv().await {
            let answer = match order {
                Ok(path) => swap(&path),
                Err(why) => Answer::Failed(why),
            };
            if writeln!(writing, "{}", answer.line()).and_then(|()| writing.flush()).is_err() {
                return;
            }
        }
    })
    .detach();
}

/// Reads a generation sent as bytes and writes it to the app's folder,
/// under a name of its own: a library loaded once is never loaded again
/// from the same file.
fn take(reader: &mut impl Read, size: &str, scratch: &Path, taken: u32) -> Result<PathBuf, String> {
    let size: u64 = size.parse().map_err(|_| format!("`take {size}` has no size"))?;
    let mut bytes = Vec::new();
    reader.take(size).read_to_end(&mut bytes).map_err(|error| format!("the generation did not arrive: {error}"))?;
    if bytes.len() as u64 != size {
        return Err(String::from("the generation arrived cut short"));
    }
    let folder = scratch.join("bunny-hot");
    std::fs::create_dir_all(&folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let path = folder.join(format!("generation-{}-{taken}.{}", std::process::id(), std::env::consts::DLL_EXTENSION));
    std::fs::write(&path, &bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// What the app answers an order.
enum Answer {
    Loaded { number: u32, millis: u128 },
    Failed(String),
}

impl Answer {
    fn line(&self) -> String {
        match self {
            Answer::Loaded { number, millis } => format!("loaded {number} {millis}"),
            Answer::Failed(why) => format!("failed {}", why.replace('\n', " ")),
        }
    }
}

/// Loads a generation and moves the views to it — or says why not, and
/// keeps the code the app has.
fn swap(path: &Path) -> Answer {
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
            Answer::Loaded { number, millis: began.elapsed().as_millis() }
        }
        Err(why) => Answer::Failed(why),
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

/// `BUNNY_HOT_CHECK`: the whole road without a window, for a machine
/// with no screen. The app draws its first view as text, loads the
/// generation the variable names, draws it again, prints both and ends:
/// 0 when both generations loaded.
fn check(root: &HotRoot, first: Option<Answer>, next: &Path) -> ! {
    let runtime = bunny_ui::prelude::Runtime::new();
    let mut ok = true;
    let mut draw = |answer: Answer| {
        println!("@@bunny-hot {}", answer.line());
        ok &= matches!(answer, Answer::Loaded { .. });
        println!("{}", runtime.render(root));
    };
    if let Some(first) = first {
        draw(first);
    }
    draw(swap(next));
    std::process::exit(if ok { 0 } else { 1 });
}

/// The line back to `bunny run`.
mod transport {
    use std::io::{Read, Write};

    pub enum Stream {
        Tcp(std::net::TcpStream),
        #[cfg(target_os = "android")]
        Local(std::os::unix::net::UnixStream),
    }

    /// `bunny run`, when it waits for this app.
    pub fn connect() -> Option<Stream> {
        if let Some(port) = std::env::var("BUNNY_HOT_PORT").ok().and_then(|port| port.parse::<u16>().ok()) {
            return std::net::TcpStream::connect(("127.0.0.1", port)).ok().map(Stream::Tcp);
        }
        android()
    }

    #[cfg(target_os = "android")]
    fn android() -> Option<Stream> {
        use std::os::android::net::SocketAddrExt;
        let address = std::os::unix::net::SocketAddr::from_abstract_name(super::ANDROID_SOCKET).ok()?;
        std::os::unix::net::UnixStream::connect_addr(&address).ok().map(Stream::Local)
    }

    #[cfg(not(target_os = "android"))]
    fn android() -> Option<Stream> {
        None
    }

    impl Stream {
        pub fn try_clone(&self) -> std::io::Result<Stream> {
            match self {
                Stream::Tcp(stream) => stream.try_clone().map(Stream::Tcp),
                #[cfg(target_os = "android")]
                Stream::Local(stream) => stream.try_clone().map(Stream::Local),
            }
        }
    }

    impl Read for Stream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self {
                Stream::Tcp(stream) => stream.read(buf),
                #[cfg(target_os = "android")]
                Stream::Local(stream) => stream.read(buf),
            }
        }
    }

    impl Write for Stream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            match self {
                Stream::Tcp(stream) => stream.write(buf),
                #[cfg(target_os = "android")]
                Stream::Local(stream) => stream.write(buf),
            }
        }

        fn flush(&mut self) -> std::io::Result<()> {
            match self {
                Stream::Tcp(stream) => stream.flush(),
                #[cfg(target_os = "android")]
                Stream::Local(stream) => stream.flush(),
            }
        }
    }
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
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const RTLD_LOCAL: c_int = 0;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
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

#[cfg(windows)]
mod native {
    use std::ffi::{c_char, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{ENTRY_SYMBOL, Entry};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
        fn GetLastError() -> u32;
    }

    /// Loads the library at `path` for as long as the app runs, and
    /// calls its entry.
    pub fn open(path: &Path) -> Result<&'static Entry, String> {
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        // SAFETY: `name` is a NUL-terminated wide path. Loading runs the
        // library's initializers, and a generation is the app's own
        // code, built by `bunny run` against the framework this app runs.
        let module = unsafe { LoadLibraryW(name.as_ptr()) };
        if module.is_null() {
            // SAFETY: no other call stands between the failure and this.
            let code = unsafe { GetLastError() };
            return Err(format!("{} did not load (Windows error {code})", path.display()));
        }
        // SAFETY: the module is loaded and the name is NUL-terminated.
        let symbol = unsafe { GetProcAddress(module, ENTRY_SYMBOL.as_ptr()) };
        if symbol.is_null() {
            return Err(format!(
                "{} has no `{}`: it is not a hot build of a bunny-ui app",
                path.display(),
                ENTRY_SYMBOL.to_string_lossy()
            ));
        }
        // SAFETY: as on the other platforms — the function `bunny_ui::app!`
        // writes, in the Rust ABI of the compiler that linked the
        // framework's library; the module is never freed.
        let entry = unsafe { std::mem::transmute::<*mut c_void, fn() -> &'static Entry>(symbol) };
        Ok(entry())
    }
}

#[cfg(not(any(unix, windows)))]
mod native {
    use std::path::Path;

    use super::Entry;

    pub fn open(path: &Path) -> Result<&'static Entry, String> {
        Err(format!("{}: this platform does not load new code", path.display()))
    }
}
