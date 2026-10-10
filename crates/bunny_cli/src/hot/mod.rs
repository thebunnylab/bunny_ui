//! Hot reload in `bunny run`, on macOS and Linux: a save swaps the code
//! of the running app, and the app keeps its state.
//!
//! A session builds the app twice to start. The binary links the
//! framework as one shared library (`bunny-ui-dylib`, brought in by the
//! `hot` feature of `bunny-ui`); the app's library builds as a shared
//! library of its own — the first *generation* — and the binary loads it
//! before its first frame. After each save, `bunny run` builds the
//! library again, which takes a moment because the framework is built
//! already, and tells the app on its standard input to load the new
//! generation (`bunny-ui-hot` is the other end).
//!
//! An edit that reaches a type gets a new salt ([`classify`]). A change
//! the library cannot carry restarts the app: `Cargo.toml`, `build.rs`,
//! the binary's own `src/main.rs`, or a dependency that had to be built
//! again.

pub mod classify;
pub mod watch;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::cargo::{self, Target};
use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::platform::{self, ChildSession, Options, Session};
use crate::process;
use crate::project::Project;
use crate::toolchains;
use classify::Reach;
use watch::Watch;

/// How long the app may take to swap to a generation.
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// One `bunny run` with hot reload: how it builds the app, and the code
/// the running app has.
pub struct Hot {
    project: Project,
    options: Options,
    /// The host's triple, named outright: without it, cargo links the
    /// binary against the framework twice — statically and through the
    /// shared library — and refuses.
    target: String,
    /// The toolchain's own libraries: the standard library that a Rust
    /// shared library links, and the app with it.
    std_libs: PathBuf,
    /// Where the build put the framework's shared library.
    libs: PathBuf,
    /// `bunny-ui/hot`, by the name the app's manifest gives bunny-ui.
    feature: String,
    /// Where the generations go, each under a name of its own: a library
    /// loaded once is never loaded again from the same file.
    generations: PathBuf,
    next: u32,
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
    /// Hot reload for the app on this computer — or why it has none, to
    /// follow "no hot reload: ".
    pub fn prepare(project: &Project, options: &Options) -> std::result::Result<Hot, String> {
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Err(String::from("it comes to this platform later"));
        }
        if !project.has_lib {
            return Err(String::from("it builds the app's code from src/lib.rs, and this app has none"));
        }
        let rust = toolchains::rust::detect().ok_or_else(|| String::from("rustc did not answer"))?;
        let feature = hot_feature(project, &rust.host)?;
        let generations = project.out_dir("hot", &rust.host, false).join("generations");
        platform::fresh_dir(&generations).map_err(|error| error.message)?;
        Ok(Hot {
            std_libs: rust.sysroot.join("lib/rustlib").join(&rust.host).join("lib"),
            libs: PathBuf::new(),
            target: rust.host,
            feature,
            generations,
            next: 1,
            salt: 1,
            sources: BTreeMap::new(),
            watch: Watch::new(&project.dir),
            project: project.clone(),
            options: options.clone(),
        })
    }

    /// Builds the binary and the first generation, and starts the app.
    pub fn start(&mut self) -> Result<Box<dyn Session>> {
        // what the build reads, looked at before it: a save during the
        // build is a change the next look finds
        self.watch = Watch::new(&self.project.dir);
        let sources = read_sources(&self.project.dir);
        let (executable, built) =
            platform::desktop::build_with(&self.project, &self.options, Some(&self.target), std::slice::from_ref(&self.feature))?;
        self.libs = built.parent().map(|dir| dir.join("deps")).unwrap_or_default();
        let (generation, _) = self.build_generation(self.salt)?;
        let session = launch(&executable, &generation, &[&self.std_libs, &self.libs], &self.options)?;
        self.sources = sources;
        Ok(session)
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
    fn build_generation(&mut self, salt: u32) -> Result<(PathBuf, Vec<String>)> {
        let mut features = self.options.features.clone();
        features.push(self.feature.clone());
        let built = cargo::build(&cargo::Build {
            manifest: self.project.manifest.clone(),
            package: self.project.package.clone(),
            what: Target::CdylibLib,
            release: false,
            profile: None,
            target: Some(self.target.clone()),
            features,
            env: self.project.build_env(),
            rustc_args: vec![String::from("-C"), format!("metadata=bunny-salt-{salt}")],
            quiet: true,
        })?;
        let extension = built.artifact.extension().map(|ext| ext.to_string_lossy().into_owned()).unwrap_or_default();
        let generation = self.generations.join(format!("generation-{}.{extension}", self.next));
        self.next += 1;
        std::fs::copy(&built.artifact, &generation)
            .map_err(|error| Error::new(format!("{} → {}: {error}", built.artifact.display(), generation.display())))?;
        Ok((generation, built.rebuilt))
    }
}

