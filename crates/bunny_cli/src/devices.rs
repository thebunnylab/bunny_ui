//! Where an app can run: this computer, the browser, simulators,
//! emulators and phones — every source asked at once, each with a
//! deadline, so a phone that does not answer costs seconds, not the
//! command.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::json::{self, Value};
use crate::process;
use crate::toolchains::{android, QUICK};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Ios,
    Windows,
    Linux,
    Android,
    Web,
}

impl Platform {
    pub const ALL: [Platform; 6] =
        [Platform::Macos, Platform::Ios, Platform::Windows, Platform::Linux, Platform::Android, Platform::Web];

    /// The keyword: `-d ios`, `--platform android`.
    pub fn key(self) -> &'static str {
        match self {
            Platform::Macos => "macos",
            Platform::Ios => "ios",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Android => "android",
            Platform::Web => "web",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Platform::Macos => "macOS",
            Platform::Ios => "iOS",
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
            Platform::Android => "Android",
            Platform::Web => "Web",
        }
    }

    pub fn from_key(key: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|platform| platform.key() == key.to_ascii_lowercase())
    }

    /// The desktop this program runs on.
    pub fn host() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// What this host can build for: its own desktop, Android and the
    /// web everywhere, iOS on a Mac.
    pub fn buildable() -> Vec<Platform> {
        let mut list = vec![Platform::host()];
        if Platform::host() == Platform::Macos {
            list.push(Platform::Ios);
        }
        list.extend([Platform::Android, Platform::Web]);
        list
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Desktop,
    Browser,
    Simulator,
    Emulator,
    Physical,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Desktop => "desktop",
            Kind::Browser => "browser",
            Kind::Simulator => "simulator",
            Kind::Emulator => "emulator",
            Kind::Physical => "device",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// An app can be installed and started on it now.
    Ready,
    /// A simulator or an emulator that is shut down — it can be booted.
    Off,
    /// There, but not usable: why, in words.
    Unavailable(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// What `-d` takes: a UDID, an adb serial, an AVD's name, a platform.
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub kind: Kind,
    pub state: State,
    /// `iOS 27.0`, `Android 16`, when known.
    pub os: Option<String>,
}

/// Every device this machine can reach, ready or not.
pub fn discover() -> Vec<Device> {
    let env = android::Env::current();
    let sdk = android::sdk_root(&env);
    let mut devices = vec![host(), web()];
    thread::scope(|scope| {
        let simulators = scope.spawn(|| if cfg!(target_os = "macos") { ios_simulators() } else { Vec::new() });
        let phones = scope.spawn(|| if cfg!(target_os = "macos") { ios_physical() } else { Vec::new() });
        let androids = scope.spawn(|| sdk.as_deref().map(android_devices).unwrap_or_default());
        for handle in [phones, simulators, androids] {
            devices.extend(handle.join().unwrap_or_default());
        }
    });
    devices
}

fn host() -> Device {
    let platform = Platform::host();
    Device {
        id: platform.key().to_string(),
        name: format!("This {}", if platform == Platform::Macos { "Mac" } else { "computer" }),
        platform,
        kind: Kind::Desktop,
        state: State::Ready,
        os: None,
    }
}

fn web() -> Device {
    Device {
        id: String::from("web"),
        name: String::from("Browser"),
        platform: Platform::Web,
        kind: Kind::Browser,
        state: State::Ready,
        os: None,
    }
}

fn ios_simulators() -> Vec<Device> {
    match process::run("xcrun", &["simctl", "list", "devices", "available", "--json"], QUICK) {
        Ok(out) if out.ok() => parse_simctl(&out.stdout),
        _ => Vec::new(),
    }
}

/// The iOS simulators in `simctl list devices --json`.
pub fn parse_simctl(text: &str) -> Vec<Device> {
    let Ok(value) = json::parse(text) else { return Vec::new() };
    let mut devices = Vec::new();
    for (runtime, list) in value.get("devices").map(Value::members).unwrap_or_default() {
        let Some(version) = runtime.split("SimRuntime.iOS-").nth(1) else { continue };
        let version = version.replace('-', ".");
        for device in list.as_array() {
            if device.get("isAvailable").and_then(Value::as_bool) == Some(false) {
                continue;
            }
            let (Some(udid), Some(name)) = (device.str_at(&["udid"]), device.str_at(&["name"])) else { continue };
            devices.push(Device {
                id: udid.to_string(),
                name: name.to_string(),
                platform: Platform::Ios,
                kind: Kind::Simulator,
                state: if device.str_at(&["state"]) == Some("Booted") { State::Ready } else { State::Off },
                os: Some(format!("iOS {version}")),
            });
        }
    }
    devices
}

fn ios_physical() -> Vec<Device> {
    match process::run("xcrun", &["devicectl", "list", "devices", "--json-output", "-"], QUICK) {
        Ok(out) if out.ok() => parse_devicectl(&out.stdout),
        _ => Vec::new(),
    }
}

/// The physical iPhones and iPads in `devicectl list devices` — which
/// lists the simulators too, marked `simulated`.
pub fn parse_devicectl(text: &str) -> Vec<Device> {
    let Ok(value) = json::parse(text) else { return Vec::new() };
    let mut devices = Vec::new();
    for device in value.path(&["result", "devices"]).map(Value::as_array).unwrap_or_default() {
        let properties = device.get("properties");
        let at = |keys: &[&str]| properties.and_then(|p| p.str_at(keys));
        if at(&["hardware", "reality"]) == Some("simulated") {
            continue;
        }
        if !matches!(at(&["hardware", "platform"]), Some("iOS" | "iPadOS")) {
            continue;
        }
        let Some(udid) = at(&["hardware", "udid"]).or_else(|| device.str_at(&["hardwareProperties", "udid"])) else {
            continue;
        };
        let name = at(&["state", "name"])
            .or_else(|| device.str_at(&["deviceProperties", "name"]))
            .unwrap_or("iPhone")
            .to_string();
        let state = match at(&["connection", "state"]) {
            Some("connected") => State::Ready,
            Some(other) => State::Unavailable(format!("not connected ({other}): plug it in or join its Wi-Fi")),
            None => State::Unavailable(String::from("not connected")),
        };
        devices.push(Device {
            id: udid.to_string(),
            name,
            platform: Platform::Ios,
            kind: Kind::Physical,
            state,
            os: at(&["software", "osVersionNumber", "stringValue"]).map(|version| format!("iOS {version}")),
        });
    }
    devices
}

/// What `adb devices -l` lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdbEntry {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
}

