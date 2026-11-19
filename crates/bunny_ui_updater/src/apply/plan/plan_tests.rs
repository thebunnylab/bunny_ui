use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bunny_ui_release::Format;

use super::linux::{self, InstallKind, Layout, Manager};
use super::macos;
use super::windows::{self, Arg, HELPER_FLAG, HelperPlan, ImageState, Pacer};

// ---- macOS ----

#[test]
fn macos_takes_only_a_disk_image() {
    assert!(macos::accepts(Format::Dmg).is_ok());
    for wrong in [Format::App, Format::TarGz, Format::Zip, Format::Nsis] {
        assert!(macos::accepts(wrong).is_err(), "{wrong:?}");
    }
}

#[test]
fn the_rsync_source_ends_in_a_slash_so_it_replaces() {
    let argv = macos::rsync_argv(Path::new("/Volumes/x/App.app"), Path::new("/Applications/App.app"));
    assert!(argv.contains(&OsString::from("--delete")));
    assert!(argv.contains(&OsString::from("Icon?")), "the Finder's icon file stays out");
    assert!(argv.iter().any(|arg| arg == "/Volumes/x/App.app/"), "{argv:?}");
}

#[test]
fn a_mount_is_quiet_and_read_only() {
    let argv = macos::mount_argv(Path::new("/t/x.dmg"), Path::new("/t/mnt"));
    for expected in ["-nobrowse", "-readonly", "-noautoopen"] {
        assert!(argv.contains(&OsString::from(expected)), "{expected}");
    }
    assert_eq!(argv.last(), Some(&OsString::from("/t/x.dmg")));
}

#[test]
fn the_bundle_is_found_by_its_extension() {
    assert_eq!(macos::dotapp_in(&["Applications".into(), "App.APP".into()]), Some(PathBuf::from("App.APP")));
    assert_eq!(macos::dotapp_in(&["Applications".into()]), None);
}

#[test]
fn the_bundle_root_comes_from_the_executable() {
    assert_eq!(
        macos::bundle_root(Path::new("/Applications/App.app/Contents/MacOS/app")).as_deref(),
        Some(Path::new("/Applications/App.app"))
    );
    assert_eq!(macos::bundle_root(Path::new("/x/target/debug/app")), None);
}

// ---- Linux ----

#[test]
fn linux_takes_only_a_tarball() {
    assert!(linux::accepts(Format::TarGz).is_ok());
    for wrong in [Format::Dmg, Format::Nsis, Format::App, Format::AppImage, Format::Zip] {
        assert!(linux::accepts(wrong).is_err(), "{wrong:?}");
    }
}

#[test]
fn a_package_managed_install_is_refused() {
    for path in ["/usr/bin/app", "/usr/local/bin/app", "/bin/app", "/sbin/app"] {
        assert_eq!(linux::classify(Path::new(path), None, None), InstallKind::Managed(Manager::SystemPackage), "{path}");
    }
}

#[test]
fn a_store_install_is_refused_by_path_or_by_environment() {
    assert_eq!(linux::classify(Path::new("/snap/app/current/app"), None, None), InstallKind::Managed(Manager::Store));
    assert_eq!(linux::classify(Path::new("/anywhere/app"), None, Some("io.app.App")), InstallKind::Managed(Manager::Store));
}

#[test]
fn an_appimage_is_refused_before_anything_else() {
    assert_eq!(
        linux::classify(Path::new("/usr/bin/app"), Some("/home/me/App.AppImage"), None),
        InstallKind::Managed(Manager::Image)
    );
}

#[test]
fn a_home_install_is_the_apps_and_an_empty_variable_is_not_set() {
    for path in ["/home/me/.local/bin/app", "/home/me/.local/share/app/app", "/opt/app/app"] {
        assert_eq!(linux::classify(Path::new(path), None, None), InstallKind::SelfManaged, "{path}");
    }
    assert_eq!(linux::classify(Path::new("/opt/app/app"), Some(""), Some("")), InstallKind::SelfManaged);
}

