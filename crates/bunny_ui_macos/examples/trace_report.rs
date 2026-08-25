//! The tape reader for the present trace.
//!
//! `BUNNY_PRESENT_TRACE=1` makes the shell write a tape: one line per
//! present event. This example reads a tape back and turns it into a
//! report. It opens no window — it is a plain command line tool, and
//! it must never panic on a bad tape: a truncated line, an unknown
//! tag, or an interleaved write is skipped and counted, never fatal.
//!
//! Usage:
//! ```text
//! cargo run -p bunny-ui-macos --example trace_report -- <path> [--json]
//! ```
//!
//! The tape (v2), one line per event; `<ms>` counts from the first
//! mark of the process, one decimal place:
//! ```text
//! # bunny-trace v2 pid=<pid> t0=<unix_ms> tag=<free text>
//! R <ms> <w>x<h> kind=<resize|move|backing> live=<0|1>
//! P <ms> <w>x<h> live=<0|1> cmds=<n> via=<redraw|wake|input|frame|web|blink>
//! H <ms> dur=<ms> hosts=<n>
//! O <ms> dur=<ms> panels=<n>
//! M <ms> dur=<ms> sync=<0|1>
//! S <ms> dur=<ms> n=<n_alive> raster=<n> px=<n>
//! E <ms> dur=<ms>
//! X <ms> what=<one-time cost>[ bytes=<n>]
//! ```
//! A `P` opens a present; the `H`/`O`/`M`/`S` marks that follow
//! belong to it; `E` closes it with the total time. `R` is the OS
//! callback that asked for the present — a resize step may write two
//! or three of them, and counting that is one goal of this report. A
//! v1 tape still reads: no header, and a `P` without `via` gets `"?"`.
//!
//! What the numbers catch:
//! - a *gesture* is a maximal run of live presents less than 300 ms
//!   apart. The first gesture of the tape is the cold one; the rest
//!   are warm. A wake present mid-drag is live too — live is the
//!   window's state, not the event's origin — and that imposter is
//!   exactly what the gesture metrics hunt;
//! - `dup rate` counts live presents that repainted the SAME size as
//!   the step before. A second presenter shows itself here, and the
//!   via histogram of the duplicate lines names it;
//! - `reversals` counts size steps that moved against the previous
//!   step — presents landing out of order;
//! - `redraw/resize` above 1.0 means more redraw presents than
//!   resize callbacks: a double dispatch;
//! - a *rest* is a stretch outside gestures, trimmed 1 s at each
//!   border and kept only if 5 s remain: who presents while nothing
//!   happens, and on what beat (`dt modes`);
//! - `cold ratio` is gesture 1's worst stage time over the warm
//!   median — the price of the first time, stage by stage.
//!
//! Percentiles use the nearest-rank method. `--json` prints the same
//! content as one JSON object with stable keys; JSON numbers are
//! rounded to 3 decimal places.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::process;

/// A gap of 300 ms or more between live presents splits two gestures.
const GESTURE_SPLIT_MS: f64 = 300.0;
/// A gap over 50 ms between live steps counts as starvation.
const STARVED_GAP_MS: f64 = 50.0;
/// Rest borders are trimmed by this much on each side.
const REST_TRIM_MS: f64 = 1000.0;
/// A trimmed rest below this length is dropped.
const REST_MIN_MS: f64 = 5000.0;
/// One frame at 120 Hz; a present over this overran the beat.
const FRAME_MS: f64 = 8.3;
/// Two frames at 120 Hz (one at 60 Hz).
const TWO_FRAMES_MS: f64 = 16.7;

/// The stage letters, in tape order. H hosts, O panels, M sync,
/// S segments, E the whole present.
const STAGES: [char; 5] = ['H', 'O', 'M', 'S', 'E'];

// ------------------------------------------------------------------
// The tape model
// ------------------------------------------------------------------

/// The `#` line at the top of a v2 tape.
#[derive(Debug)]
struct Header {
    version: String,
    pid: u64,
    t0: u64,
    tag: String,
}

/// An `R` line: the OS callback that asked for a present.
#[derive(Debug)]
struct Resize {
    ms: f64,
    kind: String,
}

/// The `S` mark of a present: the second surfaces.
#[derive(Debug)]
struct SegMark {
    dur: f64,
    alive: u64,
    raster: u64,
    px: u64,
}

/// A `P` line and the stage marks that followed it.
#[derive(Debug)]
struct Present {
    ms: f64,
    w: u32,
    h: u32,
    live: bool,
    via: String,
    hosts: Option<f64>,
    panels: Option<f64>,
    sync: Option<f64>,
    seg: Option<SegMark>,
    total: Option<f64>,
}

/// An `X` line: a one-time cost.
#[derive(Debug)]
struct OneTime {
    ms: f64,
    what: String,
    bytes: Option<u64>,
}

/// Everything one pass over the file collected.
#[derive(Debug, Default)]
struct Tape {
    header: Option<Header>,
    presents: Vec<Present>,
    resizes: Vec<Resize>,
    one_time: Vec<OneTime>,
    ignored: usize,
    first_ms: Option<f64>,
    last_ms: Option<f64>,
}

impl Tape {
    fn touch(&mut self, ms: f64) {
        self.first_ms = Some(self.first_ms.map_or(ms, |first| first.min(ms)));
        self.last_ms = Some(self.last_ms.map_or(ms, |last| last.max(ms)));
    }
}

