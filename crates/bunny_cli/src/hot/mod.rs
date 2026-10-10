//! Hot reload in `bunny run`: a save swaps the code of the running app,
//! and the app keeps its state — on this computer, in the iOS Simulator
//! and on Android.
//!
//! A session builds the app with the `hot` feature of `bunny-ui`: the
//! framework links as one shared library (`bunny-ui-dylib`), and the
//! app's library builds as a shared library of its own — the first
//! *generation*. On the desktop and in the Simulator, the binary loads
//! that generation before its first frame; on Android, the APK's library
//! is the first generation. After each save, `bunny run` builds the
//! library again, which takes a moment because the framework is built
//! already, and hands the running app the new generation (`bunny-ui-hot`
//! is the other end): the path, where the app can read this computer's
//! files, or the bytes, on Android.
//!
//! The app calls back on a socket this side listens on: the loopback,
//! which the Simulator shares with the computer, or, on Android, an
//! abstract socket that `adb reverse` carries here.
//!
//! An edit that reaches a type gets a new salt ([`classify`]). A change
//! the library cannot carry restarts the app: `Cargo.toml`, `build.rs`,
//! the binary's own `src/main.rs`, or a dependency that had to be built
//! again.

pub mod classify;
pub mod watch;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::cargo::{self, Target};
use crate::devices::{Device, Kind, Platform};
use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::platform::{self, ChildSession, Options, Session, android, ios};
use crate::process;
use crate::project::Project;
use crate::term;
use crate::toolchains;
use classify::Reach;
use watch::Watch;

/// How long the app may take to swap to a generation.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a started app may take to call back.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The abstract socket an Android app calls back on (`bunny-ui-hot`'s).
const ANDROID_SOCKET: &str = "bunny-hot";

/// Where the hot app runs.
enum Place {
    /// This computer.
    Desktop,
    /// A booted iOS Simulator.
    Simulator(Device),
    /// An Android device or emulator.
    Android { toolchain: android::Toolchain, serial: String },
}

/// One `bunny run` with hot reload: how it builds the app, and the code
/// the running app has.
pub struct Hot {
    project: Project,
    options: Options,
    place: Place,
    /// The triple, named outright even for this computer: without it,
    /// cargo links the binary against the framework twice — statically
    /// and through the shared library — and refuses.
    target: String,
    /// The environment and the compiler flags of every build for the
    /// place: a generation built with others builds the framework again.
    env: Vec<(String, OsString)>,
    rustc_args: Vec<String>,
    /// The toolchain's standard library for the target: a Rust shared
    /// library links it, and the app with it.
    std_libs: PathBuf,
    /// `bunny-ui/hot`, by the name the app's manifest gives bunny-ui.
    feature: String,
    /// Where the generations go, each under a name of its own: a library
    /// loaded once is never loaded again from the same file.
    generations: PathBuf,
    next: Cell<u32>,
    salt: u32,
    /// The library's sources as the running generation was built from
    /// them.
    sources: BTreeMap<PathBuf, String>,
    watch: Watch,
}

/// What the saves since the last look ask of the running app.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    /// New code for the library: a hot reload carries it.
    Code,
    /// Something only a new start carries — and what.
    Restart(String),
}

/// A hot reload that landed.
pub struct Reloaded {
    /// From the start of the build to the app's answer.
    pub took: Duration,
    /// The edit reached a type: the state of the app's own types starts
    /// over.
    pub new_types: bool,
}

/// Why a hot reload did not land.
pub enum Missed {
    /// The build failed, and cargo said why. The app keeps its code.
    Build,
    /// Only a restart carries this change — and what it is.
    Restart(String),
    /// The app refused the new code, or did not answer.
    Load(String),
}