#[test]
fn each_refusal_says_what_to_do_and_they_differ() {
    for manager in [Manager::SystemPackage, Manager::Store, Manager::Image] {
        assert!(!manager.advice().is_empty());
    }
    assert_ne!(Manager::SystemPackage.advice(), Manager::Image.advice());
}

#[test]
fn the_layout_is_flat_or_bunnys_prefix() {
    let flat = ["app".to_owned(), "lib".to_owned(), "lib/x.so".to_owned()];
    assert_eq!(linux::layout(&flat, "app"), Some(Layout::Flat));
    let prefix = ["bin".to_owned(), "bin/app".to_owned(), "share/applications/io.app.desktop".to_owned(), "install.sh".to_owned()];
    assert_eq!(linux::layout(&prefix, "app"), Some(Layout::Prefix));
    assert_eq!(linux::layout(&["share".to_owned()], "app"), None, "no binary, no install");
    assert_eq!(linux::layout(&["bin/other".to_owned()], "app"), None);
}

#[test]
fn the_one_top_folder_comes_off() {
    let entries = |list: &[&str]| list.iter().map(|&entry| entry.to_owned()).collect::<Vec<_>>();
    assert_eq!(linux::common_top(&entries(&["app", "app/bin/app", "app/share/x"])), Some("app".into()));
    assert_eq!(linux::common_top(&entries(&["app/bin/app"])), Some("app".into()));
    assert_eq!(linux::common_top(&entries(&["app", "lib/x"])), None, "two tops");
    assert_eq!(linux::common_top(&entries(&["app"])), None, "a lone binary is not a folder");
    assert_eq!(linux::common_top(&entries(&["application/x", "app/y"])), None, "a name prefix is not a folder");
    assert_eq!(linux::common_top(&[]), None);
}

#[test]
fn a_macs_extended_attribute_files_are_not_the_app() {
    assert!(linux::is_apple_double("app/._app"));
    assert!(linux::is_apple_double("._x"));
    assert!(!linux::is_apple_double("app/.config"));
    assert!(!linux::is_apple_double("app/a._b"));
}

#[test]
fn staging_lives_inside_the_install_folder() {
    let root = Path::new("/opt/app");
    let staging = linux::staging_dir(root, "42");
    assert!(staging.starts_with(root) && staging != root);
}

#[test]
fn the_binary_is_renamed_last_and_once() {
    let order = linux::swap_order("app", vec!["share".into(), "app".into(), "lib".into(), "lib".into()]);
    assert_eq!(order, ["lib", "share", "app"]);
}

// ---- Windows ----

fn plan(install_dir: &str, format: Format) -> HelperPlan {
    HelperPlan {
        artifact: PathBuf::from(r"C:\Temp\app-setup.exe"),
        target: PathBuf::from(format!(r"{install_dir}\app.exe")),
        install_dir: PathBuf::from(install_dir),
        format,
        sidecars: vec![PathBuf::from(format!(r"{install_dir}\tool.exe"))],
    }
}

struct FakePacer {
    now: Duration,
    pauses: Vec<Duration>,
}

impl Pacer for FakePacer {
    fn elapsed(&self) -> Duration {
        self.now
    }

    fn pause(&mut self, span: Duration) {
        self.pauses.push(span);
        self.now += span;
    }
}

fn pacer() -> FakePacer {
    FakePacer { now: Duration::ZERO, pauses: Vec::new() }
}

fn sharing_violation() -> io::Error {
    io::Error::from_raw_os_error(32)
}

#[test]
fn windows_takes_an_installer_or_a_zip() {
    for format in [Format::Nsis, Format::Wix, Format::Zip] {
        assert!(windows::accepts(format).is_ok(), "{format:?}");
    }
    for wrong in [Format::Dmg, Format::App, Format::TarGz, Format::AppImage] {
        assert!(windows::accepts(wrong).is_err(), "{wrong:?}");
    }
}