// ------------------------------------------------------------------
// Parsing — skip what does not parse, never panic
// ------------------------------------------------------------------

fn parse_size(token: &str) -> Option<(u32, u32)> {
    let (w, h) = token.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

fn parse_header(body: &str) -> Option<Header> {
    let rest = body.trim_start().strip_prefix("bunny-trace")?;
    let mut tokens = rest.split_whitespace();
    let version = tokens.next().unwrap_or("?").to_string();
    let keyed: BTreeMap<&str, &str> = tokens.filter_map(|t| t.split_once('=')).collect();
    let number = |key: &str| keyed.get(key).and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(Header {
        pid: number("pid"),
        t0: number("t0"),
        // the tag is free text and may hold spaces: take the whole rest
        tag: rest.split_once("tag=").map_or(String::new(), |(_, tag)| tag.trim().to_string()),
        version,
    })
}

/// One data line. Returns false when the line must be ignored.
fn parse_record(tape: &mut Tape, open: &mut Option<usize>, line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    let Some(tag) = tokens.next() else { return false };
    let Some(ms) = tokens.next().and_then(|t| t.parse::<f64>().ok()) else {
        return false;
    };
    let rest: Vec<&str> = tokens.collect();
    let keyed: BTreeMap<&str, &str> = rest.iter().filter_map(|t| t.split_once('=')).collect();
    let dur = || keyed.get("dur").and_then(|v| v.parse::<f64>().ok());
    let count = |key: &str| keyed.get(key).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    match tag {
        "R" => {
            if rest.first().and_then(|t| parse_size(t)).is_none() {
                return false;
            }
            tape.resizes.push(Resize {
                ms,
                kind: keyed.get("kind").unwrap_or(&"?").to_string(),
            });
        }
        "P" => {
            let Some((w, h)) = rest.first().and_then(|t| parse_size(t)) else {
                return false;
            };
            tape.presents.push(Present {
                ms,
                w,
                h,
                live: keyed.get("live") == Some(&"1"),
                // a v1 line has no via — "?" keeps it countable
                via: keyed.get("via").unwrap_or(&"?").to_string(),
                hosts: None,
                panels: None,
                sync: None,
                seg: None,
                total: None,
            });
            // a P before the last E means a truncated present: the
            // old one keeps what it had, the new one takes over
            *open = Some(tape.presents.len() - 1);
        }
        "H" | "O" | "M" => {
            let (Some(dur), Some(index)) = (dur(), *open) else { return false };
            let present = &mut tape.presents[index];
            match tag {
                "H" => present.hosts = Some(dur),
                "O" => present.panels = Some(dur),
                _ => present.sync = Some(dur),
            }
        }
        "S" => {
            let (Some(dur), Some(index)) = (dur(), *open) else { return false };
            tape.presents[index].seg = Some(SegMark {
                dur,
                alive: count("n"),
                raster: count("raster"),
                px: count("px"),
            });
        }
        "E" => {
            let (Some(dur), Some(index)) = (dur(), open.take()) else { return false };
            tape.presents[index].total = Some(dur);
        }
        "X" => {
            let Some(what) = keyed.get("what") else { return false };
            tape.one_time.push(OneTime {
                ms,
                what: what.to_string(),
                bytes: keyed.get("bytes").and_then(|v| v.parse().ok()),
            });
        }
        _ => return false,
    }
    tape.touch(ms);
    true
}

fn parse_tape(text: &str) -> Tape {
    let mut tape = Tape::default();
    let mut open: Option<usize> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(body) = line.strip_prefix('#') {
            if tape.header.is_none()
                && let Some(header) = parse_header(body)
            {
                tape.header = Some(header);
            } else {
                tape.ignored += 1;
            }
            continue;
        }
        if !parse_record(&mut tape, &mut open, line) {
            tape.ignored += 1;
        }
    }
    tape
}

// ------------------------------------------------------------------
// Analysis
// ------------------------------------------------------------------