pub fn parse_adb_devices(text: &str) -> Vec<AdbEntry> {
    text.lines()
        .skip_while(|line| !line.starts_with("List of devices attached"))
        .skip(1)
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next()?.to_string();
            let state = words.next()?.to_string();
            let model = words.find_map(|word| word.strip_prefix("model:")).map(|model| model.replace('_', " "));
            Some(AdbEntry { serial, state, model })
        })
        .collect()
}

fn android_devices(sdk: &Path) -> Vec<Device> {
    let adb = android::adb(sdk);
    let mut devices = Vec::new();
    let mut running_avds = Vec::new();
    let ask_adb = adb_may_start() || adb_server_running();
    if ask_adb
        && let Ok(out) = process::run(&adb, &["devices", "-l"], QUICK)
        && out.ok()
    {
        for entry in parse_adb_devices(&out.stdout) {
            let emulator = entry.serial.starts_with("emulator-");
            let state = match entry.state.as_str() {
                "device" => State::Ready,
                "unauthorized" => State::Unavailable(String::from("unauthorized: accept the prompt on the phone")),
                other => State::Unavailable(other.to_string()),
            };
            let mut name = entry.model.clone().unwrap_or_else(|| entry.serial.clone());
            let mut os = None;
            if state == State::Ready {
                os = adb_line(&adb, &entry.serial, &["shell", "getprop", "ro.build.version.release"])
                    .map(|release| format!("Android {release}"));
                if emulator && let Some(avd) = adb_line(&adb, &entry.serial, &["emu", "avd", "name"]) {
                    name = avd.replace('_', " ");
                    running_avds.push(avd);
                }
            }
            devices.push(Device {
                id: entry.serial,
                name,
                platform: Platform::Android,
                kind: if emulator { Kind::Emulator } else { Kind::Physical },
                state,
                os,
            });
        }
    }
    for avd in android::avds(&android::emulator(sdk)) {
        if !running_avds.contains(&avd) {
            devices.push(Device {
                name: avd.replace('_', " "),
                id: avd,
                platform: Platform::Android,
                kind: Kind::Emulator,
                state: State::Off,
                os: None,
            });
        }
    }
    devices
}

