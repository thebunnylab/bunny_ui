//! The app's life OUTSIDE its window: what the platform tells it, and
//! what it may ask of the platform.
//!
//! A mail client wants to know the machine slept and woke (the mailbox
//! went stale), to say "a letter arrived" in the desktop's own
//! notification with a button on it, to hear which button was pressed,
//! and to be the ONE process — a second launch, a link from another
//! app, both land in the process already running. None of that is a
//! view, and none of it is the shell's alone: the doors are here, in
//! the app's own vocabulary, and each shell answers them with the
//! platform's own machinery.
//!
//! ```ignore
//! let (events, life) = task::channel::<AppEvent>();
//! app::subscribe(events);
//! if app::instance("trinity-mail", &arguments) == Instance::Secondary {
//!     return; // the running one heard the arguments
//! }
//! // …in a task:
//! while let Some(event) = life.recv().await {
//!     match event {
//!         AppEvent::DidWake => sync(),
//!         AppEvent::NotificationActivated { id, action } => open(id, action),
//!         AppEvent::Reopened { arguments } => route(arguments),
//!         AppEvent::WillSleep => {}
//!     }
//! }
//! ```
//!
//! Events ride the same channel discipline `.task` uses: the app owns
//! a channel, hands the sender here, and reads on its own thread — a
//! send wakes the frame, from whichever thread the platform speaks on.

use std::cell::RefCell;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::task::Sender;

/// What the platform tells the app about its life outside the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppEvent {
    /// The machine is going to sleep — save what must be saved. Not
    /// every platform announces it (a lid closed faster than the
    /// announcement); `DidWake` is the one to rely on.
    WillSleep,
    /// The machine woke, or the session resumed — what was current is
    /// stale now.
    DidWake,
    /// The person activated a notification: its `id`, and the `action`
    /// they chose — `None` is the notification itself, clicked.
    NotificationActivated { id: String, action: Option<String> },
    /// A second launch of this app reached this process: its
    /// arguments, as the second launch received them. A link from
    /// another app arrives this way on every platform — the url is
    /// an argument — and a bundled app on macOS reports a plain
    /// reopen (the Dock icon, clicked) with no arguments at all.
    Reopened { arguments: Vec<String> },
}

/// One button on a notification: what comes back (`key`) and what the
/// person reads (`label`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

/// A notification the desktop shows on the app's behalf. Posting one
/// again with the same `id` REPLACES it — a thread that grew by one
/// more letter is one notification, updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notification {
    /// The app's own name for it — what [`AppEvent::NotificationActivated`]
    /// carries back. A conversation's id, say.
    pub id: String,
    pub title: String,
    pub body: String,
    /// The buttons, in order. A platform that shows fewer shows the
    /// first ones.
    pub actions: Vec<NotificationAction>,
}

impl Notification {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Notification {
        Notification { id: id.into(), title: title.into(), body: body.into(), actions: Vec::new() }
    }

    /// One more button.
    pub fn action(mut self, key: impl Into<String>, label: impl Into<String>) -> Notification {
        self.actions.push(NotificationAction { key: key.into(), label: label.into() });
        self
    }
}

/// The senders the app handed over — every event goes to each of
/// them, from whichever thread the platform speaks on.
static SUBSCRIBERS: Mutex<Vec<Sender<AppEvent>>> = Mutex::new(Vec::new());

/// Subscribes a channel's sender to the app's life: from now on every
/// [`AppEvent`] is sent through it, and the receiver's `.recv()` in a
/// task wakes the frame the way any channel does. Subscribe BEFORE
/// [`instance`], so a launch that lands early is heard.
pub fn subscribe(sender: Sender<AppEvent>) {
    SUBSCRIBERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(sender);
}

/// The shell reports: one event, to every subscriber. A subscriber
/// whose receiver is gone is dropped here. Answers how many heard it —
/// an event nobody heard is a fact, not a failure.
pub fn emit(event: AppEvent) -> usize {
    let mut subscribers = SUBSCRIBERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    subscribers.retain(|sender| sender.send(event.clone()).is_ok());
    subscribers.len()
}

