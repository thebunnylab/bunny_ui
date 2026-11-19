//! The pure half of the Windows install.
//!
//! A running image cannot be replaced on Windows. So the app copies its
//! own executable to the temp folder, starts the copy as a helper with
//! [`HELPER_FLAG`], and quits. The helper waits until every image of the
//! install is free, puts the new build in place and starts it.
//!
//! The helper waits for **every** image, not only the app's: an
//! installer that meets a held file half-succeeds, a new app beside an old
//! sidecar. [`HelperPlan::sidecars`] names the other executables of the
//! package.

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bunny_ui_release::{Format, UpdateError};

/// The first argument that turns a launch into the update helper.
///
/// One constant, because the strategy starts the helper and the app's
/// start reads the flag: a typo would make a helper that opens the app
/// again, and the update would not happen.
pub const HELPER_FLAG: &str = "--bunny-apply-update";

/// How long the helper waits for the images to be free. On timeout it
/// leaves the artifact in place and does nothing.
pub const HELPER_DEADLINE_SECS: u64 = 60;

const FIRST_PAUSE: Duration = Duration::from_millis(50);
const LONGEST_PAUSE: Duration = Duration::from_millis(500);

/// Which formats Windows installs: an NSIS or WiX installer, or a zip
/// of the app's folder.
///
/// # Errors
///
/// [`UpdateError::Apply`] that names the format.
pub fn accepts(format: Format) -> Result<(), UpdateError> {
    match format {
        Format::Nsis | Format::Wix | Format::Zip => Ok(()),
        other => Err(UpdateError::Apply(format!("windows cannot install a {other:?} artifact"))),
    }
}

/// What the helper must know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperPlan {
    /// The artifact: an installer to run, or a zip to unpack.
    pub artifact: PathBuf,
    /// The app's executable: waited on, and started after.
    pub target: PathBuf,
    /// The folder the build goes into.
    pub install_dir: PathBuf,
    pub format: Format,
    /// The other executables of the package, each waited on.
    pub sidecars: Vec<PathBuf>,
}

impl HelperPlan {
    /// The plan of an update of `running` by `artifact`, with the
    /// sidecars' file stems.
    ///
    /// # Errors
    ///
    /// [`UpdateError::Apply`] for a format Windows does not install, and
    /// [`UpdateError::NotBundled`] when `running` has no folder.
    pub fn derive(running: &Path, artifact: &Path, format: Format, sidecars: &[String]) -> Result<HelperPlan, UpdateError> {
        accepts(format)?;
        let install_dir = running.parent().ok_or(UpdateError::NotBundled)?.to_path_buf();
        let sidecars = sidecars.iter().map(|stem| install_dir.join(format!("{stem}.exe"))).collect();
        Ok(HelperPlan { artifact: artifact.to_path_buf(), target: running.to_path_buf(), install_dir, format, sidecars })
    }

    /// Every image the update replaces: the app's first, then each sidecar.
    pub fn images(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.target.as_path()).chain(self.sidecars.iter().map(PathBuf::as_path))
    }
}

/// The arguments of the helper: the flag first, so the app's start can
/// read it before anything else; the sidecars last, the one part of any
/// length.
#[must_use]
pub fn helper_argv(plan: &HelperPlan) -> Vec<OsString> {
    [
        OsString::from(HELPER_FLAG),
        plan.artifact.clone().into_os_string(),
        plan.target.clone().into_os_string(),
        plan.install_dir.clone().into_os_string(),
        OsString::from(format_token(plan.format)),
    ]
    .into_iter()
    .chain(plan.sidecars.iter().map(|sidecar| sidecar.clone().into_os_string()))
    .collect()
}

