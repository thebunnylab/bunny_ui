//! The menu bar's own ruler: a window with a menu bar, driven through the
//! road AppKit gives a key event, and the bar read back — no hand on the
//! keyboard and no accessibility grant.
//!
//! ```sh
//! cargo run -p bunny-ui-macos --example menu_probe
//! ```
//!
//! It opens one window (it takes the focus for the few seconds it runs),
//! prints what it found, and exits non-zero when a check fails. The
//! clipboard it writes is put back as it was.
//!
//! What it holds:
//! 1. The bar the mac draws: the app menu first, Window before Help, the
//!    key equivalents the arrangement promised, and an item nothing
//!    answers drawn disabled.
//! 2. The keymap before the menu. A key event takes AppKit's order — the
//!    key window's views, then the main menu, then `keyDown:` — and `⌘K ⌘S`
//!    reaches its sequence even though the File menu holds `Save ⌘S`.
//! 3. An edit item on the field that holds the keyboard: `⌘A ⌘C` reach the
//!    Edit menu, and the clipboard holds the field's text. `⌘Z` there is
//!    the dark Undo's: AppKit keeps a disabled item's stroke and beeps.
//! 4. A command chosen from the bar runs its action, and Quit runs the app's
//!    quit command while the window answers it.
//!
//! Each step runs from the main queue, between events, as AppKit's own
//! dispatch would — never from inside the shell's handler.

#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]

#[cfg(target_os = "macos")]
mod probe {
    use std::cell::{Cell, RefCell};
    use std::ffi::{CString, c_char, c_void};
    use std::rc::Rc;

    use bunny_ui::action::{ActionId, Key, KeyPattern};
    use bunny_ui::menu::{Command, Edit, Item, Menu, MenuBar, Role, Shortcut};
    use bunny_ui::prelude::*;
    use bunny_ui_apple::ffi::{Id, Sel, text_argument_to_string};
    use bunny_ui_macos::{App, CoreTextEngine, WindowSpec};

    const SAVE: ActionId = ActionId("probe.save");
    const OPEN: ActionId = ActionId("probe.open");
    const KEYS: ActionId = ActionId("probe.keys");
    const SETTINGS: ActionId = ActionId("probe.settings");
    const QUIT: ActionId = ActionId("probe.quit");
    const FIELD: &str = "hello menu";