/// Whether asking adb may start its server. On Windows the server
/// inherits this process's open handles: a pipe a script reads `bunny`'s
/// output through would stay open — and the script wait — for as long as
/// the server lives. There `bunny` starts it only from a terminal, and
/// otherwise asks a server that is already running.
fn adb_may_start() -> bool {
    use std::io::IsTerminal;
    !cfg!(windows) || (std::io::stdout().is_terminal() && std::io::stderr().is_terminal())
}

/// An adb server answers on its port (`ANDROID_ADB_SERVER_PORT`, 5037 by
/// default).
fn adb_server_running() -> bool {
    let port = std::env::var("ANDROID_ADB_SERVER_PORT").ok().and_then(|port| port.parse::<u16>().ok()).unwrap_or(5037);
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// The first line of an `adb -s SERIAL …` answer.
fn adb_line(adb: &Path, serial: &str, args: &[&str]) -> Option<String> {
    let mut full = vec!["-s", serial];
    full.extend_from_slice(args);
    let out = process::run(adb, &full, QUICK).ok().filter(process::Output::ok)?;
    out.stdout.lines().next().map(|line| line.trim().to_string()).filter(|line| !line.is_empty())
}

/// The device `query` names: an exact id, an exact name, a platform
/// (its ready device, or the one to boot), or a part of one name.
pub fn resolve<'a>(devices: &'a [Device], query: &str) -> Result<&'a Device> {
    if let Some(device) = devices.iter().find(|device| device.id == query) {
        return Ok(device);
    }
    let lower = query.to_ascii_lowercase();
    if let Some(device) = devices.iter().find(|device| device.name.to_ascii_lowercase() == lower) {
        return Ok(device);
    }
    if let Some(platform) = Platform::from_key(&lower) {
        let of: Vec<&Device> = devices.iter().filter(|device| device.platform == platform).collect();
        let ready: Vec<&Device> = of.iter().copied().filter(|device| device.state == State::Ready).collect();
        return match ready.as_slice() {
            [one] => Ok(one),
            [] => default_to_boot(&of).ok_or_else(|| nothing_for(platform)),
            many => Err(ambiguous(query, many)),
        };
    }
    let matching: Vec<&Device> = devices
        .iter()
        .filter(|device| {
            device.name.to_ascii_lowercase().contains(&lower) || device.id.to_ascii_lowercase().contains(&lower)
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(one),
        [] => Err(Error::usage(format!("no device matches `{query}`")).hint("`bunny devices` lists them")),
        many => Err(ambiguous(query, many)),
    }
}

/// With nothing of a platform running, the one to boot: the iPhone on
/// the newest iOS, or the first emulator.
fn default_to_boot<'a>(of: &[&'a Device]) -> Option<&'a Device> {
    let off: Vec<&Device> = of.iter().copied().filter(|device| device.state == State::Off).collect();
    let version = |device: &Device| {
        device.os.as_deref().unwrap_or("").rsplit(' ').next().unwrap_or("").split('.').map(|part| part.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>()
    };
    let iphones: Vec<&Device> = off.iter().copied().filter(|device| device.name.starts_with("iPhone")).collect();
    let pool = if iphones.is_empty() { &off } else { &iphones };
    // newest runtime first; the list order breaks ties
    pool.iter().copied().rev().max_by_key(|device| version(device))
}

fn nothing_for(platform: Platform) -> Error {
    let error = Error::new(format!("no {} device or {} is available", platform.title(), match platform {
        Platform::Ios => "simulator",
        Platform::Android => "emulator",
        _ => "target",
    }));
    match platform {
        Platform::Ios => error.hint("create a simulator in Xcode (Window › Devices and Simulators), or plug in an iPhone"),
        Platform::Android => error.hint(format!(
            "create an emulator: avdmanager create avd -n bunny -k \"{}\" — or plug in a phone with USB debugging on",
            android::suggested_image()
        )),
        _ => error.hint(format!("{} apps build on a {} computer", platform.title(), platform.title())),
    }
}

