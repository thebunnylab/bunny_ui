//! `bunny run`: build the app for a device, start it there, and keep it
//! running — its output in this terminal, `r`/`R` to restart, `q` to
//! stop.

use std::time::{Duration, Instant};

use crate::args::{HELP, Matches, Opt};
use crate::commands::doctor;
use crate::devices::{self, Device, Kind, Platform, State};
use crate::error::{Error, Result};
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

While it runs: r or R restarts it with the code as it is now, q stops it.";

pub const OPTIONS: &[Opt] = &[
    Opt::value("device", "DEVICE", "Where to run: an id, a name, or a platform (default: this computer)").short('d'),
    Opt::flag("release", "Build optimized").short('r'),
    Opt::value("features", "LIST", "Cargo features to turn on (repeatable)"),
    Opt::value("package", "NAME", "The app to run, in a workspace of several").short('p'),
    Opt::flag("detach", "Start the app and return, without watching it"),
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
    let mut target = Runner::new(&project, &device, options, web)?;
    println!("{}", term::bold(&format!("Building {} for {}…", project.name, device.name)));
    let Some(mut session) = target.start()? else {
        println!("{}", term::dim("detached: the app runs on its own"));
        return Ok(());
    };
    watch(&mut target, &mut session)
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
                Ok(Runner { project, device, options, web })
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

/// Keeps the app running: its exit ends `bunny run` with its code, a key
/// restarts or stops it.
fn watch(runner: &mut Runner, session: &mut Box<dyn Session>) -> Result<()> {
    let Some(mut keys) = Keys::start() else {
        // no terminal to read keys from: wait for the app, as cargo run does
        loop {
            if let Some(code) = session.wait(Duration::from_secs(3600)) {
                return finish(code);
            }
        }
    };
    println!("{}", term::dim("r restart · q quit · h help"));
    keys.raw();
    loop {
        if let Some(code) = session.wait(Duration::ZERO) {
            keys.cooked();
            return finish(code);
        }
        let Some(key) = keys.next(Duration::from_millis(50)) else { continue };
        match key {
            'r' | 'R' => {
                keys.cooked();
                if key == 'r' {
                    println!("{}", term::dim("hot reload is on its way; restarting instead"));
                }
                session.stop();
                println!("{}", term::bold("Restarting…"));
                match runner.start() {
                    Ok(Some(next)) => *session = next,
                    Ok(None) => return Ok(()),
                    Err(error) => {
                        // a build that fails keeps `bunny run` alive: fix and press r
                        term::print_error(&error.message, error.hint.as_deref());
                        println!("{}", term::dim("fix it and press r, or q to quit"));
                        keys.raw();
                        wait_for_retry(runner, session, &mut keys)?;
                        continue;
                    }
                }
                keys.raw();
            }
            'q' | INTERRUPT => {
                keys.cooked();
                session.stop();
                println!("{}", term::dim("stopped"));
                if key == INTERRUPT {
                    // the shell's own code for Ctrl-C
                    std::process::exit(130);
                }
                return Ok(());
            }
            'h' => {
                keys.cooked();
                println!("{KEY_HELP}");
                keys.raw();
            }
            _ => {}
        }
    }
}

/// After a failed build there is no app: only `r` (try again) or `q`.
fn wait_for_retry(runner: &mut Runner, session: &mut Box<dyn Session>, keys: &mut Keys) -> Result<()> {
    loop {
        match keys.next(Duration::from_millis(200)) {
            Some('r' | 'R') => {
                keys.cooked();
                println!("{}", term::bold("Restarting…"));
                match runner.start() {
                    Ok(Some(next)) => {
                        *session = next;
                        keys.raw();
                        return Ok(());
                    }
                    Ok(None) => return Ok(()),
                    Err(error) => {
                        term::print_error(&error.message, error.hint.as_deref());
                        keys.raw();
                    }
                }
            }
            Some('q' | INTERRUPT) => {
                keys.cooked();
                std::process::exit(0);
            }
            _ => {}
        }
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
