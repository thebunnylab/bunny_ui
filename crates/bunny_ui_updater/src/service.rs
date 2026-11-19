//! The updater of a running app: the checks on a schedule, the install
//! on a click, the restart, all on the UI thread, with the network and
//! the disk on threads of their own.
//!
//! ## Wiring
//!
//! `bunny_ui::app!` calls [`start`] before the window opens, when the
//! build has an updater configuration (the `updater` feature and
//! `[package.metadata.bunny.updater]`). An app with its own `main` calls
//! [`start`] itself. Views read [`state`] and call [`click`].
//!
//! ## Production gotchas
//!
//! **One updater per process.** A second [`start`] does nothing.
//!
//! **The UI thread only.** The state is a `State` of the UI thread, so
//! every function here is called from there.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use bunny_ui_core::app::AppEvent;
use bunny_ui_core::prelude::State;
use bunny_ui_core::task;
use bunny_ui_release::{InstallProgress, Installed, Transfer, UpdateError, UpdateStatus};

use crate::log::{Failure, UpdateLog};
use crate::cycle::{FIRST_CHECK_DELAY, UpdateState, ask_on_return, next_check};
use crate::apply::{ApplyStrategy, PlatformStrategy};
use crate::{Fetch, Updater, UpdaterConfig};

/// When the checks run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// The first check after [`FIRST_CHECK_DELAY`], then each
    /// [`crate::cycle::POLL_INTERVAL`], and on a return to the app when
    /// the last answer is old.
    Automatic,
    /// Only when the app calls [`check_now`].
    Manual,
}

struct Service {
    config: Arc<UpdaterConfig>,
    fetch: Arc<dyn Fetch>,
    strategy: Arc<dyn ApplyStrategy + Send + Sync>,
    state: State<UpdateState>,
    log: UpdateLog,
    before_restart: Option<Rc<dyn Fn() -> bool>>,
    /// When the feed was last asked, or the start: the window that opens
    /// at launch is a "return" too, and it must not run ahead of the
    /// first check's wait.
    asked_at: Instant,
    checking: bool,
}

thread_local! {
    static SERVICE: RefCell<Option<Service>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Service) -> R) -> Option<R> {
    SERVICE.with(|service| service.borrow_mut().as_mut().map(f))
}

/// Starts the updater of this app, with the `curl` transport.
#[cfg(feature = "curl")]
pub fn start(config: UpdaterConfig, schedule: Schedule) {
    start_with(config, Arc::new(crate::CurlFetch::default()), schedule);
}

/// Starts the updater of this app with a transport of the app's own. A
/// second call does nothing.
pub fn start_with(config: UpdaterConfig, fetch: Arc<dyn Fetch>, schedule: Schedule) {
    let strategy = Arc::new(PlatformStrategy::for_config(&config));
    start_with_strategy(config, fetch, strategy, schedule);
}

/// [`start_with`] with an install strategy of the app's own.
pub fn start_with_strategy(
    config: UpdaterConfig,
    fetch: Arc<dyn Fetch>,
    strategy: Arc<dyn ApplyStrategy + Send + Sync>,
    schedule: Schedule,
) {
    let fresh = SERVICE.with(|service| {
        let mut service = service.borrow_mut();
        if service.is_some() {
            return false;
        }
        *service = Some(Service {
            log: UpdateLog::new(config.log_path()),
            config: Arc::new(config),
            fetch,
            strategy,
            state: State::new(UpdateState::Idle),
            before_restart: None,
            asked_at: Instant::now(),
            checking: false,
        });
        true
    });
    if fresh && schedule == Schedule::Automatic {
        task::spawn(async {
            task::sleep(FIRST_CHECK_DELAY).await;
            loop {
                ask().await;
                let Some(next) = next_check(&current()) else { return };
                task::sleep(next).await;
            }
        })
        .detach();
        let (events, heard) = task::channel();
        bunny_ui_core::app::subscribe(events);
        task::spawn(async move {
            while let Some(event) = heard.recv().await {
                let back = matches!(event, AppEvent::DidWake | AppEvent::WindowActive { active: true, .. });
                let since = with(|service| service.asked_at.elapsed());
                if back && since.is_some_and(|since| ask_on_return(&current(), since)) {
                    task::spawn(ask()).detach();
                }
            }
        })
        .detach();
    }
}

/// Whether an updater runs in this process.
#[must_use]
pub fn is_running() -> bool {
    with(|_| ()).is_some()
}

/// The state of the update, for a view to read. `None` when no updater
/// runs: a build with no feed, a store build, the web.
#[must_use]
pub fn state() -> Option<State<UpdateState>> {
    with(|service| service.state)
}

/// The state now, [`UpdateState::Idle`] when no updater runs.
#[must_use]
pub fn current() -> UpdateState {
    state().map(|state| state.wrappedValue()).unwrap_or_default()
}

fn publish(next: UpdateState) {
    if let Some(state) = state() {
        state.set(next);
    }
}

/// Asks the feed now, unless a check runs already.
pub fn check_now() {
    task::spawn(ask()).detach();
}

/// The check: on a thread, then back here with the answer.
async fn ask() {
    let Some((config, fetch)) = with(|service| {
        if service.checking {
            return None;
        }
        service.checking = true;
        Some((service.config.clone(), service.fetch.clone()))
    })
    .flatten() else {
        return;
    };
    let answer = off_thread(move || Updater::with_fetch((*config).clone(), fetch).check()).await;
    with(|service| {
        service.checking = false;
        service.asked_at = Instant::now();
    });
    match answer {
        Some(Ok(status)) => land_check(status),
        Some(Err(error)) => record(Failure::Check(&error)),
        None => record(Failure::Check(&UpdateError::Transport("the check thread stopped".into()))),
    }
}