fn ambiguous(query: &str, many: &[&Device]) -> Error {
    let list: Vec<String> = many.iter().map(|device| format!("{}  ({})", device.name, device.id)).collect();
    Error::usage(format!("`{query}` names more than one device")).hint(format!("pick one with -d:\n{}", list.join("\n")))
}

/// Boots a simulator or an emulator that is off, and answers it ready —
/// waiting at most a few minutes for the system to finish starting.
pub fn boot(device: &Device) -> Result<Device> {
    if device.state == State::Ready {
        return Ok(device.clone());
    }
    match (device.platform, device.kind, &device.state) {
        (Platform::Ios, Kind::Simulator, State::Off) => boot_simulator(device),
        (Platform::Android, Kind::Emulator, State::Off) => boot_emulator(device),
        (_, _, State::Unavailable(why)) => Err(Error::new(format!("{} is not available: {why}", device.name))),
        _ => Err(Error::new(format!("{} cannot be started from here", device.name))),
    }
}

fn boot_simulator(device: &Device) -> Result<Device> {
    // "already booted" is an error to simctl and a success here
    let _ = process::run("xcrun", &["simctl", "boot", &device.id], QUICK);
    let _ = process::run("open", &["-a", "Simulator", "--args", "-CurrentDeviceUDID", &device.id], QUICK);
    let out = process::run("xcrun", &["simctl", "bootstatus", &device.id, "-b"], Duration::from_secs(180))
        .map_err(|error| Error::new(format!("simctl: {error}")))?;
    if !out.ok() {
        return Err(Error::new(format!("{} did not finish booting: {}", device.name, out.stderr.trim())));
    }
    Ok(Device { state: State::Ready, ..device.clone() })
}

