//! `bunny run`: build the app for a device, start it there, and keep it
//! running — its output in this terminal, a save swapping its code on
//! this computer (hot reload), `R` to restart, `q` to stop.

use std::time::{Duration, Instant};

use crate::args::{HELP, Matches, Opt};
use crate::commands::doctor;
use crate::devices::{self, Device, Kind, Platform, State};
use crate::error::{Error, Result};
use crate::hot::{Change, Hot, Missed};
use crate::keys::{INTERRUPT, Keys};
use crate::platform::{self, Options, Session};
use crate::project::Project;
use crate::serve::Server;
use crate::term;

pub const SUMMARY: &str = "Run the app on this computer, a simulator, an emulator or a phone";

pub const USAGE: &str = "bunny run [-d <DEVICE>] [OPTIONS] [-- <APP ARGS>...]";

pub const ABOUT: &str = "\
Builds the app for a device and starts it there, its output in this
terminal. Without -d it runs on this computer; -d takes an id or a name from
`bunny devices`, or a platform (ios, macos…) — a simulator that is off is
booted first. `-d web` serves the page and opens the browser; a new build
reloads it, and the page's console errors show here.

On macOS and Linux, a debug run on this computer reloads hot: a save builds
the app's library again and swaps the code of the running app, which keeps
its state. An edit that reaches a type starts the state of the app's own
types over; a change to Cargo.toml, build.rs or src/main.rs restarts the app.

While it runs: r reloads now (restarts, where there is no hot reload), R
restarts the app with the code as it is now, q stops it.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("device", "DEVICE", "Where to run: an id, a name, or a platform (default: this computer)").short('d'),
    Opt::flag("release", "Build optimized").short('r'),
    Opt::value("features", "LIST", "Cargo features to turn on (repeatable)"),
    Opt::value("package", "NAME", "The app to run, in a workspace of several").short('p'),
    Opt::flag("detach", "Start the app and return, without watching it"),
    Opt::flag("no-hot", "Restart on r instead of reloading the code in place"),
    Opt::value("web-port", "PORT", "The port the page is served on (default: 8080, or the next free one)"),
    Opt::value("web-hostname", "HOST", "The address the page is served on (default: 127.0.0.1)"),
    Opt::flag("no-open", "Serve the page without opening a browser"),
    HELP,
];

pub fn run(matches: &Matches) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let project = Project::discover(&cwd, matches.value("package"))?;
    let options = Options {
        release: matches.flag("release"),
        features: matches.values("features").iter().flat_map(|list| list.split(',')).map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect(),
        args: matches.rest.clone(),
        detach: matches.flag("detach"),
    };
    let device = pick_device(matches.value("device"))?;
    doctor::preflight(device.platform)?;
    let device = if device.state == State::Ready {
        device
    } else {
        println!("Starting {}…", term::bold(&device.name));
        devices::boot(&device)?
    };
    let web = Web {
        host: matches.value("web-hostname").unwrap_or("127.0.0.1").to_string(),
        port: match matches.value("web-port") {
            Some(port) => port.parse().map_err(|_| Error::usage(format!("`--web-port {port}` is not a port")))?,
            None => 8080,
        },
        open: !matches.flag("no-open"),
        server: None,
    };
    let hot = device.kind == Kind::Desktop && !options.release && !options.detach && !matches.flag("no-hot");
    let mut target = Runner::new(&project, &device, options, web)?;
    if hot {
        match Hot::prepare(&project, &target.options) {
            Ok(hot) => target.hot = Some(hot),
            Err(why) => println!("{}", term::dim(&format!("no hot reload: {why} · r restarts the app"))),
        }
    }
    println!("{}", term::bold(&format!("Building {} for {}…", project.name, device.name)));
    let Some(session) = target.start()? else {
        println!("{}", term::dim("detached: the app runs on its own"));
        return Ok(());
    };
    watch(&mut target, session)
}

/// The device `-d` names, or this computer.
fn pick_device(query: Option<&str>) -> Result<Device> {
    let Some(query) = query else {
        let host = Platform::host();
        return Ok(Device {
            id: host.key().to_string(),
            name: String::from("this computer"),
            platform: host,
            kind: Kind::Desktop,
            state: State::Ready,
            os: None,
        });
    };
    let found = devices::discover();
    let device = devices::resolve(&found, query)?.clone();
    if let State::Unavailable(why) = &device.state {
        return Err(Error::new(format!("{} is not available: {why}", device.name)));
    }
    Ok(device)
}

/// One app on one device: how to build it and start it, again and again.
struct Runner<'a> {
    project: &'a Project,
    device: &'a Device,
    options: Options,
    web: Web,
    /// Hot reload, when this run has it.
    hot: Option<Hot>,
}

