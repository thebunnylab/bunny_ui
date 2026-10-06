//! The app's [`MenuBar`], arranged the way UIKit builds a main menu — the
//! iPad's menu bar, and the ⌘-hold sheet on any iPhone or iPad with a
//! keyboard. Pure, and here in the shared Apple half only so its tests run
//! on the mac: the iOS shell's FFI (`buildMenuWithBuilder:`) translates what
//! this decides and decides nothing itself.
//!
//! ## UIKit's rules
//!
//! UIKit hands an app a main menu already built — the application menu,
//! File, Edit, Format, View, Window, Help — and the app edits it through a
//! `UIMenuBuilder`. So an app's menu is FILED rather than appended:
//!
//! - **A menu UIKit already has takes the app's items at its end.** File,
//!   Edit, View, Window and Help keep their system rows (Edit's Undo, Cut,
//!   Copy and Paste; Window's own management), and the app's commands follow
//!   as their own groups. The standard edits are therefore never declared
//!   twice: an [`Item::Edit`] is UIKit's own Edit row, sent by its selector
//!   to the first responder, exactly as the mac's is.
//! - **A menu UIKit does not have stands after the one before it** — Go
//!   after View, Run after Go — in the bar's own order.
//! - **The roles go where UIKit keeps them.** The Settings command replaces
//!   the application menu's Preferences group, worded in the person's
//!   language ([`bunny_ui::words`]). About is the system's own row,
//!   kept when the app declares [`Item::About`] and removed when it does not.
//!   Quit is the SYSTEM's on this platform — an app does not end itself — so
//!   a [`Role::Quit`] command has no row here.
//! - **Format and Find go.** Format is a text system's (bold, fonts) that the
//!   scene does not run, and Find is UIKit's find-in-text; the app's own Find
//!   commands stand in Edit instead. A menu of rows nothing answers is noise,
//!   not parity.
//! - **A rule is a group.** UIKit draws its separator between inline groups,
//!   so the app's separators cut its items into groups, and an empty group is
//!   not built.
//!
//! ## Key commands
//!
//! An item gets a key command for a single stroke that holds ⌘ or ⌃, or for
//! F1–F12 — the strokes UIKit can name. A sequence (`⌘K ⌘S`) is shown with no
//! key, as on the mac; its command still runs from the menu, and from its keys
//! through the keymap.
//!
//! **Production gotcha: UIKit matches a key command BEFORE the view hears the
//! key.** The mac lets the key window's views answer a key equivalent first;
//! UIKit does not, so a `Save ⌘S` row would take the second stroke of a
//! pending `⌘K ⌘S`. The shell answers it at the other end: a chosen row first
//! offers its own stroke ([`UiCommand::stroke`]) to the keymap, and runs only
//! what the keymap declined. A binding scoped to a context, a pending chord
//! and a reader's rebind all keep their strokes.

use bunny_ui::action::{ActionId, Key, KeyPattern};
use bunny_ui::menu::{Item, Menu, MenuBar, MenuRole, Pick, Role, Shortcut};
use bunny_ui::words::{Word, Words};

// =============================================================================
// What UIKit will build
// =============================================================================

/// One of the menus UIKit's main menu already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Standard {
    File,
    Edit,
    View,
    Window,
    Help,
}

impl Standard {
    /// The standard menu a title names, if UIKit has one by that name.
    fn named(menu: &Menu) -> Option<Standard> {
        if menu.role() == Some(MenuRole::Help) {
            return Some(Standard::Help);
        }
        match menu.title() {
            "File" => Some(Standard::File),
            "Edit" => Some(Standard::Edit),
            "View" => Some(Standard::View),
            "Window" => Some(Standard::Window),
            "Help" => Some(Standard::Help),
            _ => None,
        }
    }
}

/// Where a menu of the app's goes in UIKit's main menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// Its groups at the end of one of UIKit's own menus.
    Into(Standard),
    /// A menu of its own, after the menu `after` names.
    After(Anchor),
}

/// A menu a new one stands after: UIKit's, or one of the app's built just
/// before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Standard(Standard),
    Own(String),
}

/// One menu, as the builder will file it.
#[derive(Clone, Debug, PartialEq)]
pub struct UiMenu {
    pub title: String,
    /// The identifier a menu of the app's own wears, so the next one can
    /// stand after it. UIKit's own menus keep theirs.
    pub identifier: String,
    pub place: Place,
    pub groups: Vec<Vec<UiItem>>,
}

