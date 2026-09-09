//! The present tape. `BUNNY_PRESENT_TRACE=1` writes one file per
//! process — `/tmp/bunny-present.<pid>.trace`, truncated on start, so
//! two processes never interleave on one tape. Any other value is used
//! as the path, with a literal `{pid}` replaced by the process id.
//! `BUNNY_TRACE_TAG` stamps the header with free text (a build or an
//! experiment name). Off, each mark costs one branch.
//!
//! One event per line. Times are milliseconds from the first mark of
//! the process:
//!
//! ```text
//! # bunny-trace v2 pid=<pid> t0=<unix_ms> tag=<tag>
//! R <ms> <w>x<h> kind=<resize|move|backing> live=<0|1>
//! P <ms> <w>x<h> live=<0|1> cmds=<n> via=<origin>
//! H <ms> dur=<ms> hosts=<n>
//! O <ms> dur=<ms> panels=<n>
//! M <ms> dur=<ms> sync=<0|1>
//! S <ms> dur=<ms> n=<alive> raster=<n> px=<n>
//! E <ms> dur=<ms>
//! X <ms> what=<name>
//! ```
//!
//! `R` is a window callback (which notification asked, and at what
//! size). `P` opens a present; `H` (host pass), `O` (overlay panels),
//! `M` (scene presented, `sync` = inside the resize transaction) and
//! `S` (segments: mounted, rasterized, pixels) each carry the time
//! since the previous mark of the same present; `E` closes it with the
//! total. `X` names a one-time cost (`sync-on`, `sync-off`,
//! `buffer-grow`, `atlas-drain`, `segment-class`). `via` names the
//! code path that asked for the present: `redraw` (a window callback),
//! `wake` (a worker), `input` (an event), `frame` (the animation
//! tick), `web` (a page report), `blink` (the slow clock).

use std::io::Write as _;

/// The code path that asked for a present.
#[derive(Clone, Copy)]
pub enum Origin {
    Redraw,
    Wake,
    Input,
    Frame,
    Web,
    Blink,
}

impl Origin {
    fn name(self) -> &'static str {
        match self {
            Origin::Redraw => "redraw",
            Origin::Wake => "wake",
            Origin::Input => "input",
            Origin::Frame => "frame",
            Origin::Web => "web",
            Origin::Blink => "blink",
        }
    }
}

/// The tape, opened once — truncated, headed, and kept. Opening
/// per line was measurable inside the present it was measuring.
fn out() -> Option<&'static std::sync::Mutex<std::fs::File>> {
    static OUT: std::sync::OnceLock<Option<std::sync::Mutex<std::fs::File>>> =
        std::sync::OnceLock::new();
    OUT.get_or_init(|| {
        let value = std::env::var("BUNNY_PRESENT_TRACE").ok()?;
        let pid = std::process::id();
        let path = if value == "1" || value.is_empty() {
            format!("/tmp/bunny-present.{pid}.trace")
        } else {
            value.replace("{pid}", &pid.to_string())
        };
        let mut file = std::fs::File::create(path).ok()?;
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |t| t.as_millis());
        let tag = std::env::var("BUNNY_TRACE_TAG").unwrap_or_default();
        let _ = writeln!(file, "# bunny-trace v2 pid={pid} t0={t0} tag={tag}");
        Some(std::sync::Mutex::new(file))
    })
    .as_ref()
}

/// True when the tape is on — the gate a caller checks before
/// paying for anything a mark would need.
pub fn active() -> bool {
    out().is_some()
}

fn ms() -> f64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64() * 1000.0
}

fn line(args: std::fmt::Arguments<'_>) {
    if let Some(file) = out()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{args}");
    }
}

/// One line outside a present — the window callbacks (`R`) and the
/// one-time costs (`X`).
pub fn mark(kind: &str, args: std::fmt::Arguments<'_>) {
    if !active() {
        return;
    }
    line(format_args!("{kind} {:.1} {args}", ms()));
}

/// The marks of one present: `P` on begin, one line per stage, and
/// `E` with the total on drop — so every exit answers with its
/// duration.
pub struct Traced(Option<Stages>);

struct Stages {
    start: std::time::Instant,
    last: std::time::Instant,
}

impl Traced {
    /// Closes one stage: the line carries the time since the
    /// previous mark of this present.
    pub fn stage(&mut self, kind: &str, args: std::fmt::Arguments<'_>) {
        if let Some(stages) = &mut self.0 {
            let now = std::time::Instant::now();
            let dur = now.duration_since(stages.last).as_secs_f64() * 1000.0;
            line(format_args!("{kind} {:.1} dur={dur:.1} {args}", ms()));
            stages.last = now;
        }
    }
}

impl Drop for Traced {
    fn drop(&mut self) {
        if let Some(stages) = &self.0 {
            line(format_args!(
                "E {:.1} dur={:.1}",
                ms(),
                stages.start.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }
}

pub fn begin(w: f64, h: f64, live: bool, cmds: usize, via: Origin) -> Traced {
    if !active() {
        return Traced(None);
    }
    line(format_args!(
        "P {:.1} {w:.0}x{h:.0} live={} cmds={cmds} via={}",
        ms(),
        u8::from(live),
        via.name()
    ));
    let now = std::time::Instant::now();
    Traced(Some(Stages { start: now, last: now }))
}
