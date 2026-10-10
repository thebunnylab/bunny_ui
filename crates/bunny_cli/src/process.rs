//! Other programs: found on the PATH, run with a deadline, their output
//! captured — a tool that hangs (an emulator that never answers, a
//! simulator service waking up) costs a timeout, never the session.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut text);
        }
        text
    });
    let err = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut text);
        }
        text
    });
    let started = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code();
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(15));
    };
    let stdout = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    Ok(Output { code, stdout, stderr })
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
        let out = run("sh", &["-c", "sleep 30"], Duration::from_millis(200)).unwrap();
        assert_eq!(out.code, None);
        assert!(started.elapsed() < Duration::from_secs(5));
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