/// Nearest-rank percentile over a sorted, non-empty slice.
fn percentile(sorted: &[f64], q: f64) -> f64 {
    let rank = (q / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[derive(Debug)]
struct Dist {
    p50: f64,
    p95: f64,
    max: f64,
}

fn dist(mut values: Vec<f64>) -> Option<Dist> {
    values.sort_by(f64::total_cmp);
    let max = *values.last()?;
    Some(Dist {
        p50: percentile(&values, 50.0),
        p95: percentile(&values, 95.0),
        max,
    })
}

fn stage_dur(present: &Present, stage: char) -> Option<f64> {
    match stage {
        'H' => present.hosts,
        'O' => present.panels,
        'M' => present.sync,
        'S' => present.seg.as_ref().map(|seg| seg.dur),
        'E' => present.total,
        _ => None,
    }
}

/// Sizes that stepped against the previous step, per axis, summed.
fn reversals(sizes: &[(u32, u32)]) -> usize {
    let axis = |pick: fn(&(u32, u32)) -> u32| {
        sizes
            .windows(3)
            .filter(|w| {
                let first = (i64::from(pick(&w[1])) - i64::from(pick(&w[0]))).signum();
                let second = (i64::from(pick(&w[2])) - i64::from(pick(&w[1]))).signum();
                second != 0 && second == -first
            })
            .count()
    };
    axis(|size| size.0) + axis(|size| size.1)
}

/// Indices of live presents, grouped into gestures.
fn live_groups(presents: &[Present]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, present) in presents.iter().enumerate() {
        if !present.live {
            continue;
        }
        if let Some(group) = groups.last_mut()
            && let Some(&last) = group.last()
            && present.ms - presents[last].ms < GESTURE_SPLIT_MS
        {
            group.push(index);
            continue;
        }
        groups.push(vec![index]);
    }
    groups
}

#[derive(Debug)]
struct GestureReport {
    number: usize,
    cold: bool,
    start: f64,
    end: f64,
    steps: usize,
    dup_rate: f64,
    dup_vias: BTreeMap<String, usize>,
    reversals: usize,
    gaps: Option<Dist>,
    starved: usize,
    stages: Vec<(char, Dist)>,
    over_frame: usize,
    over_two_frames: usize,
    seg_alive_max: u64,
    seg_raster: u64,
    seg_px: u64,
    r_by_kind: BTreeMap<String, usize>,
    redraw_per_resize: Option<f64>,
    via_histogram: BTreeMap<String, usize>,
}

#[derive(Debug)]
struct RestReport {
    start: f64,
    end: f64,
    presents: usize,
    pps: f64,
    via_histogram: BTreeMap<String, usize>,
    dt_modes: Vec<(i64, usize)>,
    stage_p50: Vec<(char, f64)>,
    seg_alive_max: u64,
}

#[derive(Debug)]
struct Analysis {
    gestures: Vec<GestureReport>,
    rests: Vec<RestReport>,
    cold_ratio: Option<Vec<(char, f64)>>,
}

fn presents_between(tape: &Tape, start: f64, end: f64) -> Vec<&Present> {
    tape.presents.iter().filter(|p| p.ms >= start && p.ms <= end).collect()
}

fn gesture_report(tape: &Tape, number: usize, group: &[usize]) -> GestureReport {
    let live: Vec<&Present> = group.iter().map(|&index| &tape.presents[index]).collect();
    let start = live[0].ms;
    let end = live[live.len() - 1].ms;
    let steps = live.len();

    // duplicates: a live step that repainted the size of the step
    // before it — the via of the SECOND line names the presenter
    let mut dups = 0usize;
    let mut dup_vias: BTreeMap<String, usize> = BTreeMap::new();
    for pair in live.windows(2) {
        if (pair[0].w, pair[0].h) == (pair[1].w, pair[1].h) {
            dups += 1;
            *dup_vias.entry(pair[1].via.clone()).or_insert(0) += 1;
        }
    }
    let dup_rate = if steps > 1 { dups as f64 / (steps - 1) as f64 } else { 0.0 };

    let sizes: Vec<(u32, u32)> = live.iter().map(|p| (p.w, p.h)).collect();
    let gap_values: Vec<f64> = live.windows(2).map(|pair| pair[1].ms - pair[0].ms).collect();
    let starved = gap_values.iter().filter(|gap| **gap > STARVED_GAP_MS).count();
    let gaps = dist(gap_values);

    // every present inside the span belongs to the gesture — the
    // live ones drove it, the rest rode along
    let span = presents_between(tape, start, end);
    let stages: Vec<(char, Dist)> = STAGES
        .iter()
        .filter_map(|&stage| {
            dist(span.iter().filter_map(|p| stage_dur(p, stage)).collect()).map(|d| (stage, d))
        })
        .collect();
    let over_frame = span.iter().filter(|p| p.total.is_some_and(|t| t > FRAME_MS)).count();
    let over_two_frames =
        span.iter().filter(|p| p.total.is_some_and(|t| t > TWO_FRAMES_MS)).count();

    let marks: Vec<&SegMark> = span.iter().filter_map(|p| p.seg.as_ref()).collect();
    let seg_alive_max = marks.iter().map(|seg| seg.alive).max().unwrap_or(0);
    let seg_raster = marks.iter().map(|seg| seg.raster).sum();
    let seg_px = marks.iter().map(|seg| seg.px).sum();

    let mut r_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for resize in tape.resizes.iter().filter(|r| r.ms >= start && r.ms <= end) {
        *r_by_kind.entry(resize.kind.clone()).or_insert(0) += 1;
    }
    let redraws = span.iter().filter(|p| p.via == "redraw").count();
    let resize_calls = r_by_kind.get("resize").copied().unwrap_or(0);
    let redraw_per_resize =
        (resize_calls > 0).then(|| redraws as f64 / resize_calls as f64);

    let mut via_histogram: BTreeMap<String, usize> = BTreeMap::new();
    for present in &span {
        *via_histogram.entry(present.via.clone()).or_insert(0) += 1;
    }

    GestureReport {
        number,
        cold: number == 1,
        start,
        end,
        steps,
        dup_rate,
        dup_vias,
        reversals: reversals(&sizes),
        gaps,
        starved,
        stages,
        over_frame,
        over_two_frames,
        seg_alive_max,
        seg_raster,
        seg_px,
        r_by_kind,
        redraw_per_resize,
        via_histogram,
    }
}

fn rest_report(tape: &Tape, start: f64, end: f64) -> RestReport {
    let presents = presents_between(tape, start, end);
    let seconds = (end - start) / 1000.0;
    let pps = if seconds > 0.0 { presents.len() as f64 / seconds } else { 0.0 };

    let mut via_histogram: BTreeMap<String, usize> = BTreeMap::new();
    for present in &presents {
        *via_histogram.entry(present.via.clone()).or_insert(0) += 1;
    }

    // the beat of the rest: the most common intervals, to the ms
    let mut dt_counts: BTreeMap<i64, usize> = BTreeMap::new();
    for pair in presents.windows(2) {
        *dt_counts.entry((pair[1].ms - pair[0].ms).round() as i64).or_insert(0) += 1;
    }
    let mut dt_modes: Vec<(i64, usize)> = dt_counts.into_iter().collect();
    dt_modes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    dt_modes.truncate(3);

    let stage_p50: Vec<(char, f64)> = STAGES
        .iter()
        .filter_map(|&stage| {
            dist(presents.iter().filter_map(|p| stage_dur(p, stage)).collect())
                .map(|d| (stage, d.p50))
        })
        .collect();
    let seg_alive_max =
        presents.iter().filter_map(|p| p.seg.as_ref()).map(|seg| seg.alive).max().unwrap_or(0);

    RestReport {
        start,
        end,
        presents: presents.len(),
        pps,
        via_histogram,
        dt_modes,
        stage_p50,
        seg_alive_max,
    }
}

/// The stretches outside gestures, trimmed and length-checked.
fn rest_reports(tape: &Tape, groups: &[Vec<usize>]) -> Vec<RestReport> {
    let (Some(first), Some(last)) = (tape.first_ms, tape.last_ms) else {
        return Vec::new();
    };
    let mut free: Vec<(f64, f64)> = Vec::new();
    let mut cursor = first;
    for group in groups {
        let start = tape.presents[group[0]].ms;
        let end = tape.presents[*group.last().expect("groups are non-empty")].ms;
        if start > cursor {
            free.push((cursor, start));
        }
        cursor = cursor.max(end);
    }
    if last > cursor {
        free.push((cursor, last));
    }
    free.into_iter()
        .filter_map(|(a, b)| {
            let (a, b) = (a + REST_TRIM_MS, b - REST_TRIM_MS);
            (b - a >= REST_MIN_MS).then(|| rest_report(tape, a, b))
        })
        .collect()
}

/// Gesture 1's worst stage time over the pooled warm median.
fn cold_ratios(tape: &Tape, groups: &[Vec<usize>]) -> Option<Vec<(char, f64)>> {
    if groups.len() < 2 {
        return None;
    }
    let durs = |group: &[usize], stage: char| -> Vec<f64> {
        let start = tape.presents[group[0]].ms;
        let end = tape.presents[*group.last().expect("groups are non-empty")].ms;
        presents_between(tape, start, end)
            .iter()
            .filter_map(|p| stage_dur(p, stage))
            .collect()
    };
    let mut ratios = Vec::new();
    for &stage in &STAGES {
        let cold = durs(&groups[0], stage);
        let Some(cold_max) = cold.into_iter().max_by(f64::total_cmp) else {
            continue;
        };
        let mut warm: Vec<f64> = groups[1..].iter().flat_map(|group| durs(group, stage)).collect();
        if warm.is_empty() {
            continue;
        }
        warm.sort_by(f64::total_cmp);
        let median = percentile(&warm, 50.0);
        if median > 0.0 {
            ratios.push((stage, cold_max / median));
        }
    }
    (!ratios.is_empty()).then_some(ratios)
}

fn analyze(tape: &Tape) -> Analysis {
    let groups = live_groups(&tape.presents);
    Analysis {
        gestures: groups
            .iter()
            .enumerate()
            .map(|(index, group)| gesture_report(tape, index + 1, group))
            .collect(),
        rests: rest_reports(tape, &groups),
        cold_ratio: cold_ratios(tape, &groups),
    }
}

// ------------------------------------------------------------------
// The human report
// ------------------------------------------------------------------

fn histogram_line(map: &BTreeMap<String, usize>) -> String {
    if map.is_empty() {
        return "(none)".to_string();
    }
    map.iter().map(|(key, count)| format!("{key} {count}")).collect::<Vec<_>>().join("  ")
}

fn dist_line(dist: &Dist) -> String {
    format!("p50 {:>6.1}   p95 {:>6.1}   max {:>6.1}", dist.p50, dist.p95, dist.max)
}

fn section(out: &mut String, title: &str) {
    let dashes = "-".repeat(60usize.saturating_sub(title.len() + 4));
    let _ = write!(out, "\n-- {title} {dashes}\n");
}

fn row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "{label:<16} {value}");
}

