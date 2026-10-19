//! Isolated real portal protocol and native scenes; not desktop-settings UI evidence.
#![forbid(unsafe_code)]
#[cfg(target_os = "linux")]
mod native {
    use bunny_ui::prelude::*;
    use bunny_ui::theme::{self, Appearance, Theme};
    use bunny_ui_linux::{App, WindowSpec};
    use std::{
        cell::Cell,
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        rc::Rc,
        time::Duration,
    };

    struct Portal {
        process: Child,
        name: String,
    }
    impl Portal {
        fn start() -> Self {
            let mut process = Command::new("/usr/bin/python3")
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/appearance_portal.py"
                ))
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut name = String::new();
            BufReader::new(process.stdout.take().unwrap())
                .read_line(&mut name)
                .unwrap();
            assert!(name.starts_with(':'));
            Self {
                process,
                name: name.trim().into(),
            }
        }
        fn control(&self, mode: &str, value: u32) {
            let result = Command::new("gdbus")
                .args([
                    "call",
                    "--session",
                    "--dest",
                    &self.name,
                    "--object-path",
                    "/org/freedesktop/portal/desktop",
                    "--method",
                    "org.bunnyui.AppearanceProbe.Control",
                    mode,
                    &value.to_string(),
                ])
                .output()
                .unwrap();
            assert!(result.status.success(), "{mode}: {:?}", result);
        }
    }
    impl Drop for Portal {
        fn drop(&mut self) {
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
    }
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Seen {
        appearance: Option<Appearance>,
        palette: Theme,
    }
    #[derive(Clone)]
    struct Reader(Rc<Cell<Option<Seen>>>);
    impl Component for Reader {
        fn body(self) -> impl View {
            self.0.set(Some(Seen {
                appearance: theme::system_appearance(),
                palette: theme::current(),
            }));
            text("Portal appearance").foreground_color(theme::fg())
        }
    }
    type LocalCheck<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>;
    fn until<'a>(
        stage: &'a str,
        cells: &'a [Rc<Cell<Option<Seen>>>],
        expected: Seen,
    ) -> LocalCheck<'a> {
        Box::pin(async move {
            for _ in 0..300 {
                if cells.iter().all(|cell| cell.get() == Some(expected)) {
                    println!(
                        "PORTAL_STAGE {stage} appearance={:?} scenes={}",
                        expected.appearance,
                        cells.len()
                    );
                    return;
                }
                task::sleep(Duration::from_millis(10)).await;
            }
            panic!(
                "{stage}: expected {expected:?}; got {:?}",
                cells.iter().map(|c| c.get()).collect::<Vec<_>>()
            );
        })
    }
    pub fn run() {
        bunny_ui_linux::drive::watchdog(35);
        let portal = Portal::start();
        let app = App::new();
        let first = Rc::new(Cell::new(None));
        let second = Rc::new(Cell::new(None));
        let one = app.open(
            WindowSpec::titled("Portal first"),
            Rc::new(app.runtime()),
            Reader(Rc::clone(&first)),
        );
        let _two = app.open(
            WindowSpec::titled("Portal second"),
            Rc::new(app.runtime()),
            Reader(Rc::clone(&second)),
        );
        let initial = Seen {
            appearance: Some(Appearance::Dark),
            palette: Theme::dark(),
        };
        assert_eq!(first.get(), Some(initial), "first native frame");
        assert_eq!(second.get(), Some(initial), "second native frame");
        let complete = Rc::new(Cell::new(false));
        let done = Rc::clone(&complete);
        let driver = app.clone();
        task::spawn(async move {
            let cells = [first, second];
            portal.control("set", 2);
            until(
                "light",
                &cells,
                Seen {
                    appearance: Some(Appearance::Light),
                    palette: Theme::light(),
                },
            )
            .await;
            let version = theme::version();
            portal.control("set", 2);
            for mode in ["type", "key", "path", "rogue"] {
                portal.control(mode, 1);
            }
            task::sleep(Duration::from_millis(100)).await;
            assert_eq!(
                theme::version(),
                version,
                "duplicate/malformed/unrelated signals are inert"
            );
            let custom = Theme {
                accent: Color::hex(0xAA2211),
                ..Theme::light()
            };
            theme::install(custom);
            portal.control("set", 1);
            until(
                "custom-dark",
                &cells,
                Seen {
                    appearance: Some(Appearance::Dark),
                    palette: custom,
                },
            )
            .await;
            theme::follow_system();
            until("follow-dark", &cells, initial).await;
            portal.control("set", 99);
            let absent = Seen {
                appearance: None,
                palette: Theme::light(),
            };
            until("unknown", &cells, absent).await;
            portal.control("set", 1);
            until("dark-again", &cells, initial).await;
            portal.control("release", 2);
            until("owner-lost", &cells, absent).await;
            portal.control("acquire", 2);
            until(
                "owner-returned",
                &cells,
                Seen {
                    appearance: Some(Appearance::Light),
                    palette: Theme::light(),
                },
            )
            .await;
            let third = Rc::new(Cell::new(None));
            let _three = driver.open(
                WindowSpec::titled("Portal later"),
                Rc::new(driver.runtime()),
                Reader(Rc::clone(&third)),
            );
            driver.close(one);
            let surviving = [Rc::clone(&cells[1]), third];
            portal.control("set", 1);
            until("survivors", &surviving, initial).await;
            portal.control("set", 0);
            until("no-preference", &surviving, absent).await;
            theme::install(custom);
            portal.control("set", 1);
            until(
                "before-disconnect",
                &surviving,
                Seen {
                    appearance: Some(Appearance::Dark),
                    palette: custom,
                },
            )
            .await;
            portal.control("disconnect", 0);
            until(
                "bus-disconnected",
                &surviving,
                Seen {
                    appearance: None,
                    palette: custom,
                },
            )
            .await;
            println!(
                "PORTAL_APPEARANCE passed backend={} physical_input=false",
                bunny_ui_linux::drive::backend()
            );
            done.set(true);
            driver.quit();
            drop(portal);
        })
        .detach();
        app.run();
        assert!(complete.get(), "fixture did not complete");
    }
}
fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("BUNNY_PORTAL_APPEARANCE_PROBE").is_some() {
        if std::env::args().any(|arg| arg == "--portal-child") {
            native::run();
        } else {
            // Each fixture owns its bus; the disconnect case must never stop
            // the enclosing test suite's session or a desktop session bus.
            let result = std::process::Command::new("dbus-run-session")
                .arg("--")
                .arg(std::env::current_exe().unwrap())
                .arg("--portal-child")
                .status()
                .unwrap();
            assert!(result.success(), "isolated portal fixture failed: {result}");
        }
        return;
    }
    println!(
        "Portal appearance requires isolated bus/display and BUNNY_PORTAL_APPEARANCE_PROBE=1; not run."
    );
}