#[test]
fn the_handshake_round_trips_for_every_format() {
    for format in [Format::Nsis, Format::Wix, Format::Zip] {
        let original = plan(r"C:\Users\Some Name\AppData\Local\Programs\App", format);
        let mut argv = vec![OsString::from(r"C:\anything\app.exe")];
        argv.extend(windows::helper_argv(&original));
        assert_eq!(windows::parse_helper_args(argv), Some(original));
    }
}

#[test]
fn the_flag_comes_first_and_an_ordinary_launch_is_not_a_helper() {
    let argv = windows::helper_argv(&plan(r"C:\P", Format::Nsis));
    assert_eq!(argv[0], OsString::from(HELPER_FLAG));
    assert_eq!(windows::parse_helper_args([OsString::from("app.exe")]), None);
    assert_eq!(windows::parse_helper_args([OsString::from("app.exe"), OsString::from("--open"), OsString::from("x")]), None);
    // a truncated handshake is no handshake
    assert_eq!(windows::parse_helper_args(argv.into_iter().take(3)), None);
}

#[test]
fn nsis_puts_the_destination_last_and_never_quotes_it() {
    let plan = plan(r"C:\Users\Some Name\App", Format::Nsis);
    let unlocked = windows::await_images(&plan, Duration::ZERO, |_| ImageState::Free, &mut pacer()).unwrap();
    let (program, args) = windows::installer_command(&unlocked).unwrap();
    assert_eq!(program, plan.artifact);
    assert_eq!(args.last(), Some(&Arg::Raw(r"/D=C:\Users\Some Name\App".into())));
    assert_eq!(args[0], Arg::Quoted("/S".into()));
}

#[test]
fn an_msi_runs_through_msiexec_quoted_throughout() {
    let plan = plan(r"C:\P", Format::Wix);
    let unlocked = windows::await_images(&plan, Duration::ZERO, |_| ImageState::Free, &mut pacer()).unwrap();
    let (program, args) = windows::installer_command(&unlocked).unwrap();
    assert_eq!(program, PathBuf::from("msiexec"), "an .msi is not a program");
    assert!(args.iter().all(|arg| matches!(arg, Arg::Quoted(_))));
    assert!(args.contains(&Arg::Quoted(plan.artifact.to_string_lossy().into_owned())));
}

#[test]
fn a_zip_has_no_installer_the_helper_unpacks_it() {
    let plan = plan(r"C:\P", Format::Zip);
    let unlocked = windows::await_images(&plan, Duration::ZERO, |_| ImageState::Free, &mut pacer()).unwrap();
    assert!(windows::installer_command(&unlocked).is_none());
}

#[test]
fn the_plan_comes_from_the_running_executable() {
    // separators of the host, so the test means the same on every system
    let install = Path::new("programs").join("App");
    let derived = windows::HelperPlan::derive(
        &install.join("app.exe"),
        Path::new("a.zip"),
        Format::Zip,
        &["tool".into(), "helper".into()],
    )
    .unwrap();
    assert_eq!(derived.install_dir, install);
    assert_eq!(derived.sidecars, [install.join("tool.exe"), install.join("helper.exe")]);
    assert_eq!(derived.images().count(), 3);
    assert!(windows::HelperPlan::derive(Path::new("app.exe"), Path::new("a"), Format::Dmg, &[]).is_err());
}

#[test]
fn the_helper_never_runs_from_the_folder_it_replaces() {
    let helper = windows::helper_path(Path::new(r"C:\Temp"), "app", 77, "1.2.0");
    assert_eq!(helper, Path::new(r"C:\Temp").join("app-update-helper-77-1.2.0.exe"));
}

#[test]
fn the_sweep_takes_only_this_products_old_copies() {
    assert!(windows::is_sweepable_helper("app-update-helper-77-1.2.0.exe", "app", 7200));
    assert!(windows::is_sweepable_helper("app-update-helper-77-1.2.0.EXE", "app", 7200));
    assert!(!windows::is_sweepable_helper("app-update-helper-77-1.2.0.exe", "app", 60), "too new");
    assert!(!windows::is_sweepable_helper("other-update-helper-77-1.2.0.exe", "app", 7200), "another product's");
    assert!(!windows::is_sweepable_helper("app-update-helper-77.log", "app", 7200));
}