impl Hot {
    /// Hot reload for the app on `device` — or why it has none, to follow
    /// "no hot reload: ".
    pub fn prepare(project: &Project, options: &Options, device: &Device) -> std::result::Result<Hot, String> {
        if !project.has_lib {
            return Err(String::from("it builds the app's code from src/lib.rs, and this app has none"));
        }
        let rust = toolchains::rust::detect().ok_or_else(|| String::from("rustc did not answer"))?;
        let (place, target, env, rustc_args) = match (device.platform, device.kind) {
            (_, Kind::Desktop) => (Place::Desktop, rust.host.clone(), project.build_env(), Vec::new()),
            (Platform::Ios, Kind::Simulator) => {
                let env = ios::build_env(project).map_err(|error| error.message)?;
                (Place::Simulator(device.clone()), ios::simulator_target().to_string(), env, Vec::new())
            }
            (Platform::Android, _) => {
                let toolchain = android::Toolchain::find().map_err(|error| error.message)?;
                let (_, triple) = android::abi(&toolchain, &device.id).map_err(|error| error.message)?;
                let env = android::cargo_env(project, &toolchain, triple);
                (Place::Android { toolchain, serial: device.id.clone() }, triple.to_string(), env, android::page_args())
            }
            _ => return Err(format!("{} does not load new code yet", device.name)),
        };
        let feature = hot_feature(project, &target)?;
        let generations = project.out_dir("hot", &target, false).join("generations");
        platform::fresh_dir(&generations).map_err(|error| error.message)?;
        Ok(Hot {
            std_libs: rust.sysroot.join("lib/rustlib").join(&target).join("lib"),
            place,
            target,
            env,
            rustc_args,
            feature,
            generations,
            next: Cell::new(1),
            salt: 1,
            sources: BTreeMap::new(),
            watch: Watch::new(&project.dir),
            project: project.clone(),
            options: options.clone(),
        })
    }

    /// Builds the app with its first generation, and starts it.
    pub fn start(&mut self) -> Result<Box<dyn Session>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| Error::new(format!("no port for the app to call back on: {error}")))?;
        let port = listener.local_addr().map(|address| address.port()).unwrap_or_default().to_string();
        // what the build reads, looked at before it: a save during the
        // build is a change the next look finds
        self.watch = Watch::new(&self.project.dir);
        let sources = read_sources(&self.project.dir);
        let feature = std::slice::from_ref(&self.feature);
        let (app, bytes) = match &self.place {
            Place::Desktop => {
                let (executable, built) = platform::desktop::build_with(&self.project, &self.options, Some(&self.target), feature)?;
                let (generation, _) = self.build_generation(self.salt)?;
                let env = [("BUNNY_HOT_PORT", OsString::from(&port)), ("BUNNY_HOT_GENERATION", generation.into_os_string())];
                (launch(&executable, &env, &[self.std_libs.clone(), deps(&built)], &self.options)?, false)
            }
            Place::Simulator(device) => {
                let app = ios::build_simulator_with(&self.project, &self.options, feature)?;
                let (generation, _) = self.build_generation(self.salt)?;
                let libs = std::env::join_paths([self.std_libs.clone(), deps(&app.binary)]).unwrap_or_default();
                let env = [
                    (String::from("BUNNY_HOT_PORT"), OsString::from(&port)),
                    (String::from("BUNNY_HOT_GENERATION"), generation.into_os_string()),
                    (String::from("DYLD_LIBRARY_PATH"), libs),
                ];
                let session = ios::launch_simulator_with(device, &app, &self.options, &env)?;
                (session.ok_or_else(|| Error::new("the app started detached"))?, false)
            }
            Place::Android { toolchain, serial } => {
                let hot = android::HotBuild { feature: &self.feature, salt: self.salt, std_libs: &self.std_libs };
                let apk = android::build_with(&self.project, &self.options, toolchain, serial, Some(hot))?;
                let socket = format!("localabstract:{ANDROID_SOCKET}");
                let reversed = toolchain.adb(serial, &["reverse", &socket, &format!("tcp:{port}")])?;
                if !reversed.ok() {
                    println!("{}", term::warn(&format!("adb reverse failed: {}", reversed.stderr.trim())));
                }
                let session = android::launch(toolchain, serial, &apk, &self.options)?;
                (session.ok_or_else(|| Error::new("the app started detached"))?, true)
            }
        };
        self.sources = sources;
        Ok(call_back(listener, app, bytes))
    }

    /// What the saves since the last look ask for — `None` when nothing
    /// was saved.
    pub fn poll(&mut self) -> Option<Change> {
        let changed = self.watch.changed();
        if changed.is_empty() {
            return None;
        }
        Some(change(&self.project.dir, &changed))
    }

    /// Builds the library as it is now and loads it into the running app.
    pub fn reload(&mut self, app: &mut dyn Session) -> std::result::Result<Reloaded, Missed> {
        let began = Instant::now();
        let sources = read_sources(&self.project.dir);
        let new_types = reaches_types(&self.sources, &sources);
        let salt = if new_types { self.salt + 1 } else { self.salt };
        let (generation, rebuilt) = self.build_generation(salt).map_err(|_| Missed::Build)?;
        if let Some(dependency) = rebuilt.first() {
            return Err(Missed::Restart(format!("{dependency} was built again")));
        }
        match app.load(&generation) {
            None => Err(Missed::Load(String::from("this app does not take new code"))),
            Some(Err(why)) => Err(Missed::Load(why)),
            Some(Ok(_)) => {
                self.salt = salt;
                self.sources = sources;
                Ok(Reloaded { took: began.elapsed(), new_types })
            }
        }
    }

    /// The library as a shared library of its own, salted, copied under a
    /// new name — and the dependencies the build compiled again.
    fn build_generation(&self, salt: u32) -> Result<(PathBuf, Vec<String>)> {
        let mut features = self.options.features.clone();
        features.push(self.feature.clone());
        let mut rustc_args = self.rustc_args.clone();
        rustc_args.extend([String::from("-C"), format!("metadata=bunny-salt-{salt}")]);
        let built = cargo::build(&cargo::Build {
            manifest: self.project.manifest.clone(),
            package: self.project.package.clone(),
            what: Target::CdylibLib,
            release: false,
            profile: None,
            target: Some(self.target.clone()),
            features,
            env: self.env.clone(),
            rustc_args,
            quiet: true,
        })?;
        let extension = built.artifact.extension().map(|ext| ext.to_string_lossy().into_owned()).unwrap_or_default();
        let number = self.next.replace(self.next.get() + 1);
        let generation = self.generations.join(format!("generation-{number}.{extension}"));
        std::fs::copy(&built.artifact, &generation)
            .map_err(|error| Error::new(format!("{} → {}: {error}", built.artifact.display(), generation.display())))?;
        Ok((generation, built.rebuilt))
    }
}