/// What [`helper_argv`] wrote, read back from a full argument list (the
/// program first). `None` for an ordinary launch.
pub fn parse_helper_args<I: IntoIterator<Item = OsString>>(args: I) -> Option<HelperPlan> {
    let mut args = args.into_iter().skip(1);
    if args.next()? != HELPER_FLAG {
        return None;
    }
    let artifact = PathBuf::from(args.next()?);
    let target = PathBuf::from(args.next()?);
    let install_dir = PathBuf::from(args.next()?);
    let format = parse_format_token(args.next()?.to_str()?)?;
    let sidecars = args.map(PathBuf::from).collect();
    Some(HelperPlan { artifact, target, install_dir, format, sidecars })
}

const fn format_token(format: Format) -> &'static str {
    match format {
        Format::Wix => "wix",
        Format::Zip => "zip",
        // everything here passed `accepts`
        _ => "nsis",
    }
}

fn parse_format_token(raw: &str) -> Option<Format> {
    match raw {
        "nsis" => Some(Format::Nsis),
        "wix" => Some(Format::Wix),
        "zip" => Some(Format::Zip),
        _ => None,
    }
}

/// What an exclusive open of an image found.
#[derive(Debug)]
pub enum ImageState {
    /// Nothing holds it.
    Free,
    /// It is not there: the update creates it, so nothing waits on it.
    Absent,
    /// Any other failure: a process runs it, or the folder is not ours to
    /// write. When the probe cannot tell, the update does not run.
    Busy(io::Error),
}

impl From<io::Result<File>> for ImageState {
    fn from(opened: io::Result<File>) -> ImageState {
        match opened {
            Ok(_) => ImageState::Free,
            Err(error) if error.kind() == io::ErrorKind::NotFound => ImageState::Absent,
            Err(error) => ImageState::Busy(error),
        }
    }
}

/// The helper's clock, so a test can replay the whole wait.
pub trait Pacer {
    fn elapsed(&self) -> Duration;
    fn pause(&mut self, span: Duration);
}

/// A plan whose images were all free in one round. Only [`await_images`]
/// makes one, so nothing can install over a held file by mistake.
#[derive(Debug)]
pub struct Unlocked<'p> {
    plan: &'p HelperPlan,
}

impl<'p> Unlocked<'p> {
    #[must_use]
    pub const fn plan(&self) -> &'p HelperPlan {
        self.plan
    }
}

/// An image still held at the deadline, and why.
#[derive(Debug)]
pub struct BusyImage {
    pub path: PathBuf,
    pub cause: io::Error,
}

/// The deadline passed with an image still held. Nothing was installed,
/// and the artifact stays where it was.
#[derive(Debug)]
pub struct StillBusy {
    busy: Vec<BusyImage>,
    waited: Duration,
    artifact: PathBuf,
}

impl StillBusy {
    /// The images held in the last round: never empty.
    #[must_use]
    pub fn busy(&self) -> &[BusyImage] {
        &self.busy
    }
}

impl fmt::Display for StillBusy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.busy.len() == 1 { "is" } else { "are" };
        for (i, image) in self.busy.iter().enumerate() {
            let separator = if i == 0 { "" } else { ", " };
            write!(f, "{separator}{} ({})", image.path.display(), image.cause)?;
        }
        write!(
            f,
            " {verb} still in use after {}s; the update stays at {}",
            self.waited.as_secs(),
            self.artifact.display()
        )
    }
}

impl std::error::Error for StillBusy {}

/// Probes every image until all are free in the same round, or the
/// deadline passes. Each round probes all of them, so a timeout names
/// each holder. The pause starts at 50 ms and doubles to 500 ms.
///
/// # Errors
///
/// [`StillBusy`] that names the images held in the last round.
pub fn await_images<'p>(
    plan: &'p HelperPlan,
    deadline: Duration,
    mut probe: impl FnMut(&Path) -> ImageState,
    pacer: &mut impl Pacer,
) -> Result<Unlocked<'p>, StillBusy> {
    let mut pause = FIRST_PAUSE;
    loop {
        let busy: Vec<BusyImage> = plan
            .images()
            .filter_map(|image| match probe(image) {
                ImageState::Busy(cause) => Some(BusyImage { path: image.to_path_buf(), cause }),
                ImageState::Free | ImageState::Absent => None,
            })
            .collect();
        if busy.is_empty() {
            return Ok(Unlocked { plan });
        }
        if pacer.elapsed() >= deadline {
            return Err(StillBusy { busy, waited: pacer.elapsed(), artifact: plan.artifact.clone() });
        }
        pacer.pause(pause);
        pause = (pause * 2).min(LONGEST_PAUSE);
    }
}