fn render_human(path: &str, tape: &Tape, analysis: &Analysis) -> String {
    let mut out = String::new();
    row(&mut out, "tape", path);
    match &tape.header {
        Some(header) => row(
            &mut out,
            "header",
            &format!(
                "{}  pid {}  t0 {}  tag \"{}\"",
                header.version, header.pid, header.t0, header.tag
            ),
        ),
        None => row(&mut out, "header", "(none — a v1 tape?)"),
    }
    row(
        &mut out,
        "records",
        &format!(
            "{} presents, {} window callbacks, {} one-time marks",
            tape.presents.len(),
            tape.resizes.len(),
            tape.one_time.len()
        ),
    );
    row(&mut out, "ignored", &format!("{} lines", tape.ignored));
    if let (Some(first), Some(last)) = (tape.first_ms, tape.last_ms) {
        row(
            &mut out,
            "span",
            &format!("{first:.1} .. {last:.1} ms  ({:.1} s)", (last - first) / 1000.0),
        );
    }
    row(&mut out, "stages", "H hosts  O panels  M sync  S segments  E whole present");

    if analysis.gestures.is_empty() {
        section(&mut out, "gestures");
        let _ = writeln!(out, "(none — no live presents on this tape)");
    }
    for gesture in &analysis.gestures {
        let phase = if gesture.cold { "cold" } else { "warm" };
        section(&mut out, &format!("gesture {} ({phase})", gesture.number));
        row(
            &mut out,
            "span",
            &format!(
                "{:.1} .. {:.1} ms  ({:.1} ms)",
                gesture.start,
                gesture.end,
                gesture.end - gesture.start
            ),
        );
        row(&mut out, "steps", &format!("{} live presents", gesture.steps));
        row(
            &mut out,
            "dup rate",
            &format!(
                "{:.3}   duplicates by via: {}",
                gesture.dup_rate,
                histogram_line(&gesture.dup_vias)
            ),
        );
        row(&mut out, "reversals", &gesture.reversals.to_string());
        match &gesture.gaps {
            Some(gaps) => row(&mut out, "gap ms", &dist_line(gaps)),
            None => row(&mut out, "gap ms", "(single step)"),
        }
        row(&mut out, "starved gaps", &format!("{} over {STARVED_GAP_MS:.0} ms", gesture.starved));
        for (stage, dist) in &gesture.stages {
            row(&mut out, &format!("stage {stage} ms"), &dist_line(dist));
        }
        row(
            &mut out,
            "overruns",
            &format!(
                "E over {FRAME_MS} ms: {}   over {TWO_FRAMES_MS} ms: {}",
                gesture.over_frame, gesture.over_two_frames
            ),
        );
        row(
            &mut out,
            "segments",
            &format!(
                "alive max {}   raster {}   px {}",
                gesture.seg_alive_max, gesture.seg_raster, gesture.seg_px
            ),
        );
        row(&mut out, "R by kind", &histogram_line(&gesture.r_by_kind));
        match gesture.redraw_per_resize {
            Some(ratio) => row(&mut out, "redraw/resize", &format!("{ratio:.2}")),
            None => row(&mut out, "redraw/resize", "(no resize callbacks)"),
        }
        row(&mut out, "via", &histogram_line(&gesture.via_histogram));
    }

    for (index, rest) in analysis.rests.iter().enumerate() {
        section(&mut out, &format!("rest {}", index + 1));
        row(
            &mut out,
            "span",
            &format!(
                "{:.1} .. {:.1} ms  ({:.1} s)",
                rest.start,
                rest.end,
                (rest.end - rest.start) / 1000.0
            ),
        );
        row(
            &mut out,
            "presents",
            &format!("{}  ({:.3} per second)", rest.presents, rest.pps),
        );
        row(&mut out, "via", &histogram_line(&rest.via_histogram));
        let modes = if rest.dt_modes.is_empty() {
            "(none)".to_string()
        } else {
            rest.dt_modes
                .iter()
                .map(|(dt, count)| format!("{dt} ms x{count}"))
                .collect::<Vec<_>>()
                .join("   ")
        };
        row(&mut out, "dt modes", &modes);
        let p50s = if rest.stage_p50.is_empty() {
            "(no marks)".to_string()
        } else {
            rest.stage_p50
                .iter()
                .map(|(stage, p50)| format!("{stage} {p50:.1}"))
                .collect::<Vec<_>>()
                .join("   ")
        };
        row(&mut out, "stage p50 ms", &p50s);
        row(&mut out, "segments", &format!("alive max {}", rest.seg_alive_max));
    }

    section(&mut out, "cold ratio (gesture 1 max over warm median)");
    match &analysis.cold_ratio {
        Some(ratios) => {
            let line = ratios
                .iter()
                .map(|(stage, ratio)| format!("{stage} {ratio:.2}"))
                .collect::<Vec<_>>()
                .join("   ");
            let _ = writeln!(out, "{line}");
        }
        None => {
            let _ = writeln!(out, "(needs at least 2 gestures)");
        }
    }

    section(&mut out, "one-time marks");
    if tape.one_time.is_empty() {
        let _ = writeln!(out, "(none)");
    }
    for mark in &tape.one_time {
        let bytes = mark.bytes.map_or(String::new(), |bytes| format!("  bytes {bytes}"));
        let _ = writeln!(out, "{:>10.1} ms  {}{bytes}", mark.ms, mark.what);
    }
    out
}