/// One row of a group.
#[derive(Clone, Debug, PartialEq)]
pub enum UiItem {
    Command(UiCommand),
    /// A submenu, with its own groups.
    Submenu { title: String, groups: Vec<Vec<UiItem>> },
}

/// A command row: what it says, what the window runs, and the key UIKit
/// shows beside it, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct UiCommand {
    pub title: String,
    pub action: ActionId,
    pub key: Option<UiKey>,
    stroke: Option<KeyPattern>,
}

impl UiCommand {
    /// What the row runs in the window.
    pub fn pick(&self) -> Pick {
        Pick::Command(self.action)
    }

    /// The single stroke the row stands for — what the shell offers the
    /// keymap before it runs the row (the module's gotcha).
    pub fn stroke(&self) -> Option<KeyPattern> {
        self.stroke
    }
}

/// A key command's input and modifier flags (`UIKeyModifierFlags`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiKey {
    pub input: UiInput,
    pub modifiers: u64,
}

/// What a key command matches: a character, or one of UIKit's named keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiInput {
    Text(char),
    Named(NamedKey),
}

/// The keys UIKit names with an input constant (`UIKeyInputUpArrow`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKey {
    Up,
    Down,
    Left,
    Right,
    Escape,
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    /// F1 to F12, by number.
    F(u8),
}

/// `UIKeyModifierShift`, `…Control`, `…Alternate`, `…Command`.
const SHIFT: u64 = 1 << 17;
const CONTROL: u64 = 1 << 18;
const ALTERNATE: u64 = 1 << 19;
const COMMAND: u64 = 1 << 20;

impl UiKey {
    /// The key command UIKit can keep for `shortcut`, or `None`.
    fn of(shortcut: Option<&Shortcut>) -> Option<UiKey> {
        let stroke = shortcut?.single()?;
        let function = matches!(stroke.key, Key::F(1..=12));
        if !(stroke.command || stroke.control || function) {
            return None;
        }
        let input = match stroke.key {
            Key::Char(character) => UiInput::Text(character),
            Key::Enter => UiInput::Text('\r'),
            Key::Tab => UiInput::Text('\t'),
            Key::Backspace => UiInput::Text('\u{8}'),
            Key::Up => UiInput::Named(NamedKey::Up),
            Key::Down => UiInput::Named(NamedKey::Down),
            Key::Left => UiInput::Named(NamedKey::Left),
            Key::Right => UiInput::Named(NamedKey::Right),
            Key::Escape => UiInput::Named(NamedKey::Escape),
            Key::PageUp => UiInput::Named(NamedKey::PageUp),
            Key::PageDown => UiInput::Named(NamedKey::PageDown),
            Key::Home => UiInput::Named(NamedKey::Home),
            Key::End => UiInput::Named(NamedKey::End),
            Key::Delete => UiInput::Named(NamedKey::Delete),
            Key::F(number @ 1..=12) => UiInput::Named(NamedKey::F(number)),
            Key::F(_) => return None,
        };
        let modifiers = [
            (stroke.shift, SHIFT),
            (stroke.control, CONTROL),
            (stroke.option, ALTERNATE),
            (stroke.command, COMMAND),
        ]
        .into_iter()
        .filter_map(|(held, bit)| held.then_some(bit))
        .sum();
        Some(UiKey { input, modifiers })
    }
}

/// The whole main menu UIKit will build for the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiArrangement {
    pub menus: Vec<UiMenu>,
    /// The Settings command, standing in the application menu's
    /// Preferences group.
    pub settings: Option<UiCommand>,
    /// Is UIKit's own About row kept?
    pub about: bool,
}

// =============================================================================
// The arrangement
// =============================================================================

/// The app's bar, arranged as UIKit files a main menu — the module's rules.
pub fn arrange(bar: &MenuBar, words: &Words) -> UiArrangement {
    let mut arranged = UiArrangement::default();
    let mut previous = Anchor::Standard(Standard::View);
    for (index, menu) in bar.menus().iter().enumerate() {
        let groups = groups(menu, &mut arranged, words);
        let standard = Standard::named(menu);
        let identifier = match standard {
            Some(_) => String::new(),
            None => format!("bunny.menu.{index}"),
        };
        // a menu UIKit has keeps its own rows, so it stands even with no
        // groups of the app's; a new menu with nothing in it is not built —
        // and is no anchor either: the next menu standing after a menu that
        // was never built is a menu UIKit drops without a word
        let place = match standard {
            Some(standard) => Place::Into(standard),
            None if groups.is_empty() => continue,
            None => Place::After(previous.clone()),
        };
        previous = match standard {
            // a menu after it stands after UIKit's own, where the reader
            // meets it in the bar's order
            Some(standard) => Anchor::Standard(standard),
            None => Anchor::Own(identifier.clone()),
        };
        arranged.menus.push(UiMenu { title: menu.title().to_owned(), identifier, place, groups });
    }
    arranged
}

