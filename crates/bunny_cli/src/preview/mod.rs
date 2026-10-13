//! The preview lane — what `bunny preview` runs, and what an IDE drives
//! in-process: the app built hot with its preview entries, started as a
//! preview host in a process group of its own, and the wire to it.
//!
//! The lane never prints and never inherits a terminal: everything it
//! would have said is an [`Event`] the caller renders — the build's
//! diagnostics as data, the host's messages as they arrive, a crash with
//! its exit code. A save is noticed by [`Preview::saved`] and carried by
//! [`Preview::reload`] the way `bunny run` carries it: a generation built
//! and loaded into the running host, which answers with new frames; a
//! change the generation cannot carry asks for a [`Preview::restart`].
//!
//! The states are types: [`Idle`] before the first build, [`Live`] with
//! the host on the line, [`Failed`] when the build or the start did not
//! land — and only the state that has a host can be sent to.
//!
//! ## Production gotchas
//!
//! - The host is a child in its own process group (Unix): end it with
//!   [`Preview::stop`], or signal the group by [`Preview::pid`] — a tool
//!   that exits without either leaves a host and its cargo behind.
//! - [`Preview::reload`] blocks for the generation build; an IDE runs the
//!   lane on a thread of its own, since the session is not `Send`.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use bunny_ui_preview::scan::{self, ScanReport};
use bunny_ui_preview::wire::{self, Message, Reader, Received, ToHost, ToIde};

use crate::cargo::{Diagnostic, Diagnostics};
use crate::devices::{Device, Kind, Platform, State};
use crate::error::{Error, Result};
use crate::hot::{Change, Extra, Hot, Missed};
use crate::platform::{self, Session};
use crate::project::Project;

/// How long the host may take to call back after it started.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// What the lane builds with.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Cargo features of the app.
    pub features: Vec<String>,
    /// The package, in a workspace of several.
    pub package: Option<String>,
    /// What the host's process is given beside the lane's own variables
    /// — `BUNNY_PREVIEW_ENGINE=house` asks for the deterministic engines.
    pub env: Vec<(String, OsString)>,
}

/// What the lane says, in order.
#[derive(Debug)]
pub enum Event {
    /// The previews the sources list, before any build.
    Previews(ScanReport),
    BuildStarted,
    /// Stable build inputs tied to the generation acknowledged by the runtime.
    /// None means the build inputs could not be verified; frames remain usable
    /// for display but must not be joined to current editor source by geometry.
    Sources {
        generation: u32,
        sources: Option<crate::hot::sources::BuildSources>,
    },
    /// One thing the compiler said during a build that went on.
    Diagnostic(Diagnostic),
    /// The app and its first generation built, and the host started.
    Built { took: Duration },
    /// A build did not land; the host, if any, keeps the code it had.
    BuildFailed { diagnostics: Vec<Diagnostic> },
    /// A generation loaded into the running host.
    Reloaded { took: Duration, new_types: bool },
    /// A change a generation cannot carry: the host must restart.
    RestartNeeded(String),
    /// A message off the wire.
    Host(ToIde),
    /// What the road noted on the way (hot reload off, and why).
    Note(String),
    /// The host ended on its own.
    Crashed { code: Option<i32> },
}

/// The state before a build.
pub struct Idle {
    project: Project,
    options: Options,
}

/// The state with the host on the line.
pub struct Live {
    /// Boxed: the road carries the project, the watcher and the sources,
    /// and a step is moved by value.
    hot: Box<Hot>,
    app: Box<dyn Session>,
    line: TcpStream,
    inbox: Receiver<Received>,
    pid: u32,
    project: Project,
    options: Options,
}

/// The state after a build or a start that did not land.
pub struct Failed {
    project: Project,
    options: Options,
    why: String,
}

/// The lane, in one of its states.
pub struct Preview<S> {
    state: S,
}

/// Where a start or a restart landed.
pub enum Step {
    Live(Preview<Live>),
    Failed(Preview<Failed>),
}

