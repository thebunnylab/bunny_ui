//! Other programs: found on the PATH, run with a deadline, their output
//! captured — a tool that hangs (an emulator that never answers, a
//! simulator service waking up) costs a timeout, never the session.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// What a finished program said.
#[derive(Debug)]
pub struct Output {
    /// `None` when it was killed at the deadline.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// Runs `program` with `args`, its stdin empty, and kills it when it
/// outlives `timeout`. `Err` is a program that could not start at all —
/// most often one that is not installed.
pub fn run<S: AsRef<OsStr>>(program: impl AsRef<OsStr>, args: &[S], timeout: Duration) -> std::io::Result<Output> {
    run_in(program, args, None, &[], timeout)
}

/// [`run`] in a folder, with extra environment.
pub fn run_in<S: AsRef<OsStr>>(
    program: impl AsRef<OsStr>,
    args: &[S],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
    timeout: Duration,
) -> std::io::Result<Output> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn()?;
    // both pipes drain on threads of their own: a program that fills one
    // while we wait on the other would never finish
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let started = Instant::now();
    let mut killed = false;
    let code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code();
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            killed = true;
            break None;
        }
        thread::sleep(Duration::from_millis(15));
    };
    // The program is gone, but a process it started can still hold the
    // pipes open — a daemon it launched (adb's server), a grandchild a
    // kill did not reach (dash runs `sleep` as a child). Its output is
    // not this program's: the readers get a moment to finish, never more.
    let grace = if killed { Duration::from_millis(100) } else { Duration::from_secs(2) };
    let until = Instant::now() + grace;
    let stdout = stdout.take(until);
    let stderr = stderr.take(until);
    Ok(Output { code, stdout, stderr })
}

/// A pipe read on a thread of its own, into a buffer shared with the
/// caller — so the caller can stop waiting while the pipe is still open.
struct Drain {
    buffer: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Drain {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let (finished, done) = mpsc::channel();
    let shared = Arc::clone(&buffer);
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = [0u8; 8192];
            while let Ok(length @ 1..) = pipe.read(&mut chunk) {
                shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).extend_from_slice(&chunk[..length]);
            }
        }
        let _ = finished.send(());
    });
    Drain { buffer, done }
}

impl Drain {
    /// What was read by the time the pipe closed, or by `until` if it
    /// stays open — the reader is left to finish on its own.
    fn take(self, until: Instant) -> String {
        let _ = self.done.recv_timeout(until.saturating_duration_since(Instant::now()));
        let bytes = std::mem::take(&mut *self.buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// The program's full path on the PATH, the way a shell would find it
/// (with the Windows extensions on Windows).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| String::from(".COM;.EXE;.BAT;.CMD"))
            .split(';')
            .map(|ext| ext.to_ascii_lowercase())
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// A file that can be run.
pub fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else { return false };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn output_is_captured_and_a_hang_is_cut() {
        let out = run("sh", &["-c", "echo out; echo err >&2; exit 3"], Duration::from_secs(5)).unwrap();
        assert_eq!((out.code, out.stdout.as_str(), out.stderr.as_str()), (Some(3), "out\n", "err\n"));
        let started = Instant::now();
        // `; true` keeps `sh` from replacing itself with `sleep` (dash never
        // does): the grandchild outlives the kill and holds the pipes open
        let out = run("sh", &["-c", "sleep 30; true"], Duration::from_millis(200)).unwrap();
        assert_eq!(out.code, None);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A program that exits leaving a daemon on its pipes (`adb` starting
    /// its server) answers with what it printed, not when the daemon dies.
    #[cfg(unix)]
    #[test]
    fn a_daemon_on_the_pipes_does_not_hold_the_answer() {
        let started = Instant::now();
        let out = run("sh", &["-c", "sleep 30 & echo started"], Duration::from_secs(20)).unwrap();
        assert_eq!((out.code, out.stdout.as_str()), (Some(0), "started\n"));
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_hang() {
        assert!(run("bunny-no-such-program", &["x"], Duration::from_secs(1)).is_err());
        assert!(which("bunny-no-such-program").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn which_finds_a_shell() {
        assert!(which("sh").is_some_and(|path| path.is_absolute()));
    }
}