/// cargo's `deps` folder next to an artifact: where the framework's
/// shared library is.
fn deps(artifact: &Path) -> PathBuf {
    artifact.parent().map(|dir| dir.join("deps")).unwrap_or_default()
}

/// `bunny-ui/hot`, by the name the app gives bunny-ui — when the
/// bunny-ui it resolves to has hot reload.
fn hot_feature(project: &Project, target: &str) -> std::result::Result<String, String> {
    let manifest = project.manifest.to_string_lossy().into_owned();
    let args = ["metadata", "--format-version", "1", "--filter-platform", target, "--manifest-path", manifest.as_str()];
    let out = process::run_in("cargo", &args, Some(&project.dir), &[], Duration::from_secs(120))
        .map_err(|error| format!("cargo: {error}"))?;
    if !out.ok() {
        return Err(format!("cargo could not read the app's dependencies: {}", out.stderr.trim()));
    }
    let metadata = json::parse(&out.stdout).map_err(|error| format!("cargo metadata: {error}"))?;
    let packages = metadata.get("packages").map(Value::as_array).unwrap_or_default();
    let app = packages
        .iter()
        .find(|package| package.str_at(&["manifest_path"]).map(Path::new) == Some(project.manifest.as_path()))
        .ok_or_else(|| String::from("cargo did not list the app"))?;
    let dependency = app
        .get("dependencies")
        .map(Value::as_array)
        .unwrap_or_default()
        .iter()
        .find(|dependency| dependency.str_at(&["name"]) == Some("bunny-ui") && dependency.str_at(&["kind"]).is_none())
        .ok_or_else(|| String::from("the app does not depend on bunny-ui"))?;
    let has_hot = packages.iter().filter(|package| package.str_at(&["name"]) == Some("bunny-ui")).any(|package| {
        package.get("features").is_some_and(|features| features.members().iter().any(|(name, _)| name == "hot"))
    });
    if !has_hot {
        return Err(String::from("the app's bunny-ui does not have it yet (update it: cargo update -p bunny-ui)"));
    }
    Ok(format!("{}/hot", dependency.str_at(&["rename"]).unwrap_or("bunny-ui")))
}