impl Preview<Idle> {
    /// The lane over the project at `dir`.
    pub fn new(dir: &Path, options: Options) -> Result<Preview<Idle>> {
        let project = Project::discover(dir, options.package.as_deref())?;
        Ok(Preview { state: Idle { project, options } })
    }

    /// The previews the sources list — no build.
    pub fn previews(&self) -> ScanReport {
        scan::scan_dir(&self.state.project.dir.join("src"))
    }

    /// Builds the app hot with its preview entries and starts it as a
    /// host: `Live` with the host on the line, or `Failed` with why.
    pub fn start(self, events: &mut Vec<Event>) -> Step {
        let Idle { project, options } = self.state;
        events.push(Event::Previews(scan::scan_dir(&project.dir.join("src"))));
        start(project, options, events)
    }
}

impl Preview<Failed> {
    /// Why the lane is here.
    pub fn why(&self) -> &str {
        &self.state.why
    }

    /// Builds and starts again — after a fix was saved.
    pub fn retry(self, events: &mut Vec<Event>) -> Step {
        let Failed { project, options, .. } = self.state;
        start(project, options, events)
    }
}

fn start(project: Project, options: Options, events: &mut Vec<Event>) -> Step {
    let failed = |project, options, why: String, events: &mut Vec<Event>| {
        events.push(Event::Note(why.clone()));
        Step::Failed(Preview { state: Failed { project, options, why } })
    };
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) => return failed(project, options, format!("no port for the host to call back on: {error}"), events),
    };
    let port = listener.local_addr().map(|address| address.port()).unwrap_or_default();
    let device = Device {
        id: Platform::host().key().to_string(),
        name: String::from("this computer"),
        platform: Platform::host(),
        kind: Kind::Desktop,
        state: State::Ready,
        os: None,
    };
    let run = platform::Options { release: false, features: options.features.clone(), args: Vec::new(), detach: false };
    let mut env = vec![(String::from("BUNNY_PREVIEW_PORT"), OsString::from(port.to_string()))];
    env.extend(options.env.iter().cloned());
    let extra = Extra { features: vec![String::from("bunny-ui/preview")], env, process_group: true, diagnostics: Diagnostics::Collect };
    let mut hot = match Hot::prepare_with(&project, &run, &device, extra) {
        Ok(hot) => hot,
        Err(why) => return failed(project, options, why, events),
    };
    events.push(Event::BuildStarted);
    let began = Instant::now();
    let mut app = match hot.start() {
        Ok(app) => app,
        Err(error) => {
            if error.diagnostics.is_empty() {
                return failed(project, options, error.message, events);
            }
            events.push(Event::BuildFailed { diagnostics: error.diagnostics });
            return Step::Failed(Preview { state: Failed { project, options, why: String::from("the build failed") } });
        }
    };
    for note in hot.take_notes() {
        events.push(Event::Note(note));
    }
    let Some(pid) = hot.pid() else {
        app.stop();
        return failed(project, options, String::from("the host started with no process id"), events);
    };
    // the host calls back on the preview port
    let line = match accept(&listener, app.as_mut()) {
        Ok(line) => line,
        Err(why) => {
            app.stop();
            return failed(project, options, why, events);
        }
    };
    let Ok(reading) = line.try_clone() else {
        app.stop();
        return failed(project, options, String::from("the line to the host broke"), events);
    };
    let (sender, inbox) = mpsc::channel();
    std::thread::spawn(move || read_line(reading, &sender));
    if let Some(generation) = app.generation() {
        events.push(Event::Sources {
            generation,
            sources: hot.built_sources().cloned(),
        });
    }
    events.push(Event::Built { took: began.elapsed() });
    Step::Live(Preview { state: Live { hot: Box::new(hot), app, line, inbox, pid, project, options } })
}

/// Waits for the host to connect, the app watched meanwhile.
fn accept(listener: &TcpListener, app: &mut dyn Session) -> std::result::Result<TcpStream, String> {
    listener.set_nonblocking(true).map_err(|error| format!("the preview port did not open: {error}"))?;
    let until = Instant::now() + CALL_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(|error| format!("the line to the host broke: {error}"))?;
                return Ok(stream);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(code) = app.wait(Duration::from_millis(50)) {
                    return Err(format!("the host ended before it called back ({})", exit(code)));
                }
                if Instant::now() >= until {
                    return Err(String::from("the host did not call back"));
                }
            }
            Err(error) => return Err(format!("the host's call did not land: {error}")),
        }
    }
}