fn boot_emulator(device: &Device) -> Result<Device> {
    let env = android::Env::current();
    let sdk = android::sdk_root(&env).ok_or_else(|| Error::new("no Android SDK found").hint("`bunny doctor` says how to install one"))?;
    let adb = android::adb(&sdk);
    let taken: Vec<String> = process::run(&adb, &["devices"], QUICK)
        .map(|out| parse_adb_devices(&out.stdout).into_iter().map(|entry| entry.serial).collect())
        .unwrap_or_default();
    // the console port names the serial before the emulator is up
    let port = (5554..=5682)
        .step_by(2)
        .find(|port| !taken.contains(&format!("emulator-{port}")))
        .ok_or_else(|| Error::new("every emulator port is taken"))?;
    spawn_detached(
        &android::emulator(&sdk),
        &["-avd", &device.id, "-port", &port.to_string(), "-no-snapshot-save", "-no-boot-anim"],
    )
    .map_err(|error| Error::new(format!("the emulator did not start: {error}")))?;
    let serial = format!("emulator-{port}");
    let out = process::run(&adb, &["-s", &serial, "wait-for-device"], Duration::from_secs(120))
        .map_err(|error| Error::new(format!("adb: {error}")))?;
    if !out.ok() {
        return Err(Error::new(format!("{} did not come up in two minutes", device.name)));
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(300) {
        if adb_line(&adb, &serial, &["shell", "getprop", "sys.boot_completed"]).as_deref() == Some("1") {
            return Ok(Device { id: serial, state: State::Ready, ..device.clone() });
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err(Error::new(format!("{} did not finish booting in five minutes", device.name)))
}

/// Starts a program that outlives this one — an emulator stays up after
/// `bunny` exits, as Android Studio leaves it.
pub fn spawn_detached<S: AsRef<std::ffi::OsStr>>(program: &Path, args: &[S]) -> std::io::Result<()> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, platform: Platform, kind: Kind, state: State, os: Option<&str>) -> Device {
        Device { id: id.into(), name: name.into(), platform, kind, state, os: os.map(String::from) }
    }

    #[test]
    fn simulators_from_simctl() {
        let text = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0":[
              {"udid":"AAA","isAvailable":true,"state":"Shutdown","name":"iPhone 18 Pro"},
              {"udid":"BBB","isAvailable":true,"state":"Booted","name":"iPad Air"}],
            "com.apple.CoreSimulator.SimRuntime.watchOS-13-0":[
              {"udid":"CCC","isAvailable":true,"state":"Shutdown","name":"Watch"}]}}"#;
        let devices = parse_simctl(text);
        assert_eq!(devices.len(), 2, "the watch is not iOS");
        assert_eq!(devices[0].state, State::Off);
        assert_eq!(devices[1].state, State::Ready);
        assert_eq!(devices[0].os.as_deref(), Some("iOS 27.0"));
    }

    #[test]
    fn phones_from_devicectl_without_the_simulators() {
        let text = r#"{"result":{"devices":[
          {"identifier":"4D7E","properties":{"connection":{"state":"unavailable","pairingState":"paired"},
            "hardware":{"platform":"iOS","udid":"00008160-00021C3001400036","cpuType":{"subtype":18446744071562067980}},
            "software":{"osVersionNumber":{"stringValue":"27.0"}},"state":{"name":"Deco's iPhone"}}},
          {"identifier":"2AAD","properties":{"connection":{"state":"disconnected"},
            "hardware":{"platform":"iOS","udid":"2AAD","reality":"simulated"},"state":{"name":"iPhone 17"}}},
          {"identifier":"9F00","properties":{"connection":{"state":"connected"},
            "hardware":{"platform":"iOS","udid":"00008120-0001"},"state":{"name":"Work iPhone"}}}]}}"#;
        let devices = parse_devicectl(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "Deco's iPhone");
        assert!(matches!(devices[0].state, State::Unavailable(_)));
        assert_eq!(devices[0].os.as_deref(), Some("iOS 27.0"));
        assert_eq!(devices[1].state, State::Ready);
        assert_eq!(devices[1].kind, Kind::Physical);
    }

    #[test]
    fn adb_lists_serials_states_and_models() {
        let text = "* daemon started successfully\nList of devices attached\n\
                    emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\n\
                    R5CT1234567            unauthorized usb:1-1 transport_id:2\n\n";
        let entries = parse_adb_devices(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].model.as_deref(), Some("sdk gphone64 arm64"));
        assert_eq!(entries[1].state, "unauthorized");
    }

    #[test]
    fn a_query_finds_one_device() {
        let devices = vec![
            device("macos", "This Mac", Platform::Macos, Kind::Desktop, State::Ready, Some("macOS")),
            device("web", "Browser", Platform::Web, Kind::Browser, State::Ready, None),
            device("AAA", "iPhone 17", Platform::Ios, Kind::Simulator, State::Off, Some("iOS 26.0")),
            device("BBB", "iPhone 18 Pro", Platform::Ios, Kind::Simulator, State::Off, Some("iOS 27.0")),
            device("CCC", "iPad Air", Platform::Ios, Kind::Simulator, State::Off, Some("iOS 27.0")),
            device("Pixel_9", "Pixel 9", Platform::Android, Kind::Emulator, State::Off, None),
        ];
        assert_eq!(resolve(&devices, "BBB").unwrap().id, "BBB");
        assert_eq!(resolve(&devices, "ipad air").unwrap().id, "CCC");
        assert_eq!(resolve(&devices, "ios").unwrap().id, "BBB", "the iPhone on the newest iOS");
        assert_eq!(resolve(&devices, "android").unwrap().id, "Pixel_9");
        assert_eq!(resolve(&devices, "pixel").unwrap().id, "Pixel_9");
        assert_eq!(resolve(&devices, "iphone").unwrap_err().code, 2, "two iPhones: ambiguous");
        assert!(resolve(&devices, "nokia").is_err());
        assert!(resolve(&devices, "windows").is_err());
    }

    #[test]
    fn two_ready_devices_of_a_platform_must_be_named() {
        let devices = vec![
            device("A", "iPhone 18", Platform::Ios, Kind::Simulator, State::Ready, Some("iOS 27.0")),
            device("B", "My iPhone", Platform::Ios, Kind::Physical, State::Ready, Some("iOS 27.0")),
        ];
        let error = resolve(&devices, "ios").unwrap_err();
        assert!(error.hint.unwrap().contains("My iPhone  (B)"));
    }
}
