//! The menu bar, as the mac draws it: the app's [`MenuBar`] arranged by the
//! mac's own rules, before a single `NSMenu` exists.
//!
//! The arrangement is pure — a bar and the app's name in, the menus the mac
//! will show out — so every rule below is a unit test, and the FFI that
//! builds the `NSMenu`s ([`crate::ffi::install_menu_bar`]) only translates.
//!
//! ## The mac's rules (Apple's HIG, "The menu bar")
//!
//! - **The app menu comes first, and it is the shell's.** About, the
//!   app's Settings, Services, Hide, Hide Others, Show All and Quit, in that
//!   order. The app's [`Item::About`] and its [`Role::Settings`] and
//!   [`Role::Quit`] commands are lifted out of wherever the app declared
//!   them, titled in the mac's words — in the person's language, through
//!   [`bunny_ui::words`], the app's name where the language puts it — and
//!   filed here; the rest of the app menu is the platform's and the app
//!   never declares it.
//! - **Quit never goes dark.** With no quit command declared, the item is
//!   the system's `terminate:`; with one, the command runs while the window
//!   answers it and `terminate:` runs when it does not.
//! - **The Window menu is the shell's too.** Minimize, Zoom and Bring All to
//!   Front, with AppKit's own window list under them. It stands before the
//!   Help menu, or last when there is none.
//! - **A rule never stands alone.** A group moved to the app menu leaves no
//!   gap: no menu begins or ends with a separator or shows two in a row, and
//!   a menu left with nothing is not drawn.
//! - **A key equivalent is drawn only where the mac can keep it**: a single
//!   stroke that holds ⌘ or ⌃, or a function key. A sequence (`⌘K ⌘S`) has
//!   no key equivalent in AppKit, and an item that owned a bare key would
//!   take it from a field that declined it. Either is shown without one —
//!   the command still runs by its keys, because the keymap hears every
//!   stroke before the menu does.
//!
//! ## Production gotchas
//!
//! - **A dark item's key equivalent is still the menu's.** AppKit keeps the
//!   stroke of a disabled item and sounds the system beep — `⌘Z` over a field
//!   with no history beeps, as it does in TextEdit. That is safe only because
//!   the keymap heard the stroke first: the menu receives nothing but what
//!   every binding declined.
//! - **AppKit grows the Edit menu by itself**: Writing Tools, AutoFill, Start
//!   Dictation and Emoji & Symbols, the last two with hidden alternates for
//!   the globe key. They are found by the standard edit selectors, which is
//!   one more reason the edits are those selectors and not commands.
//! - **Shift rides the mask, never the character's case.** A key equivalent
//!   is the key's own lowercase character with shift in the modifier mask,
//!   which is how the keymap spells a stroke too. For a shifted punctuation
//!   key AppKit compares the SHIFTED character, so such an item draws right
//!   and matches only through the keymap — which is where the stroke goes
//!   first anyway.

use std::rc::Rc;

use bunny_ui::action::{ActionId, Key, KeyPattern};
use bunny_ui::menu::{Command, Edit, Item, Menu, MenuBar, MenuRole, Role, Shortcut};
use bunny_ui::words::{Word, Words};

// =============================================================================
// What the mac will build
// =============================================================================

/// One menu of the bar, as the mac draws it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NativeMenu {
    pub(crate) title: Rc<str>,
    pub(crate) kind: MenuKind,
    pub(crate) lines: Vec<Line>,
}

/// Which of the mac's menus a menu is, where AppKit wants to be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuKind {
    /// The first, named for the app.
    App,
    /// One of the app's own.
    Plain,
    /// `setWindowsMenu:` — AppKit keeps the window list here.
    Window,
    /// `setHelpMenu:` — AppKit gives it the search field.
    Help,
}

