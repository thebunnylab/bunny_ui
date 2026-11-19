//! Random bytes from the operating system, for a new key.
//!
//! Only key generation needs them: Ed25519 signs and verifies with no
//! random input. The standard library gives no access to the system
//! generator, and this crate has no unsafe code, so the bytes come
//! from a file or a tool the system already has:
//!
//! - macOS and Linux: `/dev/urandom`, the kernel generator.
//! - Windows: `RandomNumberGenerator` of .NET, through PowerShell, which
//!   every Windows 10 and later has.

use std::io;

/// Fills `out` with random bytes from the system generator.
///
/// # Errors
///
/// An error when the system generator cannot be read.
pub fn fill(out: &mut [u8]) -> io::Result<()> {
    system_fill(out)
}

#[cfg(unix)]
fn system_fill(out: &mut [u8]) -> io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(out)
}

#[cfg(windows)]
fn system_fill(out: &mut [u8]) -> io::Result<()> {
    let script = format!(
        "[Convert]::ToBase64String([Security.Cryptography.RandomNumberGenerator]::GetBytes({}))",
        out.len()
    );
    let run = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if !run.status.success() {
        return Err(io::Error::other("powershell did not give random bytes"));
    }
    let text = String::from_utf8_lossy(&run.stdout);
    let bytes = crate::base64::decode(text.trim())
        .ok_or_else(|| io::Error::other("powershell gave text that is not base64"))?;
    if bytes.len() != out.len() {
        return Err(io::Error::other("powershell gave the wrong number of random bytes"));
    }
    out.copy_from_slice(&bytes);
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn system_fill(_out: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no system random generator on this target"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_fills_are_not_the_same() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        fill(&mut a).unwrap();
        fill(&mut b).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
    }
}