#[test]
fn a_held_sidecar_keeps_the_update_and_names_it() {
    let plan = plan(r"C:\P", Format::Nsis);
    let mut pacer = pacer();
    let held = windows::await_images(
        &plan,
        Duration::from_secs(windows::HELPER_DEADLINE_SECS),
        |image| if image.to_string_lossy().ends_with("tool.exe") { ImageState::Busy(sharing_violation()) } else { ImageState::Free },
        &mut pacer,
    )
    .unwrap_err();
    assert_eq!(held.busy().len(), 1);
    assert!(held.busy()[0].path.to_string_lossy().ends_with("tool.exe"));
    assert!(held.to_string().contains("tool.exe") && held.to_string().contains("app-setup.exe"), "{held}");
}

#[test]
fn an_absent_sidecar_is_not_waited_on() {
    let plan = plan(r"C:\P", Format::Nsis);
    let probe = |image: &Path| if image.to_string_lossy().ends_with("tool.exe") { ImageState::Absent } else { ImageState::Free };
    assert!(windows::await_images(&plan, Duration::ZERO, probe, &mut pacer()).is_ok());
}

#[test]
fn images_freed_before_the_deadline_proceed_and_the_pause_climbs_to_a_ceiling() {
    let plan = plan(r"C:\P", Format::Nsis);
    let mut rounds = 0;
    let mut pacer = pacer();
    let probe = |_: &Path| {
        rounds += 1;
        // two images a round: free from the eighth round on
        if rounds > 14 { ImageState::Free } else { ImageState::Busy(sharing_violation()) }
    };
    assert!(windows::await_images(&plan, Duration::from_secs(60), probe, &mut pacer).is_ok());
    let pauses: Vec<u64> = pacer.pauses.iter().map(|pause| pause.as_millis() as u64).collect();
    assert_eq!(pauses, [50, 100, 200, 400, 500, 500, 500]);
}

#[test]
fn an_open_says_what_it_found() {
    assert!(matches!(ImageState::from(Err(io::Error::from(io::ErrorKind::NotFound))), ImageState::Absent));
    assert!(matches!(ImageState::from(Err(sharing_violation())), ImageState::Busy(_)));
}

fn item(path: &str, bytes: Option<&[u8]>) -> bunny_ui_formats::zip::Item {
    bunny_ui_formats::zip::Item { path: path.into(), mode: None, bytes: bytes.map(<[u8]>::to_vec) }
}

#[test]
fn a_zip_update_loses_its_top_folder_and_keeps_its_files() {
    let files = windows::zip_files(
        vec![item("App/", None), item("App/app.exe", Some(b"MZ")), item("App/lib/x.dll", Some(b"x"))],
        "app.exe",
    )
    .unwrap();
    assert_eq!(files, [(PathBuf::from("app.exe"), b"MZ".to_vec()), (PathBuf::from("lib").join("x.dll"), b"x".to_vec())]);
    // with no top folder, the files are as they are; the case of the name
    // does not matter on Windows
    assert!(windows::zip_files(vec![item("APP.EXE", Some(b"MZ"))], "app.exe").is_ok());
}

#[test]
fn a_zip_update_without_the_app_or_with_an_escape_is_refused() {
    let no_app = windows::zip_files(vec![item("App/other.exe", Some(b"MZ"))], "app.exe").unwrap_err();
    assert!(no_app.to_string().contains("app.exe"), "{no_app}");
    let escape = windows::zip_files(vec![item("app.exe", Some(b"MZ")), item("../evil.dll", Some(b"x"))], "app.exe");
    assert!(escape.unwrap_err().to_string().contains("leaves the install folder"));
}