/// One line of a menu.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Line {
    /// A command the app's window runs, through the shell's menu target.
    Command {
        title: Rc<str>,
        action: ActionId,
        key: Option<KeyEquivalent>,
    },
    /// The way out: the app's quit command while the window answers it, and
    /// the system's `terminate:` otherwise (or always, with none declared).
    Quit {
        title: Rc<str>,
        action: Option<ActionId>,
        key: Option<KeyEquivalent>,
    },
    /// A standard edit, sent by its selector to whoever is first responder —
    /// the framework's view, or a hosted page that answers for itself. The
    /// title is the framework's word for it, in the person's language.
    Edit {
        edit: Edit,
        title: Rc<str>,
        key: Option<KeyEquivalent>,
    },
    /// An item AppKit answers itself, by selector, up the responder chain.
    System {
        title: Rc<str>,
        selector: &'static str,
        key: Option<KeyEquivalent>,
    },
    /// The Services submenu, which AppKit fills, under its title.
    Services {
        title: Rc<str>,
    },
    /// A menu opening from this line.
    Submenu(NativeMenu),
    Separator,
}

/// A key equivalent as `NSMenuItem` takes it: the key's character and the
/// modifier mask (`NSEventModifierFlags`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyEquivalent {
    pub(crate) key: char,
    pub(crate) mask: u64,
}

/// `NSEventModifierFlagShift`, `…Control`, `…Option`, `…Command`.
const SHIFT: u64 = 1 << 17;
const CONTROL: u64 = 1 << 18;
const OPTION: u64 = 1 << 19;
const COMMAND: u64 = 1 << 20;

impl KeyEquivalent {
    /// The key equivalent the mac can keep for `shortcut`, or `None` — see
    /// the module's rules.
    pub(crate) fn of(shortcut: Option<&Shortcut>) -> Option<KeyEquivalent> {
        KeyEquivalent::of_stroke(shortcut?.single()?)
    }

    fn of_stroke(stroke: KeyPattern) -> Option<KeyEquivalent> {
        let function = matches!(stroke.key, Key::F(_));
        if !(stroke.command || stroke.control || function) {
            return None;
        }
        let mask = [
            (stroke.shift, SHIFT),
            (stroke.control, CONTROL),
            (stroke.option, OPTION),
            (stroke.command, COMMAND),
        ]
        .into_iter()
        .filter_map(|(held, bit)| held.then_some(bit))
        .sum();
        Some(KeyEquivalent { key: key_character(stroke.key)?, mask })
    }
}

/// The character AppKit files a key under: the key's own for a printable
/// one, and the ASCII control or `NS…FunctionKey` code point for the rest.
fn key_character(key: Key) -> Option<char> {
    Some(match key {
        Key::Char(character) => character,
        Key::Enter => '\r',
        Key::Escape => '\u{1b}',
        Key::Tab => '\t',
        // the mac's "delete" is NSDeleteCharacter; forward delete is a
        // function key
        Key::Backspace => '\u{7f}',
        Key::Delete => '\u{f728}',
        Key::Up => '\u{f700}',
        Key::Down => '\u{f701}',
        Key::Left => '\u{f702}',
        Key::Right => '\u{f703}',
        Key::Home => '\u{f729}',
        Key::End => '\u{f72b}',
        Key::PageUp => '\u{f72c}',
        Key::PageDown => '\u{f72d}',
        // NSF1FunctionKey (U+F704) onwards, F1 to F35 in a row
        Key::F(number @ 1..=24) => char::from_u32(0xF704 + u32::from(number) - 1)?,
        Key::F(_) => return None,
    })
}

// =============================================================================
// The arrangement
// =============================================================================

/// What the mac lifts out of the app's menus and files in its app menu —
/// the first of each; a second stays where the app declared it.
#[derive(Default)]
struct Filed<'bar> {
    about: bool,
    settings: Option<&'bar Command>,
    quit: Option<&'bar Command>,
}

