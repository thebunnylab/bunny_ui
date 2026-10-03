//! The app's menu bar as Android's Keyboard Shortcuts Helper lists it —
//! pure, so the Mac runs the tests the phone cannot.
//!
//! A phone has no menu bar. With a hardware keyboard, Meta+/ opens the
//! system's helper, which asks the activity for its groups
//! (`Activity.onProvideKeyboardShortcuts`) and draws them in its own
//! sheet. This turns the bar every other shell draws in its own manner
//! ([`bunny_ui::menu`]) into those groups: one per menu, under the menu's
//! title, and one row per item whose shortcut the keyboard can type.
//!
//! ## What the helper cannot list, said once
//!
//! - **A sequence** (`Ctrl+K Ctrl+S`): a `KeyboardShortcutInfo` holds one
//!   key and its modifiers.
//! - **A stroke that holds `control`**: on Android, Ctrl is the
//!   accelerator and arrives as `command` ([`crate::keys::held`]), and
//!   Meta belongs to the system, so no key types `control`.
//! - **A key with no Android key code**: a character outside the keys a
//!   latin layout names. The punctuation codes are named after the US
//!   positions (`KEYCODE_SLASH`), and the helper labels a chord by its
//!   code, so on another layout a punctuation chord can read as its US
//!   key. Letters, digits and the named keys read the same everywhere.
//! - **Quit**: a phone app is left by the system, never by a row.
//!
//! ## The wire
//!
//! The activity reads the groups over JNI as one string, a line each: a
//! group is `G\t<title>`, an item under it `K\t<label>\t<keycode>\t<meta>`.
//! A tab or a newline inside a label is a space, so the format cannot be
//! broken by a title.

use bunny_ui::action::{Key, KeyPattern};
use bunny_ui::menu::{Item, MenuBar, Role};

use crate::keys;

/// One menu's chords, under its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: String,
    pub chords: Vec<Chord>,
}

/// One row of the helper: what the item is called, the key, and the
/// modifiers held (Android's `META_*` bits).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub label: String,
    pub keycode: i32,
    pub meta: i32,
}

// android/input.h — the modifiers a chord may hold
const META_SHIFT_ON: i32 = 0x01;
const META_ALT_ON: i32 = 0x02;
const META_CTRL_ON: i32 = 0x1000;

/// The helper's groups for `bar`, in the bar's order. A menu with no chord
/// the keyboard can type is left out — the helper draws no empty sheets.
#[must_use]
pub fn groups(bar: &MenuBar) -> Vec<Group> {
    bar.menus()
        .iter()
        .filter_map(|menu| {
            let mut chords = Vec::new();
            collect(menu.entries(), &mut chords);
            (!chords.is_empty()).then(|| Group { title: menu.title().to_owned(), chords })
        })
        .collect()
}

/// The rows `items` give, a submenu's folded into its menu's.
fn collect(items: &[Item], chords: &mut Vec<Chord>) {
    for item in items {
        match item {
            Item::Command(command) if command.filed_as() == Some(Role::Quit) => {}
            Item::Command(command) => {
                if let Some(chord) = command.keys().and_then(|keys| keys.single()).and_then(|stroke| chord(command.title(), stroke)) {
                    chords.push(chord);
                }
            }
            Item::Edit(edit) => chords.extend(chord(edit.title(), edit.stroke())),
            Item::Submenu(menu) => collect(menu.entries(), chords),
            Item::About | Item::Separator => {}
        }
    }
}

/// The helper's row for one stroke — `None` when the keyboard cannot type
/// it (see the module doc).
fn chord(label: &str, stroke: KeyPattern) -> Option<Chord> {
    if stroke.control {
        return None;
    }
    let meta = [(stroke.command, META_CTRL_ON), (stroke.option, META_ALT_ON), (stroke.shift, META_SHIFT_ON)]
        .into_iter()
        .filter_map(|(held, bit)| held.then_some(bit))
        .fold(0, |meta, bit| meta | bit);
    Some(Chord { label: label.to_owned(), keycode: keycode(stroke.key)?, meta })
}