type Notifier = Box<dyn Fn(&Notification) -> Result<(), String>>;

thread_local! {
    /// The shell's own way to show a notification — installed at
    /// boot, on the thread the app runs on.
    static NOTIFIER: RefCell<Option<Notifier>> = const { RefCell::new(None) };
}

/// The shell installs how a notification is shown on this platform.
pub fn install_notifier(notifier: impl Fn(&Notification) -> Result<(), String> + 'static) {
    NOTIFIER.with(|slot| *slot.borrow_mut() = Some(Box::new(notifier)));
}

/// Shows `notification` in the desktop's own way. `Err` is the
/// platform's refusal by name — no shell running, an app the desktop
/// does not know (macOS shows notifications for a BUNDLE, never a bare
/// binary), a person who said no — and never a quiet nothing.
pub fn notify(notification: &Notification) -> Result<(), String> {
    NOTIFIER.with(|slot| match &*slot.borrow() {
        Some(notifier) => notifier(notification),
        None => Err(String::from("no shell is running on this thread to show a notification")),
    })
}

/// Which process this is — see [`instance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instance {
    /// The one: it holds the name, and hears every later launch as
    /// [`AppEvent::Reopened`].
    Primary,
    /// A later launch: its arguments reached the primary, and there
    /// is nothing left for this process to do but exit.
    Secondary,
}

/// The ONE process under `name`. The first to call holds a lock in
/// the person's own runtime directory for as long as it lives; every
/// later call from another process hands its `arguments` to the
/// holder — they arrive there as [`AppEvent::Reopened`] — and answers
/// `Secondary`, so the caller can exit. A holder that died releases
/// the lock with its last breath, and the next launch is the primary.
///
/// This is the app's half of a deep link on every platform: the OS
/// starts the app with the url as an argument, and the argument
/// crosses over. Registering the url scheme is the platform's own
/// configuration (the bundle's `Info.plist`, the registry, a
/// `.desktop` file) and stays the app's to write. On macOS a BUNDLED
/// app is already one process by the system's own rule, and its
/// reopens and urls arrive through the shell instead — the same
/// event, either road.
pub fn instance(name: &str, arguments: &[String]) -> Instance {
    instance_in(&runtime_dir(name), arguments)
}

/// Where the lock and the spool of a name live: the person's own
/// runtime directory, private to them.
fn runtime_dir(name: &str) -> PathBuf {
    let folder = format!("bunny-{name}");
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join(folder);
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join(folder);
    }
    std::env::temp_dir().join(folder)
}

/// The lock the primary holds — for the life of the process, which
/// is what releases it.
static HELD: OnceLock<File> = OnceLock::new();