/// An answer changes only a state that waits for one: never an install
/// in flight, a restart that waits, or a failure the user has not seen.
fn land_check(status: UpdateStatus) {
    let waiting = matches!(current(), UpdateState::Idle | UpdateState::Available(_));
    match status {
        UpdateStatus::Available(offer) if waiting => publish(UpdateState::Available(offer)),
        UpdateStatus::Retired { minimum, upgrade } if waiting || matches!(current(), UpdateState::Failed) => {
            publish(UpdateState::Retired { minimum, upgrade: upgrade.map(Box::new) });
        }
        // up to date: an offer on screen stays; it was not taken back
        _ => {}
    }
}

fn record(failure: Failure<'_>) {
    with(|service| service.log.record(&service.config.current, failure));
}

/// Downloads, verifies and installs the build on offer. The running
/// process stays the old build: the state then waits for a restart.
pub fn install() {
    let Some(offer) = current().offer().cloned() else { return };
    let Some((config, fetch, strategy)) =
        with(|service| (service.config.clone(), service.fetch.clone(), service.strategy.clone()))
    else {
        return;
    };
    publish(UpdateState::Installing(InstallProgress::Downloading(Transfer::new(0, offer.asset.size))));
    let (stages, heard) = task::channel::<Stage>();
    let asset = offer.asset.clone();
    std::thread::spawn(move || {
        let updater = Updater::with_fetch((*config).clone(), fetch);
        let report = stages.clone();
        let outcome = updater.update_with(&*strategy, &asset, &mut |stage| {
            // nobody watches any more: the install goes on
            let _ = report.send(Stage::Reached(stage));
        });
        let _ = stages.send(Stage::Done(outcome));
    });
    task::spawn(async move {
        while let Some(stage) = heard.recv().await {
            match stage {
                // a late stage never paints over the end
                Stage::Reached(stage) => {
                    if matches!(current(), UpdateState::Installing(_)) {
                        publish(UpdateState::Installing(stage));
                    }
                }
                Stage::Done(Ok(installed)) => {
                    publish(UpdateState::ReadyToRestart { version: offer.version.clone(), outcome: Box::new(installed) });
                    return;
                }
                Stage::Done(Err(error)) => {
                    record(Failure::Install { to: &offer.version, error: &error });
                    publish(UpdateState::Failed);
                    return;
                }
            }
        }
    })
    .detach();
}

enum Stage {
    Reached(InstallProgress),
    Done(Result<Installed, UpdateError>),
}

/// What runs before a restart: `false` keeps the app open. The place
/// for "save your work?". The restart is not lost: the state keeps
/// waiting for it.
pub fn set_before_restart(check: impl Fn() -> bool + 'static) {
    with(|service| service.before_restart = Some(Rc::new(check)));
}

/// Starts the new build and quits, when an install waits for it and the
/// app agrees. On Windows it only quits: the helper waits for that.
pub fn restart() {
    restart_with(|outcome| {
        crate::relaunch(outcome);
        std::process::exit(0);
    });
}

/// [`restart`] with what ends the process given, for a test.
pub(crate) fn restart_with(end: impl FnOnce(&Installed)) {
    let UpdateState::ReadyToRestart { outcome, .. } = current() else { return };
    let agrees = with(|service| service.before_restart.clone()).flatten().is_none_or(|check| check());
    if agrees {
        end(&outcome);
    }
}

/// What a click on the update's control does, for its state: install an
/// offer, restart after an install, show the log after a failure.
pub fn click() {
    match current() {
        UpdateState::ReadyToRestart { .. } => restart(),
        UpdateState::Failed => reveal_log(),
        UpdateState::Available(_) | UpdateState::Retired { upgrade: Some(_), .. } => install(),
        _ => {}
    }
}

/// Shows the update log in the system's file browser, the file selected
/// when the system can.
pub fn reveal_log() {
    let Some(path) = with(|service| service.log.path().to_path_buf()) else { return };
    let folder = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let mut command = if cfg!(target_os = "macos") {
        let mut command = std::process::Command::new("open");
        if path.exists() { command.arg("-R").arg(&path) } else { command.arg(&folder) };
        command
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("explorer");
        if path.exists() {
            let mut select = std::ffi::OsString::from("/select,");
            select.push(&path);
            command.arg(select);
        } else {
            command.arg(&folder);
        }
        command
    } else {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(&folder);
        command
    };
    let _ = command.spawn();
}

/// `work` on a thread of its own, its answer back here. `None` when the
/// thread stopped with no answer.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (answer, heard) = task::channel();
    std::thread::spawn(move || {
        let _ = answer.send(work());
    });
    heard.recv().await
}

#[cfg(test)]
pub(crate) fn stop() {
    SERVICE.with(|service| service.borrow_mut().take());
}

#[cfg(test)]
pub(crate) fn asked_ago() -> Option<std::time::Duration> {
    with(|service| service.asked_at.elapsed())
}

#[cfg(test)]
pub(crate) fn with_checking() -> bool {
    with(|service| service.checking).unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn log_path() -> Option<std::path::PathBuf> {
    with(|service| service.log.path().to_path_buf())
}