    #[allow(clashing_extern_declarations)]
    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        #[link_name = "objc_msgSend"]
        fn msg_id(obj: Id, sel: Sel) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_id_i64(obj: Id, sel: Sel, a: i64) -> Id;
        #[link_name = "objc_msgSend"]
        fn msg_i64(obj: Id, sel: Sel) -> i64;
        #[link_name = "objc_msgSend"]
        fn msg_u64(obj: Id, sel: Sel) -> u64;
        #[link_name = "objc_msgSend"]
        fn msg_bool(obj: Id, sel: Sel) -> i8;
        #[link_name = "objc_msgSend"]
        fn msg_bool_id(obj: Id, sel: Sel, a: Id) -> i8;
        #[link_name = "objc_msgSend"]
        fn msg_void(obj: Id, sel: Sel);
        #[link_name = "objc_msgSend"]
        fn msg_void_id(obj: Id, sel: Sel, a: Id);
        #[link_name = "objc_msgSend"]
        fn msg_void_i64(obj: Id, sel: Sel, a: i64);
        #[link_name = "objc_msgSend"]
        fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
        #[link_name = "objc_msgSend"]
        #[allow(clippy::too_many_arguments)]
        fn msg_key_event(
            class: Id,
            sel: Sel,
            kind: u64,
            location: Point,
            flags: u64,
            timestamp: f64,
            window: i64,
            context: Id,
            characters: Id,
            bare: Id,
            repeat: i8,
            code: u16,
        ) -> Id;
    }

    /// `NSPoint`, by value.
    #[repr(C)]
    struct Point {
        x: f64,
        y: f64,
    }

    /// libdispatch's main queue, an object only ever named by address.
    #[repr(C)]
    struct Queue {
        _opaque: [u8; 0],
    }

    #[link(name = "System", kind = "dylib")]
    unsafe extern "C" {
        static _dispatch_main_q: Queue;
        fn dispatch_time(when: u64, delta: i64) -> u64;
        fn dispatch_after_f(
            when: u64,
            queue: *const Queue,
            context: *mut c_void,
            work: extern "C" fn(*mut c_void),
        );
    }

    fn class(name: &str) -> Id {
        let name = CString::new(name).expect("a class name");
        unsafe { objc_getClass(name.as_ptr()) }
    }

    fn sel(name: &str) -> Sel {
        let name = CString::new(name).expect("a selector");
        unsafe { sel_registerName(name.as_ptr()) }
    }

    fn string(text: &str) -> Id {
        let text = CString::new(text).expect("no NUL");
        unsafe { msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr()) }
    }

    fn app() -> Id {
        unsafe { msg_id(class("NSApplication"), sel("sharedApplication")) }
    }

    /// What the probe counted, and what it found.
    struct Probe {
        saved: Rc<Cell<i32>>,
        keys: Rc<Cell<i32>>,
        quits: Rc<Cell<i32>>,
        failures: Cell<u32>,
        clipboard: RefCell<Option<String>>,
        step: Cell<usize>,
    }

    thread_local! {
        static PROBE: RefCell<Option<Rc<Probe>>> = const { RefCell::new(None) };
    }

    fn probe() -> Rc<Probe> {
        PROBE.with(|slot| slot.borrow().clone()).expect("the probe is armed")
    }

    fn check(what: &str, held: bool) {
        println!("{} {what}", if held { "  ok  " } else { "  FAIL" });
        if !held {
            let probe = probe();
            probe.failures.set(probe.failures.get() + 1);
        }
    }

    // =========================================================================
    // The bar, read back
    // =========================================================================

    /// One item as the bar holds it: title, key equivalent with its mask,
    /// enabled.
    #[derive(Debug)]
    struct Read {
        title: String,
        key: String,
        mask: u64,
        enabled: bool,
        hidden: bool,
    }

    fn menu_items(menu: Id) -> Vec<Read> {
        unsafe {
            msg_void(menu, sel("update"));
            (0..msg_i64(menu, sel("numberOfItems")))
                .map(|index| {
                    let item = msg_id_i64(menu, sel("itemAtIndex:"), index);
                    let separator = msg_bool(item, sel("isSeparatorItem")) != 0;
                    Read {
                        title: if separator {
                            "─".to_owned()
                        } else {
                            text_argument_to_string(msg_id(item, sel("title")))
                        },
                        key: text_argument_to_string(msg_id(item, sel("keyEquivalent"))),
                        mask: msg_u64(item, sel("keyEquivalentModifierMask")),
                        enabled: msg_bool(item, sel("isEnabled")) != 0,
                        hidden: msg_bool(item, sel("isHidden")) != 0
                            || msg_bool(item, sel("isAlternate")) != 0,
                    }
                })
                .collect()
        }
    }

    fn bar() -> Vec<(String, Id)> {
        unsafe {
            let main = msg_id(app(), sel("mainMenu"));
            (0..msg_i64(main, sel("numberOfItems")))
                .map(|index| {
                    let item = msg_id_i64(main, sel("itemAtIndex:"), index);
                    (
                        text_argument_to_string(msg_id(item, sel("title"))),
                        msg_id(item, sel("submenu")),
                    )
                })
                .collect()
        }
    }

    fn menu_named(title: &str) -> Id {
        bar()
            .into_iter()
            .find(|(name, _)| name == title)
            .map(|(_, menu)| menu)
            .unwrap_or(std::ptr::null_mut())
    }

    fn find(items: &[Read], title: &str) -> Option<usize> {
        items.iter().position(|item| item.title == title)
    }

    // =========================================================================
    // A key event, the way AppKit routes one
    // =========================================================================

    /// The probe's window, by AppKit's own list.
    fn window() -> Id {
        unsafe { msg_id_i64(msg_id(app(), sel("windows")), sel("objectAtIndex:"), 0) }
    }

    /// Who took the stroke: the window's views (the keymap's road), the main
    /// menu (a key equivalent), or `keyDown:` after both declined — the order
    /// `NSApplication` gives a key event.
    fn press(code: u16, character: char, flags: u64) -> &'static str {
        unsafe {
            let window = window();
            let text = string(&character.to_string());
            let event = msg_key_event(
                class("NSEvent"),
                sel(
                    "keyEventWithType:location:modifierFlags:timestamp:windowNumber:context:characters:charactersIgnoringModifiers:isARepeat:keyCode:",
                ),
                10, // NSEventTypeKeyDown
                Point { x: 0.0, y: 0.0 },
                flags,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |d| d.as_secs_f64()),
                msg_i64(window, sel("windowNumber")),
                std::ptr::null_mut(),
                text,
                text,
                0,
                code,
            );
            if msg_bool_id(window, sel("performKeyEquivalent:"), event) != 0 {
                return "views";
            }
            if msg_bool_id(msg_id(app(), sel("mainMenu")), sel("performKeyEquivalent:"), event) != 0
            {
                return "menu";
            }
            msg_void_id(window, sel("sendEvent:"), event);
            "keyDown"
        }
    }

    const COMMAND: u64 = 1 << 20;

    // =========================================================================
    // The steps
    // =========================================================================

    fn next(after_ms: i64) {
        unsafe {
            let when = dispatch_time(0, after_ms * 1_000_000);
            dispatch_after_f(when, &raw const _dispatch_main_q, std::ptr::null_mut(), step);
        }
    }

    extern "C" fn step(_: *mut c_void) {
        let probe = probe();
        let at = probe.step.get();
        probe.step.set(at + 1);
        match at {
            0 => read_the_bar(),
            1 => {
                println!("the keymap before the menu");
                let first = press(40, 'k', COMMAND);
                let second = press(1, 's', COMMAND);
                check(
                    &format!("⌘K ⌘S is the keymap's sequence ({first}, {second})"),
                    (first, second) == ("views", "views"),
                );
                check(
                    "…and reached its action, not the File menu's Save",
                    probe.keys.get() == 1 && probe.saved.get() == 0,
                );
                let save = press(1, 's', COMMAND);
                check(
                    &format!("⌘S alone is the keymap's too ({save})"),
                    save == "views" && probe.saved.get() == 1,
                );
            }
            2 => {
                println!("an edit item on the field");
                *probe.clipboard.borrow_mut() = bunny_ui::clipboard::read();
                let all = press(0, 'a', COMMAND);
                let copy = press(8, 'c', COMMAND);
                check(
                    &format!("⌘A and ⌘C reach the Edit menu ({all}, {copy})"),
                    (all, copy) == ("menu", "menu"),
                );
            }
            3 => {
                let copied = bunny_ui::clipboard::read();
                check(
                    &format!("the clipboard holds the field's text ({copied:?})"),
                    copied.as_deref() == Some(FIELD),
                );
                if let Some(previous) = probe.clipboard.borrow().as_deref() {
                    bunny_ui::clipboard::write(previous);
                }
                // a dark item's key equivalent is still the menu's: AppKit
                // keeps the stroke and sounds the system beep, as TextEdit
                // does with nothing to undo
                let undo = press(6, 'z', COMMAND);
                check(
                    &format!("⌘Z over a field is the dark Undo's, and runs nothing ({undo})"),
                    undo == "menu",
                );
            }
            4 => {
                println!("commands chosen from the bar");
                let file = menu_named("File");
                let items = menu_items(file);
                if let Some(index) = find(&items, "Save") {
                    unsafe {
                        msg_void_i64(file, sel("performActionForItemAtIndex:"), index as i64)
                    };
                }
            }
            5 => {
                check("File ▸ Save ran its action", probe.saved.get() == 2);
                let app_menu = bar().first().map(|(_, menu)| *menu).unwrap_or(std::ptr::null_mut());
                let items = menu_items(app_menu);
                if let Some(index) = items.iter().position(|item| item.title.starts_with("Quit ")) {
                    unsafe {
                        msg_void_i64(app_menu, sel("performActionForItemAtIndex:"), index as i64)
                    };
                }
            }
            _ => {
                check("Quit ran the app's quit command", probe.quits.get() == 1);
                let failures = probe.failures.get();
                println!(
                    "{}",
                    if failures == 0 {
                        "menu probe: every check held".to_owned()
                    } else {
                        format!("menu probe: {failures} check(s) failed")
                    }
                );
                std::process::exit(i32::from(failures != 0));
            }
        }
        next(350);
    }

    fn read_the_bar() {
        println!("the bar");
        let titles: Vec<String> = bar().into_iter().map(|(title, _)| title).collect();
        println!("        {}", titles.join(" | "));
        check(
            "File, Edit, Window and Help, the Window menu before Help",
            titles.get(1..).is_some_and(|rest| rest == ["File", "Edit", "Window", "Help"]),
        );
        let app_menu = bar().first().map(|(_, menu)| *menu).unwrap_or(std::ptr::null_mut());
        let items = menu_items(app_menu);
        for item in &items {
            println!(
                "        {:<22} {:>4} {:#x} {}",
                item.title,
                item.key.escape_default(),
                item.mask,
                if item.enabled { "" } else { "(off)" }
            );
        }
        let at = |title: &str| find(&items, title);
        check("the app menu: About, Settings…, Services, Hide, Hide Others, Show All, Quit", {
            let order = ["Settings…", "Services", "Hide Others", "Show All"].map(at);
            items.first().is_some_and(|item| item.title.starts_with("About "))
                && order.iter().all(Option::is_some)
                && order.windows(2).all(|pair| pair[0] < pair[1])
                && items.last().is_some_and(|item| item.title.starts_with("Quit "))
        });
        check(
            "Settings… carries the app's ⌘,",
            at("Settings…").is_some_and(|i| items[i].key == "," && items[i].mask == COMMAND),
        );
        check("Quit is enabled", items.last().is_some_and(|item| item.enabled));
        let file = menu_items(menu_named("File"));
        for item in &file {
            println!(
                "        File ▸ {:<15} {:>4} {:#x} {}",
                item.title,
                item.key.escape_default(),
                item.mask,
                if item.enabled { "" } else { "(off)" }
            );
        }
        check(
            "File holds Open… and Save, and nothing filed elsewhere",
            file.iter().map(|i| i.title.as_str()).eq(["Open…", "Save"]),
        );
        check(
            "Open… is dark: nothing answers it",
            find(&file, "Open…").is_some_and(|i| !file[i].enabled),
        );
        check(
            "Save is lit, with ⌘S",
            find(&file, "Save").is_some_and(|i| file[i].enabled && file[i].key == "s"),
        );
        let edit = menu_items(menu_named("Edit"));
        for item in &edit {
            println!(
                "        Edit ▸ {:<20} {:>4} {:#x} {}{}",
                item.title,
                item.key.escape_default(),
                item.mask,
                if item.enabled { "" } else { "(off)" },
                if item.hidden { " (hidden or alternate)" } else { "" },
            );
        }
        check(
            "Copy and Paste are lit over the field",
            ["Copy", "Paste"].iter().all(|t| find(&edit, t).is_some_and(|i| edit[i].enabled)),
        );
        check("Undo is dark over a field", find(&edit, "Undo").is_some_and(|i| !edit[i].enabled));
        let help = menu_items(menu_named("Help"));
        check(
            "a sequence shows no key equivalent",
            find(&help, "Keyboard Shortcuts").is_some_and(|i| help[i].key.is_empty()),
        );
    }

    // =========================================================================
    // The window
    // =========================================================================

    #[derive(Clone, Copy)]
    struct Page {
        note: State<String>,
    }

    impl Component for Page {
        fn body(self) -> impl View {
            vstack!(
                text("The menu bar's ruler").font(Font::Title),
                text_field("the field the edits reach", self.note.binding()).auto_focus(),
            )
            .padding()
        }
    }

    pub(crate) fn run() {
        let app = App::new();
        // the checks read the mac's own words back in English, whatever
        // language the system speaks
        app.set_locale(Some(Locale::new("en")));
        let runtime = app.runtime().text_engine(Rc::new(CoreTextEngine::new()));
        let probe = Rc::new(Probe {
            saved: Rc::new(Cell::new(0)),
            keys: Rc::new(Cell::new(0)),
            quits: Rc::new(Cell::new(0)),
            failures: Cell::new(0),
            clipboard: RefCell::new(None),
            step: Cell::new(0),
        });
        let count = |cell: &Rc<Cell<i32>>| {
            let cell = Rc::clone(cell);
            move || cell.set(cell.get() + 1)
        };
        runtime.on_action(SAVE, count(&probe.saved));
        runtime.on_action(KEYS, count(&probe.keys));
        runtime.on_action(QUIT, count(&probe.quits));
        runtime.on_action(SETTINGS, || {});
        runtime.bind(KeyPattern::command(Key::Char('s')), SAVE);
        runtime.bind_sequence(
            &[KeyPattern::command(Key::Char('k')), KeyPattern::command(Key::Char('s'))],
            KEYS,
        );
        PROBE.with(|slot| *slot.borrow_mut() = Some(probe));

        let stroke = |key| Shortcut::from(KeyPattern::command(Key::Char(key)));
        app.set_menu_bar(
            &MenuBar::new()
                .menu(
                    Menu::new("File")
                        .item(Command::new("Open…", OPEN).shortcut(stroke('o')))
                        .item(Command::new("Save", SAVE).shortcut(stroke('s')))
                        .separator()
                        .item(
                            Command::new("Settings", SETTINGS)
                                .shortcut(stroke(','))
                                .role(Role::Settings),
                        )
                        .separator()
                        .item(Command::new("Quit", QUIT).shortcut(stroke('q')).role(Role::Quit)),
                )
                .menu(Menu::new("Edit").items(Edit::ALL))
                .menu(
                    Menu::help("Help")
                        .item(Command::new("Keyboard Shortcuts", KEYS).shortcut(
                            Shortcut::sequence(&[
                                KeyPattern::command(Key::Char('k')),
                                KeyPattern::command(Key::Char('s')),
                            ]),
                        ))
                        .separator()
                        .item(Item::About),
                ),
        );
        app.open(
            WindowSpec::titled("Menu probe").size(420.0, 160.0),
            Rc::new(runtime),
            Page { note: State::new(FIELD.to_owned()) },
        );
        next(900);
        app.run();
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    probe::run();
}
