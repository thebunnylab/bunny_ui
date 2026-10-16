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
//! An edit that reaches a type ([`classify`]) is compiled first, then
//! restarts the app. A new build salt cannot distinguish newly inserted
//! primitive State fields from live fields with the same payload type.
//! `Cargo.toml`, `build.rs`, the binary's own `src/main.rs`, or a rebuilt
//! external dependency also require a restart.
//!
//! The path crates the app builds from — a `ui_kit` beside it — are
//! watched with it: a save inside one of their function bodies builds
//! the crate again and loads the generation that links it, the state
//! kept. An edit that reaches one of their types restarts the app
//! instead, and says so: a salt renames the app's own types, not a
//! sibling's, so there is no new arena for the moved type to start over
//! in. A change under a framework crate (`bunny-ui*`, when it is a path
//! dependency) restarts too — the framework is one shared library.

pub mod classify;
pub mod sources;
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
use crate::toolchains;
use classify::Reach;
use sources::{BuildInputs, BuildSources};
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
/// A path crate the app builds from, beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Local {
    name: String,
    dir: PathBuf,
    /// `bunny-ui*`: one shared library, loaded once — a change restarts.
    framework: bool,
}

pub struct Hot {
    project: Project,
    options: Options,
    /// The path crates the app depends on, the framework's among them.
    locals: Vec<Local>,
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
    initial_inputs: Option<BuildInputs>,
    built_sources: Option<BuildSources>,
    /// What a lane that renders no terminal asks for.
    extra: Extra,
    /// What the road would have said on the terminal — hot reload off,
    /// and why. `bunny run` prints them; an IDE reads them.
    notes: Vec<String>,
    /// The app's process id, once started.
    pid: Option<u32>,
}