/// Android's key code for `key` — the inverse of [`keys::key_pattern`].
fn keycode(key: Key) -> Option<i32> {
    Some(match key {
        Key::Up => keys::KEYCODE_DPAD_UP,
        Key::Down => keys::KEYCODE_DPAD_DOWN,
        Key::Left => keys::KEYCODE_DPAD_LEFT,
        Key::Right => keys::KEYCODE_DPAD_RIGHT,
        Key::Enter => keys::KEYCODE_ENTER,
        Key::Escape => keys::KEYCODE_ESCAPE,
        Key::Tab => keys::KEYCODE_TAB,
        Key::PageUp => keys::KEYCODE_PAGE_UP,
        Key::PageDown => keys::KEYCODE_PAGE_DOWN,
        Key::Backspace => keys::KEYCODE_DEL,
        Key::Delete => keys::KEYCODE_FORWARD_DEL,
        Key::Home => keys::KEYCODE_MOVE_HOME,
        Key::End => keys::KEYCODE_MOVE_END,
        // the platform names twelve
        Key::F(n @ 1..=12) => keys::KEYCODE_F1 + i32::from(n) - 1,
        Key::Char(c @ 'a'..='z') => keys::KEYCODE_A + offset(c, 'a')?,
        Key::Char(c @ '0'..='9') => KEYCODE_0 + offset(c, '0')?,
        Key::Char(c) => punctuation(c)?,
        _ => return None,
    })
}

// android/keycodes.h — the digits and the punctuation a latin layout names
const KEYCODE_0: i32 = 7;

/// How far `c` sits past `first` — the codes of a run of letters or digits
/// are consecutive.
fn offset(c: char, first: char) -> Option<i32> {
    i32::try_from(u32::from(c).checked_sub(u32::from(first))?).ok()
}

/// The punctuation keys Android names, by the character their US key
/// types with no modifier.
fn punctuation(c: char) -> Option<i32> {
    Some(match c {
        ',' => 55,
        '.' => 56,
        ' ' => 62,
        '`' => 68,
        '-' => 69,
        '=' => 70,
        '[' => 71,
        ']' => 72,
        '\\' => 73,
        ';' => 74,
        '\'' => 75,
        '/' => 76,
        _ => return None,
    })
}

/// The bar the app last set, on the wire — what the activity's helper is
/// answered with. One per process, as the app has one bar.
static KEPT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Keeps `bar`'s groups for the helper's next question.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn keep(bar: &MenuBar) {
    let wire = encode(&groups(bar));
    if let Ok(mut kept) = KEPT.lock() {
        *kept = wire;
    }
}

/// The groups the helper is answered with: the last bar kept, or nothing
/// before the app set one.
#[must_use]
pub fn kept() -> String {
    KEPT.lock().map(|kept| kept.clone()).unwrap_or_default()
}