/// One argument of an installer, and whether it can be quoted.
///
/// NSIS reads its `/D=` to the end of the line, as it is: a quoted path
/// under `C:\Users\Some Name\…` would install into `C:\Users\Some`. So
/// `/D=` is raw, and last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    Quoted(String),
    Raw(String),
}

/// What runs the installer of a plan whose images were free: the program
/// and its arguments. `None` for a zip, which the helper unpacks itself.
#[must_use]
pub fn installer_command(unlocked: &Unlocked<'_>) -> Option<(PathBuf, Vec<Arg>)> {
    let plan = unlocked.plan;
    match plan.format {
        Format::Wix => Some((
            PathBuf::from("msiexec"),
            vec![
                Arg::Quoted("/i".into()),
                Arg::Quoted(plan.artifact.to_string_lossy().into_owned()),
                Arg::Quoted("/qn".into()),
                Arg::Quoted("/norestart".into()),
            ],
        )),
        Format::Zip => None,
        _ => Some((
            plan.artifact.clone(),
            vec![Arg::Quoted("/S".into()), Arg::Raw(format!("/D={}", plan.install_dir.to_string_lossy()))],
        )),
    }
}

/// Where the helper copy lives: outside the install folder, which the
/// update replaces.
#[must_use]
pub fn helper_path(temp_dir: &Path, product: &str, pid: u32, version: &str) -> PathBuf {
    temp_dir.join(format!("{product}-update-helper-{pid}-{version}.exe"))
}

/// Whether a temp file is an old helper copy of `product`, to delete.
///
/// A program cannot delete its own running image on Windows without a
/// Win32 call, so the helper leaves its copy, and the next download
/// sweeps it. An hour is longer than any install.
#[must_use]
pub fn is_sweepable_helper(name: &str, product: &str, age_secs: u64) -> bool {
    let is_exe = Path::new(name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    name.starts_with(&format!("{product}-update-helper-")) && is_exe && age_secs > 3600
}

/// The files of a zip update, each with its path inside the install
/// folder: the one top folder of the archive (`App/` of `App/App.exe`)
/// is taken off, and folders drop out.
///
/// # Errors
///
/// [`UpdateError::Apply`] when an entry could land outside the folder,
/// or when the archive does not hold `exe` where the app runs it.
pub fn zip_files(
    items: Vec<bunny_ui_formats::zip::Item>,
    exe: &str,
) -> Result<Vec<(PathBuf, Vec<u8>)>, UpdateError> {
    let names: Vec<String> = items.iter().map(|item| item.path.clone()).collect();
    let top = super::linux::common_top(&names);
    let mut files = Vec::new();
    for item in items {
        let path = match &top {
            Some(top) => item.path.strip_prefix(top.as_str()).map(|rest| rest.trim_start_matches('/')).unwrap_or(""),
            None => item.path.as_str(),
        };
        let Some(bytes) = item.bytes else { continue };
        if path.is_empty() {
            continue;
        }
        let relative = bunny_ui_formats::paths::contained(path)
            .ok_or_else(|| UpdateError::Apply(format!("the archive entry {:?} leaves the install folder", item.path)))?;
        files.push((relative, bytes));
    }
    let has_exe = files.iter().any(|(path, _)| path.as_os_str().eq_ignore_ascii_case(exe));
    if !has_exe {
        return Err(UpdateError::Apply(format!("the archive has no {exe} at its top")));
    }
    Ok(files)
}