fn exit(code: Option<i32>) -> String {
    code.map_or_else(|| String::from("a signal ended it"), |code| format!("exit code {code}"))
}

/// Reads frames off the line until it closes.
fn read_line(mut reading: TcpStream, sender: &mpsc::Sender<Received>) {
    let mut reader = Reader::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = match reading.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        reader.feed(&buf[..read]);
        loop {
            match reader.pull() {
                Ok(Some(received)) => {
                    if sender.send(received).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
}

impl Preview<Live> {
    /// The host's process id — the leader of its process group on Unix.
    pub fn pid(&self) -> u32 {
        self.state.pid
    }

    /// The project.
    pub fn project(&self) -> &Project {
        &self.state.project
    }

    /// Sends one message to the host.
    pub fn send(&mut self, message: &ToHost) -> std::io::Result<()> {
        let bytes = wire::encode(&Message::ToHost(message.clone()));
        self.state.line.write_all(&bytes)?;
        self.state.line.flush()
    }

    /// Everything the host sent since the last poll, and whether it still
    /// runs: `false` means the host ended, with `Crashed` among the events.
    pub fn poll(&mut self, events: &mut Vec<Event>) -> bool {
        loop {
            match self.state.inbox.try_recv() {
                Ok(Received::Message(Message::ToIde(message))) => events.push(Event::Host(message)),
                Ok(Received::Message(Message::ToHost(_)) | Received::Unknown { .. }) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if let Some(code) = self.state.app.wait(Duration::ZERO) {
            events.push(Event::Crashed { code });
            return false;
        }
        true
    }

    /// Waits up to `timeout` for the host's next message, then polls.
    pub fn wait(&mut self, timeout: Duration, events: &mut Vec<Event>) -> bool {
        if let Ok(Received::Message(Message::ToIde(message))) = self.state.inbox.recv_timeout(timeout) {
            events.push(Event::Host(message));
        }
        self.poll(events)
    }

    /// What the saves since the last look ask for.
    pub fn saved(&mut self) -> Option<Change> {
        self.state.hot.poll()
    }

    /// Builds the library as it is now and loads it into the host. The
    /// host answers with new frames on its own; a change a generation
    /// cannot carry is a `RestartNeeded` event, a failed build its
    /// diagnostics.
    pub fn reload(&mut self, events: &mut Vec<Event>) {
        events.push(Event::BuildStarted);
        match self.state.hot.reload(self.state.app.as_mut()) {
            Ok(done) => {
                if let Some(generation) = self.state.app.generation() {
                    events.push(Event::Sources {
                        generation,
                        sources: self.state.hot.built_sources().cloned(),
                    });
                }
                events.push(Event::Reloaded {
                    took: done.took,
                    new_types: done.new_types,
                });
            }
            Err(Missed::Build) => events.push(Event::BuildFailed {
                diagnostics: Vec::new(),
            }),
            Err(Missed::Restart(why)) => events.push(Event::RestartNeeded(why)),
            Err(Missed::Load(why)) => events.push(Event::Note(format!("the host did not take the new code: {why}"))),
        }
    }

    /// Stops the host and builds and starts it again.
    pub fn restart(mut self, events: &mut Vec<Event>) -> Step {
        let _ = self.send(&ToHost::Bye);
        self.state.app.stop();
        let Live { project, options, .. } = self.state;
        start(project, options, events)
    }

    /// Ends the host.
    pub fn stop(mut self) {
        let _ = self.send(&ToHost::Bye);
        self.state.app.stop();
    }
}

impl From<Error> for Event {
    fn from(error: Error) -> Event {
        if error.diagnostics.is_empty() { Event::Note(error.message) } else { Event::BuildFailed { diagnostics: error.diagnostics } }
    }
}
