//! Process-local AppKit appearance notifications; no global preferences change.
#![forbid(unsafe_code)]

#[cfg(target_os = "macos")]
const COMPLETE: &str = "Native appearance callbacks and surviving multi-window scenes passed.";

#[cfg(target_os = "macos")]
mod native {
    use bunny_ui::prelude::*;
    use bunny_ui::theme::{self, Appearance, Theme};
    use bunny_ui_macos::{App, WindowSpec};
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAppearance, NSAppearanceCustomization, NSApplication};
    use objc2_foundation::NSString;
    use std::{cell::Cell, rc::Rc, time::Duration};

    #[derive(Clone)]
    struct Reader(Rc<Cell<Option<Appearance>>>);
    impl Component for Reader {
        fn body(self) -> impl View {
            let appearance = theme::system_appearance();
            eprintln!("Scene appearance: {appearance:?}");
            self.0.set(appearance);
            text(format!("Observed {appearance:?}")).foreground_color(theme::fg())
        }
    }
    fn change(app: &NSApplication, name: &str) {
        let appearance = NSAppearance::appearanceNamed(&NSString::from_str(name)).unwrap();
        app.setAppearance(Some(&appearance));
        eprintln!(
            "Requested {name}; native application now {}",
            app.effectiveAppearance().name()
        );
    }
    async fn until(stage: &str, native: &NSApplication, mut ready: impl FnMut() -> bool) {
        for _ in 0..500 {
            if ready() {
                return;
            }
            task::sleep(Duration::from_millis(10)).await;
        }
        for window in native.windows().iter() {
            eprintln!(
                "At {stage}: window={} visible={} view-appearance={:?}",
                window.title(),
                window.isVisible(),
                window
                    .contentView()
                    .map(|view| view.effectiveAppearance().name())
            );
        }
        assert!(
            ready(),
            "{stage}: appearance did not reach the native scenes; core={:?}",
            theme::system_appearance()
        );
    }
    pub fn run() {
        let app = App::new();
        let native = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        let previous = native.appearance();
        change(&native, "NSAppearanceNameDarkAqua");
        let first_seen = Rc::new(Cell::new(None));
        let first = app.open(
            WindowSpec::titled("Appearance first"),
            Rc::new(app.runtime()),
            Reader(Rc::clone(&first_seen)),
        );
        assert_eq!(theme::system_appearance(), Some(Appearance::Dark));
        assert_eq!(first_seen.get(), Some(Appearance::Dark));
        assert_eq!(theme::current(), Theme::dark());
        println!("Initial native dark appearance reached the first frame");
        let second_seen = Rc::new(Cell::new(None));
        let second = app.open(
            WindowSpec::titled("Appearance second"),
            Rc::new(app.runtime()),
            Reader(Rc::clone(&second_seen)),
        );
        assert_eq!(second_seen.get(), Some(Appearance::Dark));
        let completed = Rc::new(Cell::new(false));
        let done = Rc::clone(&completed);
        let driver = app.clone();
        task::spawn(async move {
            change(&native, "NSAppearanceNameAqua");
            until("light transition", &native, || {
                first_seen.get() == Some(Appearance::Light)
                    && second_seen.get() == Some(Appearance::Light)
            })
            .await;
            assert_eq!(theme::current(), Theme::light());
            println!("Native light callback rebuilt both retained scenes");
            let custom = Theme {
                accent: bunny_ui::layout::Color::hex(0xAA3311),
                ..Theme::light()
            };
            theme::install(custom);
            // App appearance remains observable even when native windows pin their
            // own appearance and therefore receive no effective-view transition.
            let light =
                NSAppearance::appearanceNamed(&NSString::from_str("NSAppearanceNameAqua")).unwrap();
            for window in native.windows().iter() {
                window.setAppearance(Some(&light));
            }
            change(&native, "NSAppearanceNameAccessibilityHighContrastDarkAqua");
            until("high-contrast dark transition", &native, || {
                first_seen.get() == Some(Appearance::Dark)
                    && second_seen.get() == Some(Appearance::Dark)
            })
            .await;
            assert_eq!(theme::current(), custom);
            println!("High-contrast dark was observed without overwriting custom tokens");
            theme::follow_system();
            assert_eq!(theme::current(), Theme::dark());
            let third_seen = Rc::new(Cell::new(None));
            let third = driver.open(
                WindowSpec::titled("Appearance later"),
                Rc::new(driver.runtime()),
                Reader(Rc::clone(&third_seen)),
            );
            assert_eq!(third_seen.get(), Some(Appearance::Dark));
            driver.close(first);
            assert_eq!(driver.windows(), vec![second, third]);
            change(&native, "NSAppearanceNameAqua");
            until("surviving windows transition", &native, || {
                second_seen.get() == Some(Appearance::Light)
                    && third_seen.get() == Some(Appearance::Light)
            })
            .await;
            assert_eq!(theme::current(), Theme::light());
            let before = theme::version();
            // Duplicate application assignments leave the observation unchanged.
            change(&native, "NSAppearanceNameAqua");
            change(&native, "NSAppearanceNameAqua");
            task::sleep(Duration::from_millis(50)).await;
            assert_eq!(theme::version(), before);
            native.setAppearance(previous.as_deref());
            done.set(true);
            println!("{}", super::COMPLETE);
            driver.quit();
        })
        .detach();
        app.run();
        assert!(completed.get(), "fixture exited before all assertions");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    if std::env::var_os("BUNNY_APPEARANCE_CHILD").is_some() {
        native::run();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_APPEARANCE_CHILD", "1")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let output = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
            print!("{output}");
            assert!(status.success(), "native appearance regression: {status}");
            assert!(
                output.contains(COMPLETE),
                "appearance fixture exited before completing its assertions"
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native appearance regression timed out");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
