//! The MSVC build tools a Windows build links with.

use std::path::PathBuf;

use super::QUICK;
use crate::process;

/// The Visual Studio (or Build Tools) installation with the C++ tools,
/// as `vswhere` reports it.
pub fn msvc() -> Option<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles(x86)")?;
    let vswhere = PathBuf::from(program_files).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let out = process::run(
        &vswhere,
        &[
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ],
        QUICK,
    )
    .ok()
    .filter(process::Output::ok)?;
    let path = out.stdout.lines().next()?.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}