/// The app's bar, arranged as the mac draws it. `app` is the name the menu
/// bar shows in bold (the bundle's), which the app menu's items repeat.
pub(crate) fn arrange(bar: &MenuBar, app: &str, words: &Words) -> Vec<NativeMenu> {
    let mut filed = Filed::default();
    let mut menus: Vec<NativeMenu> = bar
        .menus()
        .iter()
        .map(|menu| NativeMenu {
            title: menu.title().into(),
            kind: match menu.role() {
                Some(MenuRole::Help) => MenuKind::Help,
                None => MenuKind::Plain,
            },
            lines: lower(menu, &mut filed, words),
        })
        .filter(|menu| !menu.lines.is_empty())
        .collect();
    let help = menus.iter().position(|menu| menu.kind == MenuKind::Help).unwrap_or(menus.len());
    menus.insert(help, window_menu(words));
    menus.insert(0, app_menu(app, &filed, words));
    menus
}

/// A menu's lines with the filed items lifted out and the rules tidied.
fn lower<'bar>(menu: &'bar Menu, filed: &mut Filed<'bar>, words: &Words) -> Vec<Line> {
    let lines = menu.entries().iter().filter_map(|item| match item {
        Item::About if !filed.about => {
            filed.about = true;
            None
        }
        Item::Command(command) => match command.filed_as() {
            Some(Role::Settings) if filed.settings.is_none() => {
                filed.settings = Some(command);
                None
            }
            Some(Role::Quit) if filed.quit.is_none() => {
                filed.quit = Some(command);
                None
            }
            _ => Some(Line::Command {
                title: command.title().into(),
                action: command.action(),
                key: KeyEquivalent::of(command.keys()),
            }),
        },
        // a second About has nowhere else to go on the mac
        Item::About => None,
        Item::Edit(edit) => Some(Line::Edit {
            edit: *edit,
            title: Rc::from(&*words.get(edit.word())),
            key: KeyEquivalent::of(Some(&Shortcut::from(edit.stroke()))),
        }),
        Item::Submenu(menu) => {
            let lines = lower(menu, filed, words);
            (!lines.is_empty()).then(|| {
                Line::Submenu(NativeMenu {
                    title: menu.title().into(),
                    kind: MenuKind::Plain,
                    lines,
                })
            })
        }
        Item::Separator => Some(Line::Separator),
    });
    tidy(lines)
}

/// No rule at either end and never two in a row; a menu of rules alone is
/// empty.
fn tidy(lines: impl IntoIterator<Item = Line>) -> Vec<Line> {
    let mut tidied: Vec<Line> = Vec::new();
    for line in lines {
        let rule = line == Line::Separator;
        if rule && tidied.last().is_none_or(|last| *last == Line::Separator) {
            continue;
        }
        tidied.push(line);
    }
    if tidied.last() == Some(&Line::Separator) {
        tidied.pop();
    }
    tidied
}

/// ⌘ and a letter — the platform's own items' keys.
fn command_key(key: char, extra: u64) -> Option<KeyEquivalent> {
    Some(KeyEquivalent { key, mask: COMMAND | extra })
}

/// The app menu: what the app filed here, and the platform's own.
fn app_menu(app: &str, filed: &Filed<'_>, words: &Words) -> NativeMenu {
    // the mac's words, in the person's language; the ones that name the
    // app take the name where the language puts it
    let word = |word: Word| -> Rc<str> { Rc::from(&*words.get(word)) };
    let titled = |word: Word| -> Rc<str> { Rc::from(words.titled(word, app)) };
    let about = filed.about.then(|| Line::System {
        title: titled(Word::About),
        selector: "orderFrontStandardAboutPanel:",
        key: None,
    });
    let settings = filed.settings.map(|command| Line::Command {
        title: word(Word::Settings),
        action: command.action(),
        key: KeyEquivalent::of(command.keys()),
    });
    let quit = Line::Quit {
        title: titled(Word::Quit),
        action: filed.quit.map(Command::action),
        key: match filed.quit {
            Some(command) => KeyEquivalent::of(command.keys()),
            None => command_key('q', 0),
        },
    };
    let lines = about.into_iter().chain([Line::Separator]).chain(settings).chain([
        Line::Separator,
        Line::Services { title: word(Word::Services) },
        Line::Separator,
        Line::System { title: titled(Word::Hide), selector: "hide:", key: command_key('h', 0) },
        Line::System {
            title: word(Word::HideOthers),
            selector: "hideOtherApplications:",
            key: command_key('h', OPTION),
        },
        Line::System { title: word(Word::ShowAll), selector: "unhideAllApplications:", key: None },
        Line::Separator,
        quit,
    ]);
    NativeMenu { title: app.into(), kind: MenuKind::App, lines: tidy(lines) }
}