// ------------------------------------------------------------------
// The JSON report — hand-rolled, stable keys, no dependencies
// ------------------------------------------------------------------

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number, rounded to 3 decimal places.
fn json_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_string();
    }
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded == rounded.trunc() && rounded.abs() < 1e15 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

fn json_histogram(map: &BTreeMap<String, usize>) -> String {
    let inner: Vec<String> =
        map.iter().map(|(key, count)| format!("{}:{count}", json_string(key))).collect();
    format!("{{{}}}", inner.join(","))
}

fn json_dist(dist: &Dist) -> String {
    format!(
        "{{\"p50\":{},\"p95\":{},\"max\":{}}}",
        json_number(dist.p50),
        json_number(dist.p95),
        json_number(dist.max)
    )
}

fn json_stage_map<T, F: Fn(&T) -> String>(pairs: &[(char, T)], value: F) -> String {
    let inner: Vec<String> =
        pairs.iter().map(|(stage, item)| format!("\"{stage}\":{}", value(item))).collect();
    format!("{{{}}}", inner.join(","))
}

fn render_json(tape: &Tape, analysis: &Analysis) -> String {
    let mut out = String::from("{\"header\":");
    match &tape.header {
        Some(header) => {
            let _ = write!(
                out,
                "{{\"version\":{},\"pid\":{},\"t0\":{},\"tag\":{}}}",
                json_string(&header.version),
                header.pid,
                header.t0,
                json_string(&header.tag)
            );
        }
        None => out.push_str("null"),
    }
    let _ = write!(out, ",\"ignored_lines\":{}", tape.ignored);

    out.push_str(",\"gestures\":[");
    for (index, gesture) in analysis.gestures.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"number\":{},\"phase\":{},\"start_ms\":{},\"end_ms\":{},\"span_ms\":{},\
             \"steps\":{},\"dup_rate\":{},\"dup_vias\":{},\"reversals\":{},\"gap_ms\":{},\
             \"starved_gaps\":{},\"stages\":{},\"overruns_over_8_3_ms\":{},\
             \"overruns_over_16_7_ms\":{},\"seg_alive_max\":{},\"seg_raster\":{},\
             \"seg_px\":{},\"r_by_kind\":{},\"redraw_per_resize\":{},\"via_histogram\":{}}}",
            gesture.number,
            json_string(if gesture.cold { "cold" } else { "warm" }),
            json_number(gesture.start),
            json_number(gesture.end),
            json_number(gesture.end - gesture.start),
            gesture.steps,
            json_number(gesture.dup_rate),
            json_histogram(&gesture.dup_vias),
            gesture.reversals,
            gesture.gaps.as_ref().map_or("null".to_string(), json_dist),
            gesture.starved,
            json_stage_map(&gesture.stages, json_dist),
            gesture.over_frame,
            gesture.over_two_frames,
            gesture.seg_alive_max,
            gesture.seg_raster,
            gesture.seg_px,
            json_histogram(&gesture.r_by_kind),
            gesture.redraw_per_resize.map_or("null".to_string(), json_number),
            json_histogram(&gesture.via_histogram)
        );
    }
    out.push(']');

    out.push_str(",\"rests\":[");
    for (index, rest) in analysis.rests.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let modes: Vec<String> = rest
            .dt_modes
            .iter()
            .map(|(dt, count)| format!("{{\"dt_ms\":{dt},\"count\":{count}}}"))
            .collect();
        let _ = write!(
            out,
            "{{\"start_ms\":{},\"end_ms\":{},\"presents\":{},\"pps\":{},\
             \"via_histogram\":{},\"dt_modes\":[{}],\"stage_p50\":{},\"seg_alive_max\":{}}}",
            json_number(rest.start),
            json_number(rest.end),
            rest.presents,
            json_number(rest.pps),
            json_histogram(&rest.via_histogram),
            modes.join(","),
            json_stage_map(&rest.stage_p50, |p50| json_number(*p50)),
            rest.seg_alive_max
        );
    }
    out.push(']');

    out.push_str(",\"cold_ratio\":");
    match &analysis.cold_ratio {
        Some(ratios) => out.push_str(&json_stage_map(ratios, |ratio| json_number(*ratio))),
        None => out.push_str("null"),
    }

    out.push_str(",\"x_events\":[");
    for (index, mark) in tape.one_time.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"ms\":{},\"what\":{},\"bytes\":{}}}",
            json_number(mark.ms),
            json_string(&mark.what),
            mark.bytes.map_or("null".to_string(), |bytes| bytes.to_string())
        );
    }
    out.push_str("]}");
    out
}

