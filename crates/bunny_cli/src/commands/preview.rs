//! `bunny preview`: render the app's `#[preview]` functions headlessly
//! and keep them rendered — a save reloads the code, and each new frame
//! shows up here as a line.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use bunny_ui_preview::wire::{Scheme, Selection, Size, Sizing, ToHost, ToIde, Traits};

use crate::args::{HELP, Matches, Opt};
use crate::commands::doctor;
use crate::devices::Platform;
use crate::error::{Error, Result};
use crate::formats::png;
use crate::keys::{INTERRUPT, Keys};
use crate::preview::{Event, Options, Preview, Step};
use crate::term;

pub const SUMMARY: &str = "Render the app's #[preview] functions, and keep them rendered as you save";

pub const USAGE: &str = "bunny preview [NAME...] [--list] [--json] [--png DIR] [OPTIONS]";

pub const ABOUT: &str = "\
Lists the previews the app's sources mark with #[preview] (--list), or
builds the app with its preview entries, starts it as a preview host —
no window — and renders the named previews (every preview, when none is
named). Each frame the host sends shows up as a line; a save reloads the
app's code hot and the frames follow.

While it runs: r reloads now, R restarts the host, q stops it.

With --png DIR each named preview is rendered once, its pixels asked of
the host — the same picture the frame paints, through the app's own
text and image engines — and written as DIR/<ident>.png; then the host
stops. --size WxH renders at that size in points instead of what each
view wants, --dark under the dark scheme.";

pub const OPTIONS: &[Opt] = &[
    Opt::flag("list", "List the previews the sources mark, without building"),
    Opt::flag("json", "With --list: the list as JSON"),
    Opt::value("features", "LIST", "Cargo features to turn on (repeatable)"),
    Opt::value("package", "NAME", "The app, in a workspace of several").short('p'),
    Opt::value("png", "DIR", "Render each preview once to DIR/<ident>.png, then stop"),
    Opt::value("size", "WxH", "With --png: render at this size in points, not what each view wants"),
    Opt::flag("dark", "With --png: render under the dark scheme"),
    HELP,
];

/// How often the lane looks for saves.
const LOOK: Duration = Duration::from_millis(150);

pub fn run(matches: &Matches) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let options = Options {
        features: matches.values("features").iter().flat_map(|list| list.split(',')).map(str::trim).filter(|f| !f.is_empty()).map(String::from).collect(),
        package: matches.value("package").map(String::from),
    };
    let lane = Preview::new(&cwd, options)?;
    if matches.flag("list") {
        let report = lane.previews();
        if matches.flag("json") {
            print!("{}", report.json());
        } else {
            list(&report);
        }
        return Ok(());
    }
    doctor::preflight(Platform::host())?;
    let wanted: Vec<String> = matches.positionals.clone();
    let traits = traits_of(matches)?;
    let mut events = Vec::new();
    println!("{}", term::bold("Building the app with its previews…"));
    let began = Instant::now();
    let live = match lane.start(&mut events) {
        Step::Live(live) => live,
        Step::Failed(failed) => {
            render(&events, &wanted);
            return Err(Error::new(failed.why().to_string()).hint("fix it and run `bunny preview` again"));
        }
    };
    render(&events, &wanted);
    println!("{} {}", term::ok("Host up"), term::dim(&format!("(built in {:.1}s)", began.elapsed().as_secs_f32())));
    match matches.value("png") {
        Some(dir) => pictures(live, wanted, Path::new(dir), traits),
        None => watch(live, wanted),
    }
}

/// The traits `--size` and `--dark` ask for.
fn traits_of(matches: &Matches) -> Result<Traits> {
    let size = match matches.value("size") {
        Some(spec) => {
            let (width, height) = spec
                .split_once(['x', 'X', '×'])
                .and_then(|(w, h)| Some((w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?)))
                .filter(|(w, h)| *w >= 1.0 && *h >= 1.0)
                .ok_or_else(|| Error::new(format!("--size wants WxH in points, not `{spec}`")).hint("for example --size 390x844"))?;
            Sizing::Fixed(Size { width, height })
        }
        None => Sizing::Fits,
    };
    let scheme = if matches.flag("dark") { Scheme::Dark } else { Scheme::Light };
    Ok(Traits { scheme, size, ..Traits::default() })
}