/// The Window menu: the platform's items; AppKit adds the window list.
fn window_menu(words: &Words) -> NativeMenu {
    let word = |word: Word| -> Rc<str> { Rc::from(&*words.get(word)) };
    NativeMenu {
        title: word(Word::Window),
        kind: MenuKind::Window,
        lines: vec![
            Line::System {
                title: word(Word::Minimize),
                selector: "performMiniaturize:",
                key: command_key('m', 0),
            },
            Line::System { title: word(Word::Zoom), selector: "performZoom:", key: None },
            Line::Separator,
            Line::System {
                title: word(Word::BringAllToFront),
                selector: "arrangeInFront:",
                key: None,
            },
        ],
    }
}

/// The selector AppKit sends for a standard edit — the one a hosted page
/// answers for itself as first responder.
pub(crate) const fn edit_selector(edit: Edit) -> &'static str {
    match edit {
        Edit::Undo => "undo:",
        Edit::Redo => "redo:",
        Edit::Cut => "cut:",
        Edit::Copy => "copy:",
        Edit::Paste => "paste:",
        Edit::SelectAll => "selectAll:",
    }
}

/// The standard edit a selector names, for the view that answers it.
pub(crate) fn edit_of_selector(selector: &str) -> Option<Edit> {
    Edit::ALL.into_iter().find(|edit| edit_selector(*edit) == selector)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: ActionId = ActionId("app.open");
    const SAVE: ActionId = ActionId("app.save");
    const SETTINGS: ActionId = ActionId("app.settings");
    const QUIT: ActionId = ActionId("app.quit");
    const PALETTE: ActionId = ActionId("app.palette");
    const KEYS: ActionId = ActionId("app.keys");

    fn stroke(key: char) -> Shortcut {
        Shortcut::from(KeyPattern::command(Key::Char(key)))
    }

    /// The app's bar in the PC's shape, the way a cross-platform app
    /// declares it.
    fn bar() -> MenuBar {
        MenuBar::new()
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
                    .item(Command::new("Exit", QUIT).shortcut(stroke('q')).role(Role::Quit)),
            )
            .menu(Menu::new("Edit").items(Edit::ALL))
            .menu(
                Menu::new("View").item(
                    Command::new("Command Palette", PALETTE)
                        .shortcut(Shortcut::from(KeyPattern::command_shift(Key::Char('p')))),
                ),
            )
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

    fn titles(menus: &[NativeMenu]) -> Vec<&str> {
        menus.iter().map(|menu| &*menu.title).collect()
    }

    fn line_titles(menu: &NativeMenu) -> Vec<String> {
        menu.lines
            .iter()
            .map(|line| match line {
                Line::Command { title, .. }
                | Line::Quit { title, .. }
                | Line::System { title, .. }
                | Line::Edit { title, .. }
                | Line::Services { title } => title.to_string(),
                Line::Submenu(menu) => format!("{} ▸", menu.title),
                Line::Separator => "─".to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_app_menu_leads_and_holds_what_the_mac_files_there() {
        let menus = arrange(&bar(), "Trinity", &Words::english());
        assert_eq!(titles(&menus), ["Trinity", "File", "Edit", "View", "Window", "Help"]);
        assert_eq!(menus[0].kind, MenuKind::App);
        assert_eq!(
            line_titles(&menus[0]),
            [
                "About Trinity",
                "─",
                "Settings…",
                "─",
                "Services",
                "─",
                "Hide Trinity",
                "Hide Others",
                "Show All",
                "─",
                "Quit Trinity",
            ],
        );
        let Line::Command { action, key, .. } = &menus[0].lines[2] else {
            panic!("settings is the app's command");
        };
        assert_eq!(*action, SETTINGS, "the mac's title, the app's action");
        assert_eq!(*key, Some(KeyEquivalent { key: ',', mask: COMMAND }), "and the app's keys");
        let Line::Quit { action, key, .. } = &menus[0].lines[10] else {
            panic!("quit closes the app menu");
        };
        assert_eq!(*action, Some(QUIT));
        assert_eq!(*key, Some(KeyEquivalent { key: 'q', mask: COMMAND }));
    }

    #[test]
    fn a_filed_item_leaves_no_gap_where_it_stood() {
        let menus = arrange(&bar(), "Trinity", &Words::english());
        assert_eq!(line_titles(&menus[1]), ["Open…", "Save"], "no rule is left dangling in File");
        assert_eq!(
            line_titles(&menus[5]),
            ["Keyboard Shortcuts"],
            "About moved out of Help and took its rule with it",
        );
        let only_filed = MenuBar::new().menu(
            Menu::new("File")
                .item(Command::new("Settings", SETTINGS).role(Role::Settings))
                .separator()
                .item(Command::new("Exit", QUIT).role(Role::Quit)),
        );
        assert_eq!(
            titles(&arrange(&only_filed, "Trinity", &Words::english())),
            ["Trinity", "Window"],
            "a menu left with nothing is not drawn",
        );
    }

    #[test]
    fn the_window_menu_stands_before_help_or_last() {
        let menus = arrange(&bar(), "Trinity", &Words::english());
        assert_eq!(menus[4].kind, MenuKind::Window);
        assert_eq!(menus[5].kind, MenuKind::Help);
        assert_eq!(line_titles(&menus[4]), ["Minimize", "Zoom", "─", "Bring All to Front"]);
        let helpless = MenuBar::new().menu(Menu::new("File").item(Command::new("Open…", OPEN)));
        let menus = arrange(&helpless, "Trinity", &Words::english());
        assert_eq!(titles(&menus), ["Trinity", "File", "Window"]);
    }

    #[test]
    fn with_no_quit_declared_the_system_quits() {
        let menus = arrange(&MenuBar::new(), "Trinity", &Words::english());
        assert_eq!(
            line_titles(&menus[0]),
            ["Services", "─", "Hide Trinity", "Hide Others", "Show All", "─", "Quit Trinity"]
        );
        assert_eq!(
            menus[0].lines.last(),
            Some(&Line::Quit {
                title: "Quit Trinity".into(),
                action: None,
                key: Some(KeyEquivalent { key: 'q', mask: COMMAND }),
            }),
        );
    }

    /// The bar speaks the words it is given: a Brazilian machine reads the
    /// app menu, the edits and the Window menu in Portuguese, and the
    /// app's own titles stay the app's.
    #[test]
    fn the_app_menu_speaks_the_words_it_is_given() {
        let words = Words::for_locale(&bunny_ui::prelude::Locale::new("pt-BR"));
        let menus = arrange(&bar(), "Bunny", &words);
        assert_eq!(titles(&menus), ["Bunny", "File", "Edit", "View", "Janela", "Help"]);
        assert_eq!(
            line_titles(&menus[0]),
            [
                "Sobre o Bunny",
                "─",
                "Ajustes…",
                "─",
                "Serviços",
                "─",
                "Ocultar Bunny",
                "Ocultar Outros",
                "Mostrar Tudo",
                "─",
                "Encerrar Bunny",
            ],
        );
        assert_eq!(
            line_titles(&menus[2]),
            ["Desfazer", "Refazer", "Recortar", "Copiar", "Colar", "Selecionar Tudo"]
        );
        assert_eq!(line_titles(&menus[4]), ["Minimizar", "Zoom", "─", "Trazer Tudo para a Frente"]);
    }

    /// A language that puts the app's name last keeps its order: the
    /// template decides where the name goes, not a prefix.
    #[test]
    fn a_word_order_that_puts_the_app_last_is_kept() {
        let german = Words::for_locale(&bunny_ui::prelude::Locale::new("de"));
        let menus = arrange(&MenuBar::new(), "Bunny", &german);
        assert_eq!(
            line_titles(&menus[0]),
            ["Dienste", "─", "Bunny ausblenden", "Andere ausblenden", "Alle einblenden", "─", "Bunny beenden"]
        );
        let japanese = Words::for_locale(&bunny_ui::prelude::Locale::new("ja"));
        let menus = arrange(&MenuBar::new(), "Bunny", &japanese);
        let lines = line_titles(&menus[0]);
        assert_eq!(lines[2], "Bunnyを非表示");
        assert_eq!(lines[6], "Bunnyを終了");
    }

    #[test]
    fn a_key_equivalent_is_drawn_only_where_the_mac_can_keep_it() {
        let of = |pattern: KeyPattern| KeyEquivalent::of(Some(&Shortcut::from(pattern)));
        assert_eq!(
            of(KeyPattern::command_shift(Key::Char('p'))),
            Some(KeyEquivalent { key: 'p', mask: COMMAND | SHIFT }),
            "shift rides the mask, the character stays lowercase",
        );
        assert_eq!(
            of(KeyPattern::control(Key::Char('`'))),
            Some(KeyEquivalent { key: '`', mask: CONTROL }),
            "a control chord is kept",
        );
        assert_eq!(
            of(KeyPattern::key(Key::F(12))),
            Some(KeyEquivalent { key: '\u{f70f}', mask: 0 })
        );
        assert_eq!(of(KeyPattern::key(Key::Char('j'))), None, "a bare key would take typing");
        assert_eq!(of(KeyPattern::shift(Key::Enter)), None, "and so would a bare named key");
        assert_eq!(
            of(KeyPattern::command(Key::Up)),
            Some(KeyEquivalent { key: '\u{f700}', mask: COMMAND }),
        );
        let menus = arrange(&bar(), "Trinity", &Words::english());
        let Line::Command { key, .. } = &menus[5].lines[0] else {
            panic!("the shortcuts item");
        };
        assert_eq!(*key, None, "a sequence has no key equivalent in AppKit");
    }

    #[test]
    fn the_edit_items_carry_their_accelerators_and_selectors() {
        let menus = arrange(&bar(), "Trinity", &Words::english());
        let keys: Vec<_> = menus[2]
            .lines
            .iter()
            .map(|line| match line {
                Line::Edit { edit, key, .. } => (edit_selector(*edit), key.clone()),
                other => panic!("the Edit menu holds edits only, not {other:?}"),
            })
            .collect();
        assert_eq!(
            keys,
            [
                ("undo:", Some(KeyEquivalent { key: 'z', mask: COMMAND })),
                ("redo:", Some(KeyEquivalent { key: 'z', mask: COMMAND | SHIFT })),
                ("cut:", Some(KeyEquivalent { key: 'x', mask: COMMAND })),
                ("copy:", Some(KeyEquivalent { key: 'c', mask: COMMAND })),
                ("paste:", Some(KeyEquivalent { key: 'v', mask: COMMAND })),
                ("selectAll:", Some(KeyEquivalent { key: 'a', mask: COMMAND })),
            ],
        );
        for edit in Edit::ALL {
            assert_eq!(edit_of_selector(edit_selector(edit)), Some(edit), "{edit:?} round-trips");
        }
        assert_eq!(edit_of_selector("performZoom:"), None);
    }

    #[test]
    fn a_submenu_is_lowered_and_tidied_like_a_menu() {
        let bar = MenuBar::new().menu(
            Menu::new("File").item(
                Menu::new("Recent")
                    .separator()
                    .item(Command::new("Keys", KEYS))
                    .separator()
                    .separator(),
            ),
        );
        let menus = arrange(&bar, "Trinity", &Words::english());
        let Line::Submenu(recent) = &menus[1].lines[0] else {
            panic!("the submenu stands");
        };
        assert_eq!(line_titles(recent), ["Keys"]);
        let empty = MenuBar::new()
            .menu(Menu::new("File").item(Menu::new("Recent")).item(Command::new("Open…", OPEN)));
        assert_eq!(
            line_titles(&arrange(&empty, "Trinity", &Words::english())[1]),
            ["Open…"],
            "an empty submenu is not drawn"
        );
    }
}