// ------------------------------------------------------------------
// Entry
// ------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let json = args.iter().any(|arg| arg == "--json");
    let Some(path) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: trace_report <path> [--json]");
        eprintln!("  <path>   a tape written under BUNNY_PRESENT_TRACE=1");
        eprintln!("  --json   the same report as one JSON object");
        process::exit(2);
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            process::exit(1);
        }
    };
    let tape = parse_tape(&text);
    let analysis = analyze(&tape);
    if json {
        println!("{}", render_json(&tape, &analysis));
    } else {
        print!("{}", render_human(path, &tape, &analysis));
    }
}

// ------------------------------------------------------------------
// Tests — a golden tape with numbers worked out by hand
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Timeline of the golden tape:
    /// - one X at 500 opens the timeline;
    /// - a rest: presents every 500 ms from 1000 to 4500, two vias.
    ///   The free stretch [500, 10500] trims to [1500, 9500] (8 s);
    ///   inside it: 7 presents, so 0.875 per second;
    /// - two malformed lines, ignored;
    /// - gesture 1 (cold), 5 steps: one duplicate size via wake, one
    ///   width reversal (805 -> 812 against 810 -> 805), one X
    ///   atlas-drain, 3 resize callbacks + 1 move, 4 redraw presents;
    /// - the stretch between gestures trims below 5 s: no rest;
    /// - gesture 2 (warm), 3 clean steps, ratio redraw/resize = 1.
    const GOLDEN: &str = "# bunny-trace v2 pid=4242 t0=1724592000000 tag=golden