/// What a save of `changed` asks for: a file the library does not build
/// from restarts the app.
fn change(dir: &Path, changed: &[PathBuf]) -> Change {
    for path in changed {
        let relative = path.strip_prefix(dir).unwrap_or(path);
        let binary = relative == Path::new("src/main.rs") || relative.starts_with("src/bin");
        if binary || relative == Path::new("Cargo.toml") || relative == Path::new("build.rs") {
            return Change::Restart(format!("{} changed", relative.display()));
        }
    }
    Change::Code
}

/// Whether the edit from `old` to `new` reaches a type: a file that came
/// or went (a module, an item set), or an edit outside function bodies.
fn reaches_types(old: &BTreeMap<PathBuf, String>, new: &BTreeMap<PathBuf, String>) -> bool {
    old.keys().ne(new.keys())
        || old.iter().any(|(path, text)| new.get(path).is_some_and(|now| classify::classify(text, now) == Reach::Types))
}

/// The library's sources: every `.rs` under `src/`, the binary's own
/// files left out.
fn read_sources(dir: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, sources: &mut BTreeMap<PathBuf, String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, sources);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                sources.insert(path, text);
            }
        }
    }
    let mut sources = BTreeMap::new();
    walk(&dir.join("src"), &mut sources);
    sources.retain(|path, _| !matches!(change(dir, std::slice::from_ref(path)), Change::Restart(_)));
    sources
}

/// Starts the binary on this computer, its output in this terminal.
fn launch(executable: &Path, env: &[(&str, OsString)], libs: &[PathBuf], options: &Options) -> Result<Box<dyn Session>> {
    let mut command = Command::new(executable);
    command.args(&options.args).stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    for (key, value) in env {
        command.env(key, value);
    }
    // the shared libraries the binary links: the framework, and the
    // standard library of the toolchain that built it
    let variable = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths = libs.to_vec();
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env(variable, joined);
    }
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        command.env("RUST_BACKTRACE", "1");
    }
    let child =
        command.spawn().map_err(|error| Error::new(format!("{} did not start: {error}", executable.display())))?;
    Ok(Box::new(ChildSession { child, on_stop: None }))
}

/// Waits for the started app to call back, and answers the session that
/// talks to it. An app that never calls runs on, without hot reload.
fn call_back(listener: TcpListener, mut app: Box<dyn Session>, bytes: bool) -> Box<dyn Session> {
    let off = |why: &str| println!("{}", term::warn(&format!("hot reload is off: {why} — r restarts the app")));
    if listener.set_nonblocking(true).is_err() {
        off("the port did not open");
        return app;
    }
    let until = Instant::now() + CALL_TIMEOUT;
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if app.wait(Duration::from_millis(50)).is_some() {
                    return app;
                }
                if Instant::now() >= until {
                    off("the app did not call back");
                    return app;
                }
            }
            Err(error) => {
                off(&format!("the app's call did not land: {error}"));
                return app;
            }
        }
    };
    match HotApp::new(app, stream, bytes) {
        Ok(hot) => Box::new(hot),
        Err((app, why)) => {
            off(&why);
            app
        }
    }
}

/// The running app, with a line to it.
struct HotApp {
    app: Box<dyn Session>,
    line: TcpStream,
    answers: Receiver<String>,
    /// The app reads no file of this computer: a generation goes as
    /// bytes, not as a path.
    bytes: bool,
}