/// `--png`: every wanted preview rendered once under `traits`, its pixels
/// asked of the host after its first frame and written as
/// `DIR/<ident>.png` (`<ident>-v<n>.png` past the first variant); then
/// the host stops.
fn pictures(mut live: Preview<crate::preview::Live>, wanted: Vec<String>, dir: &Path, traits: Traits) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|error| Error::new(format!("{}: {error}", dir.display())))?;
    let until = Instant::now() + Duration::from_secs(120);
    let mut events = Vec::new();
    let mut selected = false;
    let mut pending: Vec<String> = Vec::new();
    let mut asked: HashSet<String> = HashSet::new();
    let mut written = 0usize;
    loop {
        let alive = live.wait(Duration::from_millis(50), &mut events);
        for event in events.drain(..) {
            match &event {
                Event::Host(ToIde::Ready { previews, .. }) if !selected => {
                    let chosen: Vec<Selection> = previews
                        .iter()
                        .filter(|preview| wanted.is_empty() || wanted.iter().any(|name| *name == preview.ident || *name == preview.name))
                        .map(|preview| Selection { id: preview.id.clone(), ident: preview.ident.clone(), traits: traits.clone() })
                        .collect();
                    if chosen.is_empty() {
                        live.stop();
                        return Err(Error::new(format!("none of {wanted:?} is a preview of this app")).hint("`bunny preview --list` names them"));
                    }
                    pending = chosen.iter().map(|selection| selection.id.clone()).collect();
                    let _ = live.send(&ToHost::Select(chosen));
                    selected = true;
                }
                Event::Host(ToIde::Frame(frame)) => {
                    // the scene's first frame: its pixels, once
                    let suffix = format!("#{}", frame.preview);
                    if let Some(id) = pending.iter().find(|id| id.ends_with(&suffix)).cloned()
                        && asked.insert(id.clone())
                    {
                        let _ = live.send(&ToHost::RequestSnapshot { id });
                    }
                }
                Event::Host(ToIde::Snapshot { preview, variant, size, rgba }) => {
                    let name = if *variant == 0 { format!("{preview}.png") } else { format!("{preview}-v{variant}.png") };
                    let path = dir.join(&name);
                    std::fs::write(&path, png::encode(size.0, size.1, rgba)?).map_err(|error| Error::new(format!("{}: {error}", path.display())))?;
                    println!("{} {}", term::ok(&path.display().to_string()), term::dim(&format!("{}×{}", size.0, size.1)));
                    written += 1;
                    let suffix = format!("#{preview}");
                    pending.retain(|id| !id.ends_with(&suffix));
                }
                Event::BuildFailed { .. } | Event::Crashed { .. } => {
                    print_event(&event, &wanted);
                    live.stop();
                    return Err(Error::new("no pictures: the host did not render"));
                }
                _ => print_event(&event, &wanted),
            }
        }
        if selected && pending.is_empty() {
            live.stop();
            println!("{}", term::dim(&format!("{written} picture(s) in {}", dir.display())));
            return Ok(());
        }
        if !alive {
            return Err(Error::new("the host ended before its pictures were taken"));
        }
        if Instant::now() > until {
            live.stop();
            return Err(Error::new(format!("the host sent no picture of {pending:?} within 120 s")));
        }
    }
}

/// The list: `scope / name — example (file:line)`.
fn list(report: &bunny_ui_preview::scan::ScanReport) {
    if report.previews.is_empty() {
        println!("{}", term::dim("no #[preview] functions under src/"));
    }
    for preview in &report.previews {
        let scope = if preview.scope.is_empty() { "unsorted" } else { preview.scope.as_str() };
        println!(
            "{} / {} {} {}",
            term::dim(scope),
            term::bold(&preview.name),
            term::dim(&format!("— {}", preview.example)),
            term::dim(&format!("({}:{})", preview.file, preview.line))
        );
    }
    for refused in &report.refused {
        println!("{}", term::warn(&refused.to_string()));
    }
    for duplicate in &report.duplicates {
        println!("{}", term::warn(&format!("`{}` twice: {} keeps it, {} dropped", duplicate.ident, duplicate.kept, duplicate.dropped)));
    }
}

/// Keeps the host running: its frames as lines, a save reloaded, a key
/// honoured. Ends with the host, or on q.
fn watch(mut live: Preview<crate::preview::Live>, wanted: Vec<String>) -> Result<()> {
    let mut keys = Keys::start();
    if keys.is_some() {
        println!("{}", term::dim("a save reloads the previews · r reload · R restart · q quit"));
    }
    if let Some(keys) = keys.as_mut() {
        keys.raw();
    }
    let mut selected = false;
    let mut looked = Instant::now();
    let mut events = Vec::new();
    loop {
        let alive = live.poll(&mut events);
        for event in events.drain(..) {
            if let Event::Host(ToIde::Ready { previews, .. }) = &event
                && !selected
            {
                // the first Ready: select what was asked for, or everything
                let chosen: Vec<Selection> = previews
                    .iter()
                    .filter(|preview| wanted.is_empty() || wanted.iter().any(|name| *name == preview.ident || *name == preview.name))
                    .map(|preview| Selection { id: preview.id.clone(), ident: preview.ident.clone(), traits: Traits::default() })
                    .collect();
                if chosen.is_empty() {
                    cooked(&mut keys);
                    live.stop();
                    return Err(Error::new(format!("none of {wanted:?} is a preview of this app")).hint("`bunny preview --list` names them"));
                }
                let _ = live.send(&ToHost::Select(chosen));
                selected = true;
            }
            print_event(&event, &wanted);
        }
        if !alive {
            cooked(&mut keys);
            return Err(Error::new("the host ended"));
        }
        if looked.elapsed() >= LOOK {
            looked = Instant::now();
            if let Some(change) = live.saved() {
                cooked(&mut keys);
                match change {
                    crate::hot::Change::Code => live.reload(&mut events),
                    crate::hot::Change::Restart(why) => events.push(Event::RestartNeeded(why)),
                }
                if let Some(keys) = keys.as_mut() {
                    keys.raw();
                }
                if events.iter().any(|event| matches!(event, Event::RestartNeeded(_))) {
                    for event in events.drain(..) {
                        print_event(&event, &wanted);
                    }
                    live = restarted(live, &mut events)?;
                    selected = false;
                }
            }
        }
        let key = match keys.as_ref() {
            Some(keys) => keys.next(Duration::from_millis(50)),
            None => {
                std::thread::sleep(Duration::from_millis(50));
                None
            }
        };
        match key {
            Some('r') => {
                cooked(&mut keys);
                live.reload(&mut events);
                if let Some(keys) = keys.as_mut() {
                    keys.raw();
                }
            }
            Some('R') => {
                cooked(&mut keys);
                live = restarted(live, &mut events)?;
                selected = false;
                if let Some(keys) = keys.as_mut() {
                    keys.raw();
                }
            }
            Some('q') | Some(INTERRUPT) => {
                cooked(&mut keys);
                live.stop();
                println!("{}", term::dim("stopped"));
                if key == Some(INTERRUPT) {
                    std::process::exit(130);
                }
                return Ok(());
            }
            _ => {}
        }
    }
}

