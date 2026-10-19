//! `bunny devices` and `bunny emulators`: where an app can run now, and
//! the simulators and emulators that can be started for it.

use crate::args::{HELP, Matches, Opt};
use crate::devices::{self, Device, Kind, State};
use crate::error::{Error, Result};
use crate::json;
use crate::term;

pub const DEVICES_SUMMARY: &str = "List where an app can run";

pub const DEVICES_USAGE: &str = "bunny devices [OPTIONS]";

pub const DEVICES_ABOUT: &str = "\
Lists this computer, the browser, the booted simulators and emulators, and the
connected phones — the ids `bunny run -d` takes. `--all` adds the ones that are
off or unreachable.";

pub const DEVICES_OPTIONS: &[Opt] = &[
    Opt::flag("all", "Also list devices that are off or unreachable").short('a'),
    Opt::flag("json", "Print the devices as JSON"),
    HELP,
];

pub const EMULATORS_SUMMARY: &str = "List and start simulators and emulators";

pub const EMULATORS_USAGE: &str = "bunny emulators [--launch <ID>]";

pub const EMULATORS_ABOUT: &str = "\
Lists the iOS simulators and Android emulators this machine can start; with
--launch, boots one and waits until it is ready.";

pub const EMULATORS_OPTIONS: &[Opt] = &[
    Opt::value("launch", "ID", "Boot this simulator or emulator (an id, a name, or ios / android)"),
    Opt::flag("json", "Print them as JSON"),
    HELP,
];

pub fn devices(matches: &Matches) -> Result<()> {
    let all = devices::discover();
    let shown: Vec<&Device> =
        all.iter().filter(|device| matches.flag("all") || device.state == State::Ready).collect();
    if matches.flag("json") {
        println!("{}", to_json(&shown));
        return Ok(());
    }
    table(&shown);
    let hidden = all.len() - shown.len();
    if hidden > 0 {
        println!();
        println!("{}", term::dim(&format!("{hidden} more off or unreachable: `bunny devices --all`, `bunny emulators`")));
    }
    Ok(())
}

pub fn emulators(matches: &Matches) -> Result<()> {
    let all = devices::discover();
    let bootable: Vec<&Device> =
        all.iter().filter(|device| matches!(device.kind, Kind::Simulator | Kind::Emulator)).collect();
    if let Some(query) = matches.value("launch") {
        let owned: Vec<Device> = bootable.iter().map(|device| (*device).clone()).collect();
        let device = devices::resolve(&owned, query)?;
        if device.state == State::Ready {
            println!("{}", term::ok(&format!("{} is already running", device.name)));
            return Ok(());
        }
        println!("Starting {}…", term::bold(&device.name));
        let ready = devices::boot(device)?;
        println!("{}", term::ok(&format!("{} is ready — `bunny run -d {}`", ready.name, ready.id)));
        return Ok(());
    }
    if matches.flag("json") {
        println!("{}", to_json(&bootable));
        return Ok(());
    }
    if bootable.is_empty() {
        return Err(Error::new("no simulator or emulator on this machine")
            .hint("`bunny doctor` says how to create one"));
    }
    table(&bootable);
    println!();
    println!("{}", term::dim("Start one: bunny emulators --launch <ID>"));
    Ok(())
}

fn table(devices: &[&Device]) {
    let rows: Vec<[String; 4]> = devices
        .iter()
        .map(|device| {
            let what = format!("{} {}", device.platform.title(), device.kind.word());
            let state = match &device.state {
                State::Ready => device.os.clone().unwrap_or_default(),
                State::Off => format!("{} · off", device.os.clone().unwrap_or_default()).trim_start_matches(" · ").to_string(),
                State::Unavailable(why) => why.clone(),
            };
            [device.name.clone(), device.id.clone(), what, state]
        })
        .collect();
    let widths: Vec<usize> = (0..3).map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0)).collect();
    for row in &rows {
        let pad = |text: &str, width: usize| format!("{text}{}", " ".repeat(width.saturating_sub(text.chars().count())));
        println!(
            "{}  {}  {}  {}",
            term::bold(&pad(&row[0], widths[0])),
            term::cyan(&pad(&row[1], widths[1])),
            pad(&row[2], widths[2]),
            term::dim(&row[3])
        );
    }
}

fn to_json(devices: &[&Device]) -> String {
    let items: Vec<String> = devices
        .iter()
        .map(|device| {
            let (state, why) = match &device.state {
                State::Ready => ("ready", None),
                State::Off => ("off", None),
                State::Unavailable(why) => ("unavailable", Some(why.as_str())),
            };
            format!(
                "{{\"id\":{},\"name\":{},\"platform\":\"{}\",\"kind\":\"{}\",\"state\":\"{state}\",\"os\":{},\"reason\":{}}}",
                json::quote(&device.id),
                json::quote(&device.name),
                device.platform.key(),
                device.kind.word(),
                device.os.as_deref().map_or(String::from("null"), json::quote),
                why.map_or(String::from("null"), json::quote),
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}