/// The browser's half: the server outlives every rebuild, and the page
/// reloads itself when one lands.
#[derive(Default)]
struct Web {
    host: String,
    port: u16,
    open: bool,
    server: Option<Server>,
}

impl<'a> Runner<'a> {
    fn new(project: &'a Project, device: &'a Device, options: Options, web: Web) -> Result<Runner<'a>> {
        match (device.platform, device.kind) {
            (Platform::Web, _) if options.detach => Err(Error::usage("the page needs `bunny run` to serve it: run it without --detach")),
            (_, Kind::Desktop | Kind::Browser) | (Platform::Ios, Kind::Simulator) | (Platform::Android, _) => {
                Ok(Runner { project, device, options, web, hot: None })
            }
            (Platform::Ios, _) => Err(Error::new("running on an iPhone needs signing, which `bunny` does not do yet")
                .hint("run on a simulator meanwhile: bunny run -d ios")),
            (platform, _) => Err(Error::new(format!("`bunny run` does not run {} apps yet", platform.title()))
                .hint("it runs on this computer, the iOS Simulator, Android and the browser")),
        }
    }

    /// Builds and starts the app (`None`: started detached).
    fn start(&mut self) -> Result<Option<Box<dyn Session>>> {
        let began = Instant::now();
        let session = match (self.device.platform, self.device.kind) {
            (Platform::Web, _) => {
                let site = platform::web::build(self.project, &self.options, true)?;
                match &self.web.server {
                    Some(server) => {
                        server.reload();
                        announce(began, self.device);
                    }
                    None => {
                        let server = Server::start(site.dir, &self.web.host, self.web.port)
                            .map_err(|error| Error::new(format!("the page could not be served: {error}")))?;
                        announce(began, self.device);
                        println!("{}", term::bold(&format!("    {}", server.url())));
                        if self.web.open {
                            platform::web::open(&server.url());
                        }
                        self.web.server = Some(server);
                    }
                }
                Some(Box::new(Page) as Box<dyn Session>)
            }
            (Platform::Android, _) => {
                let toolchain = platform::android::Toolchain::find()?;
                let apk = platform::android::build(self.project, &self.options, &toolchain, &self.device.id)?;
                let session = platform::android::launch(&toolchain, &self.device.id, &apk, &self.options)?;
                announce(began, self.device);
                session
            }
            (_, Kind::Desktop) if self.hot.is_some() => {
                let session = self.hot.as_mut().map(Hot::start).transpose()?;
                announce(began, self.device);
                session
            }
            (_, Kind::Desktop) => {
                let executable = platform::desktop::build(self.project, &self.options)?;
                announce(began, self.device);
                platform::desktop::launch(&executable, &self.options)?
            }
            _ => {
                let app = platform::ios::build_simulator(self.project, &self.options)?;
                let session = platform::ios::launch_simulator(self.device, &app, &self.options)?;
                announce(began, self.device);
                session
            }
        };
        Ok(session)
    }
}

/// The page in the browser: it ends when the person closes `bunny run`.
struct Page;

impl Session for Page {
    fn wait(&mut self, timeout: Duration) -> Option<Option<i32>> {
        std::thread::sleep(timeout);
        None
    }

    fn stop(&mut self) {}
}

fn announce(began: Instant, device: &Device) {
    println!(
        "{} {}",
        term::ok(&format!("Running on {}", device.name)),
        term::dim(&format!("(built in {:.1}s)", began.elapsed().as_secs_f32()))
    );
}

const KEY_HELP: &str = "r / R  restart with the code as it is now\nq      stop the app\nh      this help";

const KEY_HELP_HOT: &str = "\
a save reloads the code of the app; it keeps its state
r      reload now
R      restart the app
q      stop the app
h      this help";

/// How often a hot run looks for saves.
const LOOK: Duration = Duration::from_millis(150);