/// A menu's items cut into groups at its rules, with the roles lifted out.
fn groups(menu: &Menu, arranged: &mut UiArrangement, words: &Words) -> Vec<Vec<UiItem>> {
    let mut groups: Vec<Vec<UiItem>> = vec![Vec::new()];
    for item in menu.entries() {
        match item {
            Item::Separator => groups.push(Vec::new()),
            Item::About => arranged.about = true,
            // UIKit's own Edit rows, by the standard selectors
            Item::Edit(_) => {}
            Item::Command(command) => match command.filed_as() {
                Some(Role::Settings) if arranged.settings.is_none() => {
                    arranged.settings = Some(UiCommand {
                        title: words.get(Word::Settings).into_owned(),
                        action: command.action(),
                        key: UiKey::of(command.keys()),
                        stroke: command.keys().and_then(Shortcut::single),
                    });
                }
                // the system ends an app on this platform; the app does not
                Some(Role::Quit) => {}
                _ => {
                    if let Some(group) = groups.last_mut() {
                        group.push(UiItem::Command(UiCommand {
                            title: command.title().to_owned(),
                            action: command.action(),
                            key: UiKey::of(command.keys()),
                            stroke: command.keys().and_then(Shortcut::single),
                        }));
                    }
                }
            },
            Item::Submenu(submenu) => {
                let inner = self::groups(submenu, arranged, words);
                if !inner.is_empty()
                    && let Some(group) = groups.last_mut()
                {
                    group.push(UiItem::Submenu { title: submenu.title().to_owned(), groups: inner });
                }
            }
        }
    }
    groups.retain(|group| !group.is_empty());
    groups
}

#[cfg(test)]
mod tests {
    use bunny_ui::menu::{Command, Edit};

    use super::*;

    const OPEN: ActionId = ActionId("app.open");
    const SAVE: ActionId = ActionId("app.save");
    const SETTINGS: ActionId = ActionId("app.settings");
    const QUIT: ActionId = ActionId("app.quit");
    const BACK: ActionId = ActionId("app.back");
    const RUN: ActionId = ActionId("app.run");
    const KEYS: ActionId = ActionId("app.keys");
    const FIND: ActionId = ActionId("app.find");

    fn command(key: char) -> Shortcut {
        Shortcut::from(KeyPattern::command(Key::Char(key)))
    }

    fn bar() -> MenuBar {
        MenuBar::new()
            .menu(
                Menu::new("File")
                    .item(Command::new("Open…", OPEN).shortcut(command('o')))
                    .item(Command::new("Save", SAVE).shortcut(command('s')))
                    .separator()
                    .item(Command::new("Settings…", SETTINGS).shortcut(command(',')).role(Role::Settings))
                    .separator()
                    .item(Command::new("Quit", QUIT).shortcut(command('q')).role(Role::Quit)),
            )
            .menu(
                Menu::new("Edit")
                    .items(Edit::ALL)
                    .separator()
                    .item(Command::new("Find", FIND).shortcut(command('f'))),
            )
            .menu(Menu::new("Go").item(Command::new("Back", BACK)))
            .menu(Menu::new("Run").item(Command::new("Run", RUN).shortcut(Shortcut::from(KeyPattern::key(Key::F(5))))))
            .menu(
                Menu::help("Help")
                    .item(Command::new("Keyboard Shortcuts", KEYS).shortcut(Shortcut::sequence(&[
                        KeyPattern::command(Key::Char('k')),
                        KeyPattern::command(Key::Char('s')),
                    ])))
                    .separator()
                    .item(Item::About),
            )
    }

    fn titles(group: &[UiItem]) -> Vec<&str> {
        group
            .iter()
            .map(|item| match item {
                UiItem::Command(command) => command.title.as_str(),
                UiItem::Submenu { title, .. } => title.as_str(),
            })
            .collect()
    }