/// `bunny-ui/hot`, by the name the app gives bunny-ui — when the
/// bunny-ui it resolves to has hot reload.
fn hot_feature(project: &Project, host: &str) -> std::result::Result<String, String> {
    let manifest = project.manifest.to_string_lossy().into_owned();
    let args = ["metadata", "--format-version", "1", "--filter-platform", host, "--manifest-path", manifest.as_str()];
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

/// The running app, with a line to it: `load` orders go in on its
/// standard input, and its answers come back among its output.
struct HotApp {
    app: ChildSession,
    input: ChildStdin,
    answers: Receiver<String>,
    /// The app took its first generation: it takes the next ones too.
    hot: bool,
}

/// Starts the binary on its first generation. Its output comes to this
/// terminal, but for the answers to `bunny run`.
fn launch(executable: &Path, generation: &Path, libs: &[&Path], options: &Options) -> Result<Box<dyn Session>> {
    let mut command = Command::new(executable);
    command.args(&options.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    command.env("BUNNY_HOT_GENERATION", generation);
    // the shared libraries the binary links: the framework, and the
    // standard library of the toolchain that built it
    let variable = if cfg!(target_os = "macos") { "DYLD_LIBRARY_PATH" } else { "LD_LIBRARY_PATH" };
    let mut paths: Vec<PathBuf> = libs.iter().map(|dir| dir.to_path_buf()).collect();
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env(variable, joined);
    }
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        command.env("RUST_BACKTRACE", "1");
    }
    let mut child =
        command.spawn().map_err(|error| Error::new(format!("{} did not start: {error}", executable.display())))?;
    let input = child.stdin.take().ok_or_else(|| Error::new("the app's input did not open"))?;
    let output = child.stdout.take().ok_or_else(|| Error::new("the app's output did not open"))?;
    let (sender, answers) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|read| read > 0) {
            let text = String::from_utf8_lossy(&line);
            match text.strip_prefix("@@bunny-hot ") {
                Some(answer) => {
                    let _ = sender.send(answer.trim_end().to_string());
                }
                None => {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&line);
                    let _ = out.flush();
                }
            }
            line.clear();
        }
    });
    let mut app = HotApp { app: ChildSession { child, on_stop: None }, input, answers, hot: true };
    if let Err(why) = app.answer(LOAD_TIMEOUT) {
        app.hot = false;
        println!("{}", crate::term::warn(&format!("hot reload is off: {why} — r restarts the app")));
    }
    Ok(Box::new(app))
}

impl HotApp {
    /// The app's answer to the last order: how long its swap took.
    fn answer(&mut self, timeout: Duration) -> std::result::Result<Duration, String> {
        let until = Instant::now() + timeout;
        loop {
            match self.answers.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return parse_answer(&answer),
                Err(RecvTimeoutError::Disconnected) => return Err(String::from("the app stopped")),
                Err(RecvTimeoutError::Timeout) => {
                    if matches!(self.app.child.try_wait(), Ok(Some(_))) {
                        return Err(String::from("the app stopped"));
                    }
                    if Instant::now() >= until {
                        return Err(format!("the app did not answer in {} seconds", timeout.as_secs()));
                    }
                }
            }
        }
    }
}

/// `loaded <n> <ms>`, or `failed <why>`.
fn parse_answer(answer: &str) -> std::result::Result<Duration, String> {
    match answer.split_once(' ') {
        Some(("loaded", rest)) => {
            let millis = rest.split_whitespace().nth(1).and_then(|ms| ms.parse().ok()).unwrap_or(0);
            Ok(Duration::from_millis(millis))
        }
        Some(("failed", why)) => Err(why.to_string()),
        _ => Err(format!("the app answered {answer:?}")),
    }
}

impl Session for HotApp {
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>> {
        self.app.wait(timeout)
    }

    fn stop(&mut self) {
        self.app.stop();
    }

    fn load(&mut self, generation: &Path) -> Option<std::result::Result<Duration, String>> {
        if !self.hot {
            return None;
        }
        // an answer that came too late for its order is not this one's
        while self.answers.try_recv().is_ok() {}
        let sent = writeln!(self.input, "load {}", generation.display()).and_then(|()| self.input.flush());
        if sent.is_err() {
            return Some(Err(String::from("the app stopped")));
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
        assert_eq!(parse_answer("failed it has no entry"), Err(String::from("it has no entry")));
        assert!(parse_answer("hello").is_err());
    }
}