fn restarted(live: Preview<crate::preview::Live>, events: &mut Vec<Event>) -> Result<Preview<crate::preview::Live>> {
    println!("{}", term::bold("Restarting the host…"));
    match live.restart(events) {
        Step::Live(live) => Ok(live),
        Step::Failed(failed) => {
            for event in events.drain(..) {
                print_event(&event, &[]);
            }
            Err(Error::new(failed.why().to_string()))
        }
    }
}

fn render(events: &[Event], wanted: &[String]) {
    for event in events {
        print_event(event, wanted);
    }
}

fn print_event(event: &Event, _wanted: &[String]) {
    match event {
        Event::Previews(report) => {
            if report.previews.is_empty() {
                println!("{}", term::warn("no #[preview] functions under src/ — mark one and save"));
            }
        }
        Event::BuildStarted => println!("{}", term::dim("building…")),
        Event::Diagnostic(diagnostic) => {
            if diagnostic.level == "warning" || diagnostic.level == "error" {
                print!("{}", diagnostic.rendered);
            }
        }
        Event::Built { took } => println!("{}", term::dim(&format!("built in {}", seconds(*took)))),
        Event::BuildFailed { diagnostics } => {
            for diagnostic in diagnostics.iter().filter(|d| d.level == "error") {
                print!("{}", diagnostic.rendered);
            }
            println!("{}", term::dim("the host keeps the code it had: fix it and save"));
        }
        Event::Reloaded { took, new_types } => {
            let note = if *new_types { " · a type changed: the previews' state starts over" } else { "" };
            println!("{}{}", term::ok(&format!("Reloaded in {}", seconds(*took))), term::dim(note));
        }
        Event::RestartNeeded(why) => println!("{}", term::bold(&format!("{why}: restarting…"))),
        Event::Note(note) => println!("{}", term::warn(note)),
        Event::Crashed { code } => println!("{}", term::red(&format!("The host stopped ({}).", code.map_or_else(|| String::from("a signal ended it"), |code| format!("exit code {code}"))))),
        Event::Host(message) => match message {
            ToIde::Hello { framework, pid, .. } => println!("{}", term::dim(&format!("host pid {pid} · bunny-ui {framework}"))),
            ToIde::Ready { generation, previews, missing } => {
                println!("{}", term::dim(&format!("generation {generation}: {} preview(s)", previews.len())));
                for name in missing {
                    println!("{}", term::warn(&format!("`{name}` is in the sources but not in the build")));
                }
            }
            ToIde::Frame(frame) => println!(
                "{} {} {}",
                term::ok(&frame.preview),
                term::dim(&format!("v{}", frame.variant)),
                term::dim(&format!(
                    "{}×{} · {} commands · {} sites · {:.1} ms",
                    frame.size.width as i64,
                    frame.size.height as i64,
                    frame.commands.len(),
                    frame.sites.len(),
                    frame.metrics.layout_ms
                ))
            ),
            ToIde::Panicked { preview, message } => println!("{}", term::red(&format!("{preview} panicked: {message}"))),
            ToIde::Panic { message } => println!("{}", term::red(&format!("the host panicked: {message}"))),
            ToIde::Status { text } => println!("{}", term::dim(text)),
            ToIde::Resource(_) | ToIde::Snapshot { .. } | ToIde::Pong { .. } => {}
        },
    }
}

fn seconds(took: Duration) -> String {
    if took < Duration::from_secs(1) { format!("{}ms", took.as_millis()) } else { format!("{:.1}s", took.as_secs_f32()) }
}

fn cooked(keys: &mut Option<Keys>) {
    if let Some(keys) = keys.as_mut() {
        keys.cooked();
    }
}