/// The groups on the wire the activity's Java reads (see the module doc).
#[must_use]
pub fn encode(groups: &[Group]) -> String {
    let clean = |text: &str| text.replace(['\t', '\n', '\r'], " ");
    groups
        .iter()
        .flat_map(|group| {
            std::iter::once(format!("G\t{}", clean(&group.title))).chain(
                group
                    .chords
                    .iter()
                    .map(|chord| format!("K\t{}\t{}\t{}", clean(&chord.label), chord.keycode, chord.meta)),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use bunny_ui::action::{ActionId, Key, KeyPattern};
    use bunny_ui::menu::{Command, Edit, Menu, MenuBar, Role, Shortcut};

    use super::*;

    const SAVE: ActionId = ActionId("app.save");
    const SETTINGS: ActionId = ActionId("app.settings");
    const QUIT: ActionId = ActionId("app.quit");
    const PALETTE: ActionId = ActionId("app.palette");
    const KEYMAP: ActionId = ActionId("app.keymap");
    const DEFINITION: ActionId = ActionId("app.definition");
    const BACK: ActionId = ActionId("app.back");

    fn ctrl(c: char) -> KeyPattern {
        KeyPattern { key: Key::Char(c), shift: false, command: true, option: false, control: false }
    }

    fn bar() -> MenuBar {
        MenuBar::new()
            .menu(
                Menu::new("File")
                    .item(Command::new("Save", SAVE).shortcut(Shortcut::sequence(&[ctrl('s')])))
                    .item(Command::new("Settings…", SETTINGS).shortcut(Shortcut::sequence(&[ctrl(',')])).role(Role::Settings))
                    .item(Command::new("Quit", QUIT).shortcut(Shortcut::sequence(&[ctrl('q')])).role(Role::Quit)),
            )
            .menu(Menu::new("Edit").item(Edit::Copy))
            .menu(
                Menu::new("View")
                    .item(
                        Command::new("Command Palette…", PALETTE)
                            .shortcut(Shortcut::sequence(&[KeyPattern { shift: true, ..ctrl('p') }])),
                    )
                    // a sequence has no row in the helper
                    .item(Command::new("Keyboard Shortcuts", KEYMAP).shortcut(Shortcut::sequence(&[ctrl('k'), ctrl('s')]))),
            )
            .menu(
                Menu::new("Go")
                    .item(
                        Command::new("Go to Definition", DEFINITION)
                            .shortcut(Shortcut::sequence(&[KeyPattern { key: Key::F(12), command: false, ..ctrl('x') }])),
                    )
                    // `control` is not a key an Android keyboard types
                    .item(
                        Command::new("Back", BACK)
                            .shortcut(Shortcut::sequence(&[KeyPattern { command: false, control: true, ..ctrl('-') }])),
                    ),
            )
            .menu(Menu::help("Help").item(bunny_ui::menu::Item::About))
    }

    /// Every chord the keyboard can type is in its menu's group, in the
    /// bar's order, with Android's key code and meta bits; Quit, a sequence,
    /// a `control` stroke and a menu with nothing to type are left out.
    #[test]
    fn the_helper_lists_each_menus_typeable_chords_under_its_title() {
        let groups = groups(&bar());
        let titles: Vec<&str> = groups.iter().map(|group| group.title.as_str()).collect();
        assert_eq!(titles, ["File", "Edit", "View", "Go"], "Help holds nothing to type");

        let row = |label: &str| {
            groups
                .iter()
                .flat_map(|group| group.chords.iter())
                .find(|chord| chord.label == label)
                .cloned()
        };
        assert_eq!(row("Save"), Some(Chord { label: "Save".into(), keycode: 47, meta: META_CTRL_ON }));
        assert_eq!(row("Settings…").map(|chord| chord.keycode), Some(55), "the comma key");
        assert_eq!(row("Copy"), Some(Chord { label: "Copy".into(), keycode: 31, meta: META_CTRL_ON }));
        assert_eq!(
            row("Command Palette…").map(|chord| chord.meta),
            Some(META_CTRL_ON | META_SHIFT_ON)
        );
        assert_eq!(row("Go to Definition"), Some(Chord { label: "Go to Definition".into(), keycode: 142, meta: 0 }));
        assert_eq!(row("Quit"), None, "a phone app is left by the system");
        assert_eq!(row("Keyboard Shortcuts"), None, "a sequence has no row");
        assert_eq!(row("Back"), None, "no key types control");
    }

    /// The wire is a line per group and per chord, and a label cannot break
    /// it.
    #[test]
    fn the_wire_is_a_line_per_group_and_chord() {
        let groups = vec![Group {
            title: "Fi\tle".into(),
            chords: vec![Chord { label: "Save\nAll".into(), keycode: 47, meta: META_CTRL_ON }],
        }];
        assert_eq!(encode(&groups), "G\tFi le\nK\tSave All\t47\t4096");
    }

    /// Every chord the helper lists reads back, through the shell's own
    /// key mapping, as the stroke the bar declared — the two tables agree.
    #[test]
    fn a_listed_chord_reads_back_as_the_stroke_the_bar_declared() {
        for (stroke, keycode) in [(ctrl('s'), 47), (ctrl('0'), 7), (ctrl('/'), 76)] {
            let chord = chord("x", stroke).expect("typeable");
            assert_eq!(chord.keycode, keycode);
            let typed = match stroke.key {
                Key::Char(c) => c.to_string(),
                _ => String::new(),
            };
            let back = keys::key_pattern(&keys::KeyStroke {
                keycode: chord.keycode,
                shift: chord.meta & META_SHIFT_ON != 0,
                control: chord.meta & META_CTRL_ON != 0,
                alt: chord.meta & META_ALT_ON != 0,
                meta: false,
                chars_ignoring: typed,
                typed: None,
            });
            assert_eq!(back, Some(stroke));
        }
    }
}