X 500.0 what=sync-on
P 1000.0 800x600 live=0 cmds=3 via=frame
E 1000.0 dur=1.0
P 1500.0 800x600 live=0 cmds=3 via=frame
E 1500.0 dur=1.0
P 2000.0 800x600 live=0 cmds=3 via=frame
H 2000.0 dur=0.4 hosts=1
E 2000.0 dur=1.0
P 2500.0 800x600 live=0 cmds=3 via=blink
E 2500.0 dur=1.0
P 3000.0 800x600 live=0 cmds=3 via=frame
E 3000.0 dur=2.0
P 3500.0 800x600 live=0 cmds=3 via=blink
E 3500.0 dur=1.0
P 4000.0 800x600 live=0 cmds=3 via=frame
E 4000.0 dur=1.0
P 4500.0 800x600 live=0 cmds=3 via=frame
E 4500.0 dur=1.0
Z 4600.0 what=not-a-record
P 99999
R 10500.0 800x600 kind=resize live=1
P 10500.0 800x600 live=1 cmds=40 via=redraw
H 10500.0 dur=2.0 hosts=2
O 10500.0 dur=1.0 panels=1
M 10500.0 dur=1.5 sync=1
S 10500.0 dur=6.0 n=12 raster=3 px=250000
E 10500.0 dur=12.0
R 10516.0 810x600 kind=resize live=1
P 10516.0 810x600 live=1 cmds=40 via=redraw
H 10516.0 dur=1.0 hosts=2
O 10516.0 dur=0.5 panels=1
M 10516.0 dur=1.0 sync=1
S 10516.0 dur=3.0 n=12 raster=1 px=90000
E 10516.0 dur=7.0
P 10520.0 810x600 live=1 cmds=40 via=wake
H 10520.0 dur=0.5 hosts=2
O 10520.0 dur=0.5 panels=1
M 10520.0 dur=0.5 sync=1
S 10520.0 dur=1.0 n=12 raster=0 px=0
E 10520.0 dur=3.0
X 10520.0 what=atlas-drain bytes=524288
R 10533.0 805x600 kind=resize live=1
P 10533.0 805x600 live=1 cmds=40 via=redraw
H 10533.0 dur=1.0 hosts=2
O 10533.0 dur=0.5 panels=1
M 10533.0 dur=1.0 sync=1
S 10533.0 dur=2.0 n=12 raster=1 px=90000
E 10533.0 dur=5.0
R 10540.0 805x600 kind=move live=1
P 10549.0 812x600 live=1 cmds=40 via=redraw
H 10549.0 dur=1.0 hosts=2
O 10549.0 dur=0.5 panels=1
M 10549.0 dur=1.0 sync=1
S 10549.0 dur=2.0 n=12 raster=1 px=90000
E 10549.0 dur=5.0
R 16000.0 900x600 kind=resize live=1
P 16000.0 900x600 live=1 cmds=40 via=redraw
H 16000.0 dur=1.0 hosts=2
O 16000.0 dur=0.5 panels=1
M 16000.0 dur=1.0 sync=1
S 16000.0 dur=2.0 n=10 raster=1 px=90000
E 16000.0 dur=5.0
R 16016.0 910x600 kind=resize live=1
P 16016.0 910x600 live=1 cmds=40 via=redraw
H 16016.0 dur=0.5 hosts=2
O 16016.0 dur=0.5 panels=1
M 16016.0 dur=0.5 sync=1
S 16016.0 dur=1.0 n=10 raster=1 px=90000
E 16016.0 dur=4.0
R 16032.0 920x600 kind=resize live=1
P 16032.0 920x600 live=1 cmds=40 via=redraw
H 16032.0 dur=1.0 hosts=2
O 16032.0 dur=0.5 panels=1
M 16032.0 dur=1.0 sync=1
S 16032.0 dur=2.0 n=10 raster=1 px=90000
E 16032.0 dur=4.0
";

    fn ratio(analysis: &Analysis, stage: char) -> f64 {
        analysis
            .cold_ratio
            .as_ref()
            .expect("two gestures give a cold ratio")
            .iter()
            .find(|(s, _)| *s == stage)
            .expect("every stage has marks")
            .1
    }

    #[test]
    fn golden_header_and_ignored() {
        let tape = parse_tape(GOLDEN);
        let header = tape.header.as_ref().expect("the golden tape has a header");
        assert_eq!(header.version, "v2");
        assert_eq!(header.pid, 4242);
        assert_eq!(header.t0, 1_724_592_000_000);
        assert_eq!(header.tag, "golden");
        assert_eq!(tape.ignored, 2, "the Z line and the truncated P line");
        assert_eq!(tape.presents.len(), 16);
        assert_eq!(tape.resizes.len(), 7);
    }

    #[test]
    fn golden_gestures() {
        let tape = parse_tape(GOLDEN);
        let analysis = analyze(&tape);
        assert_eq!(analysis.gestures.len(), 2);

        let first = &analysis.gestures[0];
        assert!(first.cold);
        assert_eq!(first.steps, 5);
        assert_eq!(first.start, 10500.0);
        assert_eq!(first.end, 10549.0);
        assert_eq!(first.dup_rate, 0.25, "1 duplicate over 4 deltas");
        assert_eq!(first.dup_vias.len(), 1);
        assert_eq!(first.dup_vias.get("wake"), Some(&1), "the wake present is the imposter");
        assert_eq!(first.reversals, 1, "805 -> 812 turns against 810 -> 805");
        let gaps = first.gaps.as_ref().expect("5 steps give 4 gaps");
        assert_eq!(gaps.p50, 13.0);
        assert_eq!(gaps.p95, 16.0);
        assert_eq!(gaps.max, 16.0);
        assert_eq!(first.starved, 0);
        assert_eq!(first.over_frame, 1, "only the 12.0 ms present overran 8.3");
        assert_eq!(first.over_two_frames, 0);
        assert_eq!(first.seg_alive_max, 12);
        assert_eq!(first.seg_raster, 6);
        assert_eq!(first.seg_px, 520_000);
        assert_eq!(first.r_by_kind.get("resize"), Some(&3));
        assert_eq!(first.r_by_kind.get("move"), Some(&1));
        assert_eq!(first.r_by_kind.len(), 2);
        assert_eq!(first.redraw_per_resize, Some(4.0 / 3.0), "4 redraws over 3 callbacks");
        assert_eq!(first.via_histogram.get("redraw"), Some(&4));
        assert_eq!(first.via_histogram.get("wake"), Some(&1));

        let second = &analysis.gestures[1];
        assert!(!second.cold);
        assert_eq!(second.steps, 3);
        assert_eq!(second.dup_rate, 0.0);
        assert_eq!(second.reversals, 0);
        assert_eq!(second.r_by_kind.get("resize"), Some(&3));
        assert_eq!(second.redraw_per_resize, Some(1.0));
        assert_eq!(second.over_frame, 0);
    }

    #[test]
    fn golden_rest() {
        let tape = parse_tape(GOLDEN);
        let analysis = analyze(&tape);
        assert_eq!(analysis.rests.len(), 1, "the stretch between gestures is too short");

        let rest = &analysis.rests[0];
        assert_eq!(rest.start, 1500.0);
        assert_eq!(rest.end, 9500.0);
        assert_eq!(rest.presents, 7);
        assert_eq!(rest.pps, 0.875, "7 presents over 8 seconds");
        assert_eq!(rest.via_histogram.get("frame"), Some(&5));
        assert_eq!(rest.via_histogram.get("blink"), Some(&2));
        assert_eq!(rest.via_histogram.len(), 2);
        assert_eq!(rest.dt_modes, vec![(500, 6)]);
        assert_eq!(rest.stage_p50, vec![('H', 0.4), ('E', 1.0)]);
        assert_eq!(rest.seg_alive_max, 0);
    }

    #[test]
    fn golden_cold_ratio() {
        let tape = parse_tape(GOLDEN);
        let analysis = analyze(&tape);
        assert_eq!(ratio(&analysis, 'H'), 2.0);
        assert_eq!(ratio(&analysis, 'O'), 2.0);
        assert_eq!(ratio(&analysis, 'M'), 1.5);
        assert_eq!(ratio(&analysis, 'S'), 3.0);
        assert_eq!(ratio(&analysis, 'E'), 3.0);
    }

    #[test]
    fn golden_one_time_marks() {
        let tape = parse_tape(GOLDEN);
        assert_eq!(tape.one_time.len(), 2);
        assert_eq!(tape.one_time[0].what, "sync-on");
        assert_eq!(tape.one_time[0].ms, 500.0);
        assert_eq!(tape.one_time[0].bytes, None);
        assert_eq!(tape.one_time[1].what, "atlas-drain");
        assert_eq!(tape.one_time[1].bytes, Some(524_288));
    }

    #[test]
    fn golden_renders() {
        let tape = parse_tape(GOLDEN);
        let analysis = analyze(&tape);
        let human = render_human("golden", &tape, &analysis);
        assert!(human.contains("gesture 1 (cold)"));
        assert!(human.contains("gesture 2 (warm)"));
        assert!(human.contains("rest 1"));
        assert!(human.contains("atlas-drain"));
        let json = render_json(&tape, &analysis);
        assert!(json.starts_with("{\"header\":{\"version\":\"v2\",\"pid\":4242"));
        assert!(json.contains("\"dup_rate\":0.25"));
        assert!(json.contains("\"pps\":0.875"));
        assert!(json.contains("\"redraw_per_resize\":1.333"));
        assert!(json.ends_with("}"));
    }

    #[test]
    fn v1_present_line_gets_a_question_mark() {
        let tape = parse_tape("P 12.0 100x100 live=1 cmds=3\n");
        assert_eq!(tape.presents.len(), 1);
        assert_eq!(tape.presents[0].via, "?");
        assert_eq!(tape.ignored, 0);
    }

    #[test]
    fn truncated_and_interleaved_tape_never_panics() {
        // stage marks with no open present, a present with no E, a
        // second P that lands before the first E
        let tape = parse_tape("H 1.0 dur=2.0\nE 2.0 dur=3.0\nP 5.0 100x100 live=1\nP 9.0 110x100 live=1\nE 9.0 dur=1.0\nP 12.0 120x");
        assert_eq!(tape.presents.len(), 2);
        assert_eq!(tape.presents[0].total, None, "truncated by the second P");
        assert_eq!(tape.presents[1].total, Some(1.0));
        assert_eq!(tape.ignored, 3, "two orphan marks and one torn P");
        let analysis = analyze(&tape);
        assert_eq!(analysis.gestures.len(), 1);
        assert!(analysis.rests.is_empty());
    }
}