/// What a preview lane asks of the hot road beyond `bunny run`'s: more
/// features of the framework, more environment for the app, the app in a
/// process group of its own so a tool can end it with everything it
/// spawned, and the compiler's diagnostics as data instead of a terminal.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// Framework features beyond `hot` (`bunny-ui/preview`).
    pub features: Vec<String>,
    /// Environment for the app (`BUNNY_PREVIEW_PORT`).
    pub env: Vec<(String, OsString)>,
    /// Start the app as the leader of a new process group (Unix), its
    /// output nowhere.
    pub process_group: bool,
    /// Where the builds' diagnostics go.
    pub diagnostics: cargo::Diagnostics,
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
        Hot::prepare_with(project, options, device, Extra::default())
    }

    /// [`Hot::prepare`] with what a lane asks beyond `bunny run`'s.
    pub fn prepare_with(project: &Project, options: &Options, device: &Device, extra: Extra) -> std::result::Result<Hot, String> {
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
        let metadata = metadata(project, &target)?;
        let feature = hot_feature(&metadata, project)?;
        for wanted in &extra.features {
            if !framework_has_feature(&metadata, wanted.trim_start_matches("bunny-ui/")) {
                return Err(format!("the app's bunny-ui does not have `{wanted}` yet (update it: cargo update -p bunny-ui)"));
            }
        }
        let locals = locals(&metadata, project);
        let generations = project.out_dir("hot", &target, false).join("generations");
        platform::fresh_dir(&generations).map_err(|error| error.message)?;
        let mut roots = vec![project.dir.clone()];
        roots.extend(locals.iter().map(|local| local.dir.clone()));
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
            initial_inputs: None,
            built_sources: None,
            watch: Watch::new_many(&roots),
            project: project.clone(),
            options: options.clone(),
            locals,
            extra,
            notes: Vec::new(),
            pid: None,
        })
    }

    /// The app's directory and its path crates' — what is watched and
    /// read.
    fn roots(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.project.dir.clone()];
        roots.extend(self.locals.iter().map(|local| local.dir.clone()));
        roots
    }

    /// Which package a file belongs to: the app's, or one of its path
    /// crates — the deepest directory that holds it.
    fn local_of(&self, path: &Path) -> Option<&Local> {
        self.locals.iter().filter(|local| path.starts_with(&local.dir)).max_by_key(|local| local.dir.as_os_str().len())
    }

    /// Whether a saved file is one the library builds from at all: a
    /// sibling's own binaries are not.
    fn relevant(&self, path: &Path) -> bool {
        match self.local_of(path) {
            Some(local) if !local.framework => {
                let relative = path.strip_prefix(&local.dir).unwrap_or(path);
                !(relative == Path::new("src/main.rs") || relative.starts_with("src/bin"))
            }
            _ => true,
        }
    }

    /// What a save of `changed` asks for: a file the library does not
    /// build from restarts the app, and so does anything under a
    /// framework crate; the app's own files answer as [`change`] says.
    fn change(&self, changed: &[PathBuf]) -> Change {
        let mut own = Vec::new();
        for path in changed {
            let Some(local) = self.local_of(path) else {
                own.push(path.clone());
                continue;
            };
            let relative = path.strip_prefix(&local.dir).unwrap_or(path);
            if local.framework {
                return Change::Restart(format!("{} changed: the framework is one shared library", local.name));
            }
            if relative == Path::new("Cargo.toml") || relative == Path::new("build.rs") {
                return Change::Restart(format!("{}/{} changed", local.name, relative.display()));
            }
        }
        change(&self.project.dir, &own)
    }

    /// The library's sources, the path crates' with them: every `.rs`
    /// under each `src/`, the binaries' own files and the framework's
    /// left out.
    fn read_sources(&self) -> BTreeMap<PathBuf, String> {
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
        for root in self.roots() {
            walk(&root.join("src"), &mut sources);
        }
        sources.retain(|path, _| self.relevant(path) && !matches!(self.change(std::slice::from_ref(path)), Change::Restart(_)));
        sources
    }

    /// Whether a sibling crate's edit reached one of its types, by name.
    fn sibling_types_moved(&self, now: &BTreeMap<PathBuf, String>) -> Option<&str> {
        self.locals.iter().filter(|local| !local.framework).find(|local| reaches_types_under(&local.dir, &self.sources, now)).map(|local| local.name.as_str())
    }

    /// What the road would have said on the terminal since the last
    /// take — `bunny run` prints it, an IDE shows it.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// The app's process id, once started — and the leader of its
    /// process group when [`Extra::process_group`] asked for one.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The project the road builds.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Sources observed stable around the last successful build. None means
    /// provenance is unavailable; callers must not substitute current disk text.
    pub fn built_sources(&self) -> Option<&BuildSources> {
        self.built_sources.as_ref()
    }

    fn capture_inputs(&mut self) -> Option<BuildInputs> {
        match BuildInputs::capture(&self.roots()) {
            Ok(inputs) => Some(inputs),
            Err(error) => {
                self.notes
                    .push(format!("source provenance unavailable: {error}"));
                None
            }
        }
    }

    fn finish_inputs(&mut self, inputs: Option<BuildInputs>) {
        self.built_sources = inputs.and_then(|inputs| match inputs.finish() {
            Ok(sources) => {
                if sources.is_none() {
                    self.notes.push(String::from(
                        "source provenance unavailable: inputs changed during the build",
                    ));
                }
                sources
            }
            Err(error) => {
                self.notes
                    .push(format!("source provenance unavailable: {error}"));
                None
            }
        });
    }

    /// Builds the app with its first generation, and starts it.
    pub fn start(&mut self) -> Result<Box<dyn Session>> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| Error::new(format!("no port for the app to call back on: {error}")))?;
        let port = listener.local_addr().map(|address| address.port()).unwrap_or_default().to_string();
        // what the build reads, looked at before it: a save during the
        // build is a change the next look finds
        self.watch = Watch::new_many(&self.roots());
        let sources = self.read_sources();
        let inputs = self.capture_inputs();
        let mut features = vec![self.feature.clone()];
        features.extend(self.extra.features.iter().cloned());
        let feature = features.as_slice();
        let (app, bytes) = match &self.place {
            Place::Desktop => {
                let (executable, built) =
                    platform::desktop::build_with_diagnostics(&self.project, &self.options, Some(&self.target), feature, self.extra.diagnostics)?;
                let (generation, _) = self.build_generation(self.salt)?;
                let mut env = vec![(String::from("BUNNY_HOT_PORT"), OsString::from(&port)), (String::from("BUNNY_HOT_GENERATION"), generation.into_os_string())];
                env.extend(self.extra.env.iter().cloned());
                let (session, pid) = launch(&executable, &env, &[self.std_libs.clone(), deps(&built)], &self.options, self.extra.process_group)?;
                self.pid = Some(pid);
                (session, false)
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
                    self.notes.push(format!("adb reverse failed: {}", reversed.stderr.trim()));
                }
                let session = android::launch(toolchain, serial, &apk, &self.options)?;
                (session.ok_or_else(|| Error::new("the app started detached"))?, true)
            }
        };
        self.sources = sources;
        self.initial_inputs = inputs.clone();
        self.finish_inputs(inputs);
        if self.built_sources.is_none() {
            self.initial_inputs = None;
        }
        let (session, note) = call_back(listener, app, bytes);
        if let Some(why) = note {
            self.notes.push(format!("hot reload is off: {why} — r restarts the app"));
        }
        Ok(session)
    }

    /// What the saves since the last look ask for — `None` when nothing
    /// was saved.
    pub fn poll(&mut self) -> Option<Change> {
        let changed: Vec<PathBuf> = self.watch.changed().into_iter().filter(|path| self.relevant(path)).collect();
        if changed.is_empty() {
            return None;
        }
        Some(self.change(&changed))
    }

    /// Builds the library as it is now and loads it into the running app.
    pub fn reload(&mut self, app: &mut dyn Session) -> std::result::Result<Reloaded, Missed> {
        let began = Instant::now();
        let sources = self.read_sources();
        let inputs = self.capture_inputs().filter(|now| {
            self.initial_inputs.as_ref().is_some_and(|initial| {
                now.unchanged_where(initial, |path| {
                    matches!(self.change(&[path.to_path_buf()]), Change::Restart(_))
                })
            })
        });
        // a sibling's type that moved has no salt to start over under
        if let Some(name) = self.sibling_types_moved(&sources) {
            return Err(Missed::Restart(format!("a type of {name} changed: a sibling's types keep their state only through a restart")));
        }
        let new_types = reaches_types_under(&self.project.dir, &self.sources, &sources);
        let salt = if new_types { self.salt + 1 } else { self.salt };
        let (generation, rebuilt) = self.build_generation(salt).map_err(|_| Missed::Build)?;
        // a sibling built again is the save that was just made; anything
        // else built again — the framework, a registry crate — is a restart
        if let Some(dependency) = rebuilt.iter().find(|name| !self.locals.iter().any(|local| !local.framework && local.name == **name)) {
            return Err(Missed::Restart(format!("{dependency} was built again")));
        }
        // Salting renames application types, not primitive State payloads. A
        // new field can otherwise claim a live sibling's ordinal. Compile first
        // so a failed edit leaves the old app alive, then reset all execution.
        if new_types {
            return Err(Missed::Restart(String::from("an application type changed: live state must restart")));
        }
        match app.load(&generation) {
            None => Err(Missed::Load(String::from("this app does not take new code"))),
            Some(Err(why)) => Err(Missed::Load(why)),
            Some(Ok(_)) => {
                self.salt = salt;
                self.sources = sources;
                self.finish_inputs(inputs);
                Ok(Reloaded { took: began.elapsed(), new_types })
            }
        }
    }

    /// The library as a shared library of its own, salted, copied under a
    /// new name — and the dependencies the build compiled again.
    fn build_generation(&self, salt: u32) -> Result<(PathBuf, Vec<String>)> {
        let mut features = self.options.features.clone();
        features.push(self.feature.clone());
        features.extend(self.extra.features.iter().cloned());
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
            diagnostics: self.extra.diagnostics,
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

/// The app's dependency graph for `target`, as cargo answers it.
fn metadata(project: &Project, target: &str) -> std::result::Result<Value, String> {
    let manifest = project.manifest.to_string_lossy().into_owned();
    let args = ["metadata", "--format-version", "1", "--filter-platform", target, "--manifest-path", manifest.as_str()];
    let out = process::run_in("cargo", &args, Some(&project.dir), &[], Duration::from_secs(120))
        .map_err(|error| format!("cargo: {error}"))?;
    if !out.ok() {
        return Err(format!("cargo could not read the app's dependencies: {}", out.stderr.trim()));
    }
    json::parse(&out.stdout).map_err(|error| format!("cargo metadata: {error}"))
}

/// The path crates the app depends on, directly or through another: a
/// package with no source is a path, and the framework's own are told
/// apart by their name.
fn locals(metadata: &Value, project: &Project) -> Vec<Local> {
    let packages = metadata.get("packages").map(Value::as_array).unwrap_or_default();
    let by_id: BTreeMap<&str, &Value> = packages.iter().filter_map(|package| Some((package.str_at(&["id"])?, package))).collect();
    let nodes = metadata.path(&["resolve", "nodes"]).map(Value::as_array).unwrap_or_default();
    let deps_of: BTreeMap<&str, Vec<&str>> = nodes
        .iter()
        .filter_map(|node| {
            let deps = node.get("deps").map(Value::as_array).unwrap_or_default().iter().filter_map(|dep| dep.str_at(&["pkg"])).collect();
            Some((node.str_at(&["id"])?, deps))
        })
        .collect();
    let Some(app) = packages.iter().find(|package| package.str_at(&["manifest_path"]).map(Path::new) == Some(project.manifest.as_path())).and_then(|package| package.str_at(&["id"]))
    else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = vec![app];
    let mut found = Vec::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        for dep in deps_of.get(id).into_iter().flatten() {
            queue.push(dep);
        }
        if id == app {
            continue;
        }
        let Some(package) = by_id.get(id) else { continue };
        if package.str_at(&["source"]).is_some() {
            continue;
        }
        let (Some(name), Some(manifest)) = (package.str_at(&["name"]), package.str_at(&["manifest_path"])) else { continue };
        let Some(dir) = Path::new(manifest).parent() else { continue };
        found.push(Local { name: name.to_string(), dir: dir.to_path_buf(), framework: name.starts_with("bunny-ui") });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// `bunny-ui/hot`, by the name the app gives bunny-ui — when the
/// bunny-ui it resolves to has hot reload.
fn hot_feature(metadata: &Value, project: &Project) -> std::result::Result<String, String> {
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

/// Whether the app's `bunny-ui` has the feature `name`.
fn framework_has_feature(metadata: &Value, name: &str) -> bool {
    let packages = metadata.get("packages").map(Value::as_array).unwrap_or_default();
    packages.iter().filter(|package| package.str_at(&["name"]) == Some("bunny-ui")).any(|package| {
        package.get("features").is_some_and(|features| features.members().iter().any(|(feature, _)| feature == name))
    })
}

/// What a save of the app's own `changed` files asks for: a file the
/// library does not build from restarts the app.
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

/// Whether the edit from `old` to `new`, among the files under `root`,
/// reaches a type: a file that came or went (a module, an item set), or
/// an edit outside function bodies.
fn reaches_types_under(root: &Path, old: &BTreeMap<PathBuf, String>, new: &BTreeMap<PathBuf, String>) -> bool {
    let before: Vec<&PathBuf> = old.keys().filter(|path| path.starts_with(root)).collect();
    let after: Vec<&PathBuf> = new.keys().filter(|path| path.starts_with(root)).collect();
    before != after
        || before.iter().any(|path| {
            let (Some(text), Some(now)) = (old.get(*path), new.get(*path)) else { return false };
            classify::classify(text, now) == Reach::Types
        })
}

/// Starts the binary on this computer, its output in this terminal.
fn launch(
    executable: &Path,
    env: &[(String, OsString)],
    libs: &[PathBuf],
    options: &Options,
    process_group: bool,
) -> Result<(Box<dyn Session>, u32)> {
    let mut command = Command::new(executable);
    // a lane's app writes to no terminal: its output goes nowhere
    let (out, err) = if process_group { (Stdio::null(), Stdio::null()) } else { (Stdio::inherit(), Stdio::inherit()) };
    command.args(&options.args).stdin(Stdio::null()).stdout(out).stderr(err);
    #[cfg(unix)]
    if process_group {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
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
    let pid = child.id();
    Ok((Box::new(ChildSession { child, on_stop: None }), pid))
}

/// Waits for the started app to call back, and answers the session that
/// talks to it. An app that never calls runs on, without hot reload.
fn call_back(listener: TcpListener, mut app: Box<dyn Session>, bytes: bool) -> (Box<dyn Session>, Option<String>) {
    if listener.set_nonblocking(true).is_err() {
        return (app, Some(String::from("the port did not open")));
    }
    let until = Instant::now() + CALL_TIMEOUT;
    let stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if app.wait(Duration::from_millis(50)).is_some() {
                    return (app, None);
                }
                if Instant::now() >= until {
                    return (app, Some(String::from("the app did not call back")));
                }
            }
            Err(error) => {
                return (app, Some(format!("the app's call did not land: {error}")));
            }
        }
    };
    match HotApp::new(app, stream, bytes) {
        Ok(hot) => (Box::new(hot), None),
        Err((app, why)) => (app, Some(why)),
    }
}

/// The running app, with a line to it.
struct HotApp {
    generation: Option<u32>,
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
        let mut hot = HotApp {
            app,
            line,
            answers,
            bytes,
            generation: None,
        };
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
                Ok(answer) => {
                    let loaded = parse_answer(&answer)?;
                    self.generation = loaded.generation;
                    return Ok(loaded.took);
                }
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

/// An acknowledgment carries the runtime's generation, never a guessed counter.
#[derive(Debug, PartialEq, Eq)]
struct Loaded {
    generation: Option<u32>,
    took: Duration,
}

/// `loaded <n> <ms>`, `ready`, or `failed <why>`.
fn parse_answer(answer: &str) -> std::result::Result<Loaded, String> {
    match answer.split_once(' ').unwrap_or((answer, "")) {
        ("loaded", rest) => {
            let mut fields = rest.split_whitespace();
            let generation = fields.next().and_then(|n| n.parse().ok());
            let millis = fields.next().and_then(|ms| ms.parse().ok());
            match (generation, millis, fields.next()) {
                (Some(generation), Some(millis), None) => Ok(Loaded {
                    generation: Some(generation),
                    took: Duration::from_millis(millis),
                }),
                _ => Err(format!(
                    "the app sent an invalid load acknowledgment: {answer:?}"
                )),
            }
        }
        ("ready", "") => Ok(Loaded {
            generation: None,
            took: Duration::ZERO,
        }),
        ("failed", why) => Err(why.to_string()),
        _ => Err(format!("the app answered {answer:?}")),
    }
}

impl Session for HotApp {
    fn generation(&self) -> Option<u32> {
        self.generation
    }

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
        let root = Path::new("");
        assert!(!reaches_types_under(root, &one, &body));
        assert!(reaches_types_under(root, &one, &two));
        assert!(reaches_types_under(root, &two, &one));
        // another root's files are not this one's business
        assert!(!reaches_types_under(Path::new("elsewhere"), &one, &two));
    }

    #[test]
    fn the_answers_of_the_app_are_read() {
        assert_eq!(
            parse_answer("loaded 3 164"),
            Ok(Loaded {
                generation: Some(3),
                took: Duration::from_millis(164)
            })
        );
        assert_eq!(
            parse_answer("ready"),
            Ok(Loaded {
                generation: None,
                took: Duration::ZERO
            })
        );
        assert!(parse_answer("loaded x 164").is_err());
        assert!(parse_answer("loaded 3").is_err());
        assert!(parse_answer("loaded 3 x").is_err());
        assert_eq!(parse_answer("failed it has no entry"), Err(String::from("it has no entry")));
        assert!(parse_answer("hello").is_err());
    }
}