/// The mechanism of [`instance`], on a directory of the caller's
/// choosing: `lock` inside it is the name, `spool/` the mailbox. The
/// primary clears the mailbox as it takes the name (a launch a dead
/// primary never read is not this one's to replay), then watches it
/// four times a second from a thread of its own.
pub fn instance_in(dir: &Path, arguments: &[String]) -> Instance {
    let spool = dir.join("spool");
    let _ = fs::create_dir_all(&spool);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"));
    let Ok(lock) = lock else {
        // the directory refused us: nobody can hold the name here, so
        // every launch is the primary — one process is better than none
        return Instance::Primary;
    };
    if lock.try_lock().is_ok() {
        if HELD.set(lock).is_err() {
            // this process already holds a name: the lock just taken
            // stays held by the file the OnceLock keeps
        }
        clear_spool(&spool);
        std::thread::spawn(move || {
            loop {
                for arguments in take_spool(&spool) {
                    emit(AppEvent::Reopened { arguments });
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        return Instance::Primary;
    }
    post_spool(&spool, arguments);
    Instance::Secondary
}

/// One launch's arguments, as a file: the arguments NUL-separated,
/// written whole and then named into place — the watcher never reads
/// half a launch.
fn post_spool(spool: &Path, arguments: &[String]) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let name = format!("{stamp}-{}", std::process::id());
    let draft = spool.join(format!("{name}.draft"));
    let posted = spool.join(format!("{name}.args"));
    let written = File::create(&draft)
        .and_then(|mut file| file.write_all(arguments.join("\0").as_bytes()));
    if written.is_ok() {
        let _ = fs::rename(&draft, &posted);
    }
}

/// Every launch posted so far, oldest first, taken out of the spool.
fn take_spool(spool: &Path) -> Vec<Vec<String>> {
    let Ok(entries) = fs::read_dir(spool) else {
        return Vec::new();
    };
    let mut posted: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "args"))
        .collect();
    posted.sort();
    posted
        .into_iter()
        .filter_map(|path| {
            let text = fs::read_to_string(&path).ok();
            let _ = fs::remove_file(&path);
            text
        })
        .map(|text| {
            if text.is_empty() {
                Vec::new()
            } else {
                text.split('\0').map(str::to_string).collect()
            }
        })
        .collect()
}

fn clear_spool(spool: &Path) {
    let _ = take_spool(spool);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A subscriber hears what the shell emits; one whose receiver is
    /// gone is dropped on the next emit, not kept as a quiet listener.
    #[test]
    fn a_subscriber_hears_the_app_live() {
        let (sender, receiver) = crate::task::channel::<AppEvent>();
        let (gone, dropped) = crate::task::channel::<AppEvent>();
        subscribe(sender);
        subscribe(gone);
        drop(dropped);
        let heard = emit(AppEvent::DidWake);
        assert!(heard >= 1, "the live subscriber counted");
        // the receiver reads it on its own turn
        let mut got = None;
        let mut task = std::pin::pin!(async { receiver.recv().await });
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        if let std::task::Poll::Ready(event) = task.as_mut().poll(&mut context) {
            got = event;
        }
        assert_eq!(got, Some(AppEvent::DidWake));
    }

    /// Without a shell a notification is refused by name, never
    /// dropped; with one installed, the shell's answer is the answer.
    #[test]
    fn a_notification_is_refused_by_name_without_a_shell() {
        let letter = Notification::new("thread-7", "Ada", "Could you send the figures?")
            .action("reply", "Reply")
            .action("archive", "Archive");
        assert_eq!(letter.actions.len(), 2);
        assert!(notify(&letter).unwrap_err().contains("no shell"));
        install_notifier(|notification| {
            if notification.title.is_empty() { Err(String::from("no title")) } else { Ok(()) }
        });
        assert_eq!(notify(&letter), Ok(()));
        assert_eq!(notify(&Notification::new("x", "", "")), Err(String::from("no title")));
    }

    /// The first caller holds the name; a later launch's arguments
    /// cross the spool whole, in order, and the mailbox empties.
    #[test]
    fn the_second_launch_reaches_the_first() {
        let dir = std::env::temp_dir().join(format!("bunny-instance-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (sender, receiver) = crate::task::channel::<AppEvent>();
        subscribe(sender);
        assert_eq!(instance_in(&dir, &[String::from("first")]), Instance::Primary);
        // the same process asking again is a second launch as far as
        // the lock is concerned: the file is held, the arguments cross
        let arguments = vec![String::from("mail://thread/7"), String::from("with\nnewline")];
        assert_eq!(instance_in(&dir, &arguments), Instance::Secondary);
        let mut heard = None;
        for _ in 0..40 {
            let mut task = std::pin::pin!(async { receiver.recv().await });
            let waker = std::task::Waker::noop();
            let mut context = std::task::Context::from_waker(waker);
            if let std::task::Poll::Ready(event) = task.as_mut().poll(&mut context) {
                heard = event;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(heard, Some(AppEvent::Reopened { arguments }));
        assert!(take_spool(&dir.join("spool")).is_empty(), "the mailbox was emptied");
        // an empty launch is an empty list, not one empty argument
        post_spool(&dir.join("spool"), &[]);
        assert_eq!(take_spool(&dir.join("spool")), vec![Vec::<String>::new()]);
        let _ = fs::remove_dir_all(&dir);
    }
}