    #[test]
    fn a_menu_uikit_has_takes_the_apps_groups_and_keeps_its_own_rows() {
        let arranged = arrange(&bar(), &Words::english());
        let file = &arranged.menus[0];
        assert_eq!(file.place, Place::Into(Standard::File));
        assert_eq!(file.groups.len(), 1, "Settings and Quit left File; its rule went with them");
        assert_eq!(titles(&file.groups[0]), ["Open…", "Save"]);
        let edit = &arranged.menus[1];
        assert_eq!(edit.place, Place::Into(Standard::Edit));
        assert_eq!(edit.groups.len(), 1, "the six edits are UIKit's own rows, not the app's");
        assert_eq!(titles(&edit.groups[0]), ["Find"]);
    }

    #[test]
    fn a_menu_uikit_has_not_stands_after_the_one_before_it() {
        let arranged = arrange(&bar(), &Words::english());
        let go = &arranged.menus[2];
        assert_eq!(go.place, Place::After(Anchor::Standard(Standard::Edit)));
        let run = &arranged.menus[3];
        assert_eq!(run.place, Place::After(Anchor::Own(go.identifier.clone())), "Run after Go");
        assert_ne!(go.identifier, run.identifier);
        assert_eq!(arranged.menus[4].place, Place::Into(Standard::Help));
    }

    #[test]
    fn the_roles_go_where_uikit_keeps_them() {
        let arranged = arrange(&bar(), &Words::english());
        let settings = arranged.settings.as_ref().expect("Settings is the app's");
        assert_eq!(settings.title, "Settings…");
        assert_eq!(settings.action, SETTINGS);
        assert_eq!(settings.key, Some(UiKey { input: UiInput::Text(','), modifiers: COMMAND }));
        assert!(arranged.about, "the app declared About: UIKit's row stays");
        let quits = arranged
            .menus
            .iter()
            .flat_map(|menu| menu.groups.iter().flatten())
            .any(|item| matches!(item, UiItem::Command(command) if command.action == QUIT));
        assert!(!quits, "the system ends an app here; Quit has no row");
        assert!(!arrange(&MenuBar::new(), &Words::english()).about, "no About declared, none kept");
    }

    #[test]
    fn a_key_command_is_given_only_where_uikit_can_name_the_stroke() {
        let arranged = arrange(&bar(), &Words::english());
        let UiItem::Command(save) = &arranged.menus[0].groups[0][1] else { panic!("Save") };
        assert_eq!(save.key, Some(UiKey { input: UiInput::Text('s'), modifiers: COMMAND }));
        assert_eq!(save.stroke(), Some(KeyPattern::command(Key::Char('s'))), "the stroke the keymap is offered");
        let UiItem::Command(run) = &arranged.menus[3].groups[0][0] else { panic!("Run") };
        assert_eq!(run.key, Some(UiKey { input: UiInput::Named(NamedKey::F(5)), modifiers: 0 }));
        let UiItem::Command(back) = &arranged.menus[2].groups[0][0] else { panic!("Back") };
        assert_eq!((back.key.clone(), back.stroke()), (None, None), "unbound");
        let UiItem::Command(keys) = &arranged.menus[4].groups[0][0] else { panic!("Keys") };
        assert_eq!((keys.key.clone(), keys.stroke()), (None, None), "a sequence names no single key");
        let of = |pattern: KeyPattern| UiKey::of(Some(&Shortcut::from(pattern)));
        assert_eq!(of(KeyPattern::key(Key::Char('j'))), None, "a bare key would take typing");
        assert_eq!(
            of(KeyPattern::command_shift(Key::Up)),
            Some(UiKey { input: UiInput::Named(NamedKey::Up), modifiers: COMMAND | SHIFT }),
        );
        assert_eq!(of(KeyPattern::key(Key::F(13))), None, "UIKit names F1 to F12 only");
    }

    #[test]
    fn an_empty_menu_of_the_apps_is_not_built_and_a_submenu_keeps_its_groups() {
        let bar = MenuBar::new()
            .menu(Menu::new("Tools").separator())
            .menu(
                Menu::new("Go").item(
                    Menu::new("Recent").item(Command::new("Back", BACK)).separator().item(Command::new("Run", RUN)),
                ),
            );
        let arranged = arrange(&bar, &Words::english());
        assert_eq!(arranged.menus.len(), 1, "Tools held nothing");
        let UiItem::Submenu { title, groups } = &arranged.menus[0].groups[0][0] else { panic!("a submenu") };
        assert_eq!(title, "Recent");
        assert_eq!(groups.len(), 2, "its rule is a group too");
        assert_eq!(arranged.menus[0].place, Place::After(Anchor::Standard(Standard::View)));
    }
}