impl HotApp {
    /// The app on the line, once it said how it started.
    fn new(app: Box<dyn Session>, line: TcpStream, bytes: bool) -> std::result::Result<HotApp, (Box<dyn Session>, String)> {
        let reading = match line.set_nonblocking(false).and_then(|()| line.try_clone()) {
            Ok(reading) => reading,
            Err(error) => return Err((app, format!("the line to the app broke: {error}"))),
        };
        let (sender, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for answer in BufReader::new(reading).lines().map_while(std::result::Result::ok) {
                if sender.send(answer).is_err() {
                    return;
                }
            }
        });
        let mut hot = HotApp { app, line, answers, bytes };
        match hot.answer(LOAD_TIMEOUT) {
            Ok(_) => Ok(hot),
            Err(why) => Err((hot.app, why)),
        }
    }

    /// The app's answer to the last order: how long its swap took.
    fn answer(&mut self, timeout: Duration) -> std::result::Result<Duration, String> {
        let until = Instant::now() + timeout;
        loop {
            match self.answers.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return parse_answer(&answer),
                Err(RecvTimeoutError::Disconnected) => return Err(String::from("the app stopped")),
                Err(RecvTimeoutError::Timeout) => {
                    if self.app.wait(Duration::ZERO).is_some() {
                        return Err(String::from("the app stopped"));
                    }
                    if Instant::now() >= until {
                        return Err(format!("the app did not answer in {} seconds", timeout.as_secs()));
                    }
                }
            }
        }
    }

    fn send(&mut self, generation: &Path) -> std::io::Result<()> {
        if self.bytes {
            let bytes = std::fs::read(generation)?;
            writeln!(self.line, "take {}", bytes.len())?;
            self.line.write_all(&bytes)?;
        } else {
            writeln!(self.line, "load {}", generation.display())?;
        }
        self.line.flush()
    }
}

/// `loaded <n> <ms>`, `ready`, or `failed <why>`.
fn parse_answer(answer: &str) -> std::result::Result<Duration, String> {
    match answer.split_once(' ').unwrap_or((answer, "")) {
        ("loaded", rest) => {
            let millis = rest.split_whitespace().nth(1).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            Ok(Duration::from_millis(millis))
        }
        ("ready", _) => Ok(Duration::ZERO),
        ("failed", why) => Err(why.to_string()),
        _ => Err(format!("the app answered {answer:?}")),
    }
}

impl Session for HotApp {
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>> {
        self.app.wait(timeout)
    }

    fn stop(&mut self) {
        let _ = self.line.shutdown(std::net::Shutdown::Both);
        self.app.stop();
    }

    fn load(&mut self, generation: &Path) -> Option<std::result::Result<Duration, String>> {
        // an answer that came too late for its order is not this one's
        while self.answers.try_recv().is_ok() {}
        if let Err(error) = self.send(generation) {
            return Some(Err(format!("the line to the app broke: {error}")));
        }
        Some(self.answer(LOAD_TIMEOUT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_outside_the_library_restarts_the_app() {
        let dir = Path::new("/app");
        let at = |paths: &[&str]| change(dir, &paths.iter().map(|path| dir.join(path)).collect::<Vec<_>>());
        assert_eq!(at(&["src/lib.rs", "src/ui/row.rs"]), Change::Code);
        assert_eq!(at(&["src/lib.rs", "Cargo.toml"]), Change::Restart(String::from("Cargo.toml changed")));
        assert_eq!(at(&["src/main.rs"]), Change::Restart(String::from("src/main.rs changed")));
        assert_eq!(at(&["src/bin/tool.rs"]), Change::Restart(String::from("src/bin/tool.rs changed")));
        assert_eq!(at(&["build.rs"]), Change::Restart(String::from("build.rs changed")));
    }

    #[test]
    fn a_file_that_comes_or_goes_reaches_the_types() {
        let one: BTreeMap<PathBuf, String> = [(PathBuf::from("src/lib.rs"), String::from("fn a() { 1 }"))].into();
        let body = [(PathBuf::from("src/lib.rs"), String::from("fn a() { 2 }"))].into();
        let mut two = one.clone();
        two.insert(PathBuf::from("src/row.rs"), String::new());
        assert!(!reaches_types(&one, &body));
        assert!(reaches_types(&one, &two));
        assert!(reaches_types(&two, &one));
    }

    #[test]
    fn the_answers_of_the_app_are_read() {
        assert_eq!(parse_answer("loaded 3 164"), Ok(Duration::from_millis(164)));
        assert_eq!(parse_answer("ready"), Ok(Duration::ZERO));
        assert_eq!(parse_answer("failed it has no entry"), Err(String::from("it has no entry")));
        assert!(parse_answer("hello").is_err());
    }
}