/// Keeps the app running: a save reloads it (hot), a key restarts or
/// stops it, and its exit ends `bunny run` with its code — but a hot app
/// that crashed waits for the fix.
fn watch(runner: &mut Runner, session: Box<dyn Session>) -> Result<()> {
    let mut keys = Keys::start();
    let hot = runner.hot.is_some();
    match (keys.is_some(), hot) {
        (true, true) => println!("{}", term::dim("a save reloads the app · r reload · R restart · q quit · h help")),
        (true, false) => println!("{}", term::dim("r restart · q quit · h help")),
        (false, true) => println!("{}", term::dim("a save reloads the app")),
        (false, false) => {}
    }
    raw(&mut keys);
    let mut app = Some(session);
    let mut looked = Instant::now();
    loop {
        if let Some(code) = app.as_mut().and_then(|running| running.wait(Duration::ZERO)) {
            if !hot || code == Some(0) {
                cooked(&mut keys);
                return finish(code);
            }
            app = None;
            cooked(&mut keys);
            let how = code.map_or_else(|| String::from("a signal ended it"), |code| format!("exit code {code}"));
            println!("{}", term::red(&format!("The app stopped ({how}).")));
            println!("{}", term::dim("save a fix or press R to start it again · q quit"));
            raw(&mut keys);
        }
        if hot && looked.elapsed() >= LOOK {
            looked = Instant::now();
            match (runner.hot.as_mut().and_then(Hot::poll), app.as_mut()) {
                (None, _) => {}
                (Some(Change::Code), Some(running)) => {
                    if let Some(why) = reload(runner, running.as_mut(), &mut keys) {
                        app = restart(runner, app.take(), &mut keys, Some(&why))?;
                    }
                }
                (Some(Change::Restart(why)), _) => app = restart(runner, app.take(), &mut keys, Some(&why))?,
                (Some(Change::Code), None) => app = restart(runner, None, &mut keys, None)?,
            }
        }
        let key = match keys.as_ref() {
            Some(keys) => keys.next(Duration::from_millis(50)),
            None => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
        };
        let Some(key) = key else { continue };
        match key {
            'r' if hot && app.is_some() => {
                if let Some(running) = app.as_mut()
                    && let Some(why) = reload(runner, running.as_mut(), &mut keys)
                {
                    app = restart(runner, app.take(), &mut keys, Some(&why))?;
                }
            }
            'r' | 'R' => app = restart(runner, app.take(), &mut keys, None)?,
            'q' | INTERRUPT => {
                cooked(&mut keys);
                if let Some(running) = app.as_mut() {
                    running.stop();
                }
                println!("{}", term::dim("stopped"));
                if key == INTERRUPT {
                    // the shell's own code for Ctrl-C
                    std::process::exit(130);
                }
                return Ok(());
            }
            'h' => {
                cooked(&mut keys);
                println!("{}", if hot { KEY_HELP_HOT } else { KEY_HELP });
                raw(&mut keys);
            }
            _ => {}
        }
    }
}

/// A hot reload of the code as it is now. `Some(why)`: only a restart
/// carries the change.
fn reload(runner: &mut Runner, app: &mut dyn Session, keys: &mut Option<Keys>) -> Option<String> {
    let hot = runner.hot.as_mut()?;
    // the build is cargo's: Ctrl-C reaches it
    cooked(keys);
    let mut restart = None;
    match hot.reload(app) {
        Ok(done) => {
            let note = if done.new_types { " · a type changed: the state of the app's own types starts over" } else { "" };
            println!("{}{}", term::ok(&format!("Reloaded in {}", seconds(done.took))), term::dim(note));
        }
        Err(Missed::Build) => println!("{}", term::dim("the app keeps the code it had: fix it and save")),
        Err(Missed::Restart(why)) => restart = Some(why),
        Err(Missed::Load(why)) => {
            term::print_error(&format!("the app did not take the new code: {why}"), Some("press R to restart it"));
        }
    }
    raw(keys);
    restart
}

/// Stops the app, when one runs, and builds and starts it again. A build
/// that fails leaves no app: fix it and save (hot), or press r.
fn restart(
    runner: &mut Runner,
    app: Option<Box<dyn Session>>,
    keys: &mut Option<Keys>,
    why: Option<&str>,
) -> Result<Option<Box<dyn Session>>> {
    cooked(keys);
    if let Some(mut running) = app {
        running.stop();
    }
    match why {
        Some(why) => println!("{}", term::bold(&format!("{why}: restarting…"))),
        None => println!("{}", term::bold("Restarting…")),
    }
    let started = match runner.start() {
        Ok(next) => next,
        Err(error) => {
            term::print_error(&error.message, error.hint.as_deref());
            let next = if runner.hot.is_some() { "fix it and save, or press R" } else { "fix it and press r" };
            println!("{}", term::dim(&format!("{next} · q quit")));
            None
        }
    };
    raw(keys);
    Ok(started)
}

/// `0.4s`, or `850ms` under a second.
fn seconds(took: Duration) -> String {
    if took < Duration::from_secs(1) { format!("{}ms", took.as_millis()) } else { format!("{:.1}s", took.as_secs_f32()) }
}

fn raw(keys: &mut Option<Keys>) {
    if let Some(keys) = keys.as_mut() {
        keys.raw();
    }
}

fn cooked(keys: &mut Option<Keys>) {
    if let Some(keys) = keys.as_mut() {
        keys.cooked();
    }
}

/// The app ended on its own: `bunny run` ends with its code.
fn finish(code: Option<i32>) -> Result<()> {
    match code {
        Some(0) => {
            println!("{}", term::dim("the app exited"));
            Ok(())
        }
        Some(code) => Err(Error { message: format!("the app exited with code {code}"), hint: None, code: code.clamp(1, 255) as u8 }),
        None => Err(Error::new("the app was ended by a signal")),
    }
}
