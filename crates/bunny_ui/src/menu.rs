//! The app's commands, gathered into menus — one model, and every shell
//! draws it in its own platform's manner.
//!
//! A menu is a way in to what the app already does. Every item it holds
//! is something the reader could reach another way: a [`Command`] is an
//! [`ActionId`] the keymap and the palette already know, and an [`Edit`]
//! item is the copy or the paste the keyboard already makes. So the menu
//! declares nothing new about behaviour. It declares WHERE a command is
//! found, and what it is called there.
//!
//! The model has no platform in it, and it could not have one: the same
//! bar is the mac's system menu bar, a strip drawn in a Windows or Linux
//! window, the iPad's menu bar and a phone's overflow menu. What differs
//! between them is filed by [`Role`]:
//!
//! - the app declares its menus in the PC's shape — File, Edit, View, …,
//!   Help — because that is the shape every platform but one reads;
//! - [`Item::About`], and a command marked [`Role::Settings`] or
//!   [`Role::Quit`], are placed by the platform. The mac moves them to its
//!   app menu and titles them in its own words; a PC leaves them where the
//!   app put them;
//! - what only one platform has — the mac's Services, Hide and Show All,
//!   its Window menu — is that shell's, and the app never declares it.
//!
//! ## What a shell owes the model
//!
//! - **The keymap first.** A stroke reaches the keymap before any item's
//!   key equivalent. A pending chord, a binding scoped to a context and a
//!   reader's rebind all keep their strokes, so a menu can never become a
//!   second keymap.
//! - **The shortcut shown is the app's.** [`Command::shortcut`] is what the
//!   app read from its own keymap. A shell draws it, or draws nothing where
//!   its platform cannot say it — it never invents one.
//! - **Nothing hides.** An item the window cannot answer now is drawn
//!   disabled ([`Runtime::menu_answers`]), so a menu still teaches what the
//!   app can do.
//! - **An item does what its keys do.** [`Runtime::menu_pick`] runs a
//!   command through the same dispatch a binding takes, and an edit item
//!   through the same road its stroke takes.
//!
//! ```ignore
//! let bar = MenuBar::new()
//!     .menu(
//!         Menu::new("File")
//!             .item(Command::new("Save", SAVE).shortcut(Shortcut::from(KeyPattern::command(Key::Char('s')))))
//!             .separator()
//!             .item(Command::new("Settings…", SETTINGS).role(Role::Settings))
//!             .item(Command::new("Quit", QUIT).role(Role::Quit)),
//!     )
//!     .menu(Menu::new("Edit").items(Edit::ALL))
//!     .menu(Menu::help("Help").item(Item::About));
//! ```
//!
//! [`Runtime::menu_answers`]: crate::runtime::Runtime::menu_answers
//! [`Runtime::menu_pick`]: crate::runtime::Runtime::menu_pick

use std::rc::Rc;

use crate::action::{ActionId, Key, KeyPattern};

// =============================================================================
// The bar and its menus
// =============================================================================

/// Every menu the app offers, in the order the reader meets them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuBar {
    menus: Vec<Menu>,
}

impl MenuBar {
    /// A bar with no menus yet.
    pub fn new() -> MenuBar {
        MenuBar::default()
    }

    /// The next menu along the bar.
    #[must_use]
    pub fn menu(mut self, menu: Menu) -> MenuBar {
        self.menus.push(menu);
        self
    }

    /// The menus, in order.
    pub fn menus(&self) -> &[Menu] {
        &self.menus
    }

    /// Every command the bar holds, submenus included, in reading order —
    /// what a platform with no menu bar lists instead, and what a probe
    /// walks.
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.menus.iter().flat_map(Menu::commands)
    }
}

/// What a whole menu is FOR, where a platform treats it apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuRole {
    /// The Help menu. The mac gives it the system's search field and keeps
    /// its own Window menu just before it.
    Help,
}

/// One menu: a title, and what opens under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    title: Rc<str>,
    role: Option<MenuRole>,
    items: Vec<Item>,
}

impl Menu {
    /// An empty menu under `title`.
    pub fn new(title: impl Into<Rc<str>>) -> Menu {
        Menu { title: title.into(), role: None, items: Vec::new() }
    }

    /// The Help menu, under `title` ([`MenuRole::Help`]).
    pub fn help(title: impl Into<Rc<str>>) -> Menu {
        Menu { role: Some(MenuRole::Help), ..Menu::new(title) }
    }

    /// The next item down.
    #[must_use]
    pub fn item(mut self, item: impl Into<Item>) -> Menu {
        self.items.push(item.into());
        self
    }

    /// Several items at once, in order — `Edit::ALL` is the usual one.
    #[must_use]
    pub fn items<I: Into<Item>>(mut self, items: impl IntoIterator<Item = I>) -> Menu {
        self.items.extend(items.into_iter().map(Into::into));
        self
    }

    /// A rule between two groups. A shell draws none at either end and
    /// never two in a row, so a group that a platform moves away leaves no
    /// gap behind.
    #[must_use]
    pub fn separator(self) -> Menu {
        self.item(Item::Separator)
    }

    /// The menu's title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// What the menu is for, where a platform cares.
    pub fn role(&self) -> Option<MenuRole> {
        self.role
    }

    /// What opens under the title, in order.
    pub fn entries(&self) -> &[Item] {
        &self.items
    }

    /// Every command under this menu, its submenus' included.
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.items.iter().flat_map(|item| -> Box<dyn Iterator<Item = &Command> + '_> {
            match item {
                Item::Command(command) => Box::new(std::iter::once(command)),
                Item::Submenu(menu) => Box::new(menu.commands()),
                Item::Edit(_) | Item::About | Item::Separator => Box::new(std::iter::empty()),
            }
        })
    }
}

// =============================================================================
// Items
// =============================================================================

/// One line of a menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// Something the app does, by the action that does it.
    Command(Command),
    /// One of the standard edits, on whoever holds the keyboard.
    Edit(Edit),
    /// The app's About box — the platform's own where it has one. The mac
    /// files it first in its app menu; elsewhere it stands where the app
    /// put it, which is the Help menu by every PC's convention.
    About,
    /// A menu opening from this line.
    Submenu(Menu),
    /// A rule between two groups.
    Separator,
}

impl From<Command> for Item {
    fn from(command: Command) -> Item {
        Item::Command(command)
    }
}

impl From<Edit> for Item {
    fn from(edit: Edit) -> Item {
        Item::Edit(edit)
    }
}

impl From<Menu> for Item {
    fn from(menu: Menu) -> Item {
        Item::Submenu(menu)
    }
}

/// Where a platform FILES an item, when it files it apart from where the
/// app declared it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The app's settings. The mac files it in the app menu as
    /// "Settings…".
    Settings,
    /// The way out of the app. The mac files it last in the app menu as
    /// "Quit {app}", and Windows titles it Exit.
    ///
    /// Quit is the one item that never goes dark: where the window does not
    /// answer the action, the platform's own quit runs instead.
    Quit,
}

/// Something the app does, as a menu offers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    title: Rc<str>,
    action: ActionId,
    shortcut: Option<Shortcut>,
    role: Option<Role>,
}

impl Command {
    /// `title`, running `action` — the same id the keymap binds and the
    /// view mounts `.on_action` for.
    pub fn new(title: impl Into<Rc<str>>, action: ActionId) -> Command {
        Command { title: title.into(), action, shortcut: None, role: None }
    }

    /// The keys that reach the same action, as the app's keymap binds them.
    ///
    /// Read it from the keymap and never write one here by hand: a
    /// shortcut shown beside an item is a promise about what the keys do,
    /// and only the keymap can keep it. `None` says there is none.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<Option<Shortcut>>) -> Command {
        self.shortcut = shortcut.into();
        self
    }

    /// Files the command where the platform keeps items of this kind.
    #[must_use]
    pub fn role(mut self, role: Role) -> Command {
        self.role = Some(role);
        self
    }

    /// The title the app gave it. A platform with a word of its own for a
    /// [`Role`] uses that word instead.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The action it runs.
    pub fn action(&self) -> ActionId {
        self.action
    }

    /// The keys that reach it, if any.
    pub fn keys(&self) -> Option<&Shortcut> {
        self.shortcut.as_ref()
    }

    /// Where a platform files it, if anywhere special.
    pub fn filed_as(&self) -> Option<Role> {
        self.role
    }
}

/// The standard edits every platform's Edit menu carries. They act on
/// whoever holds the keyboard — a field, a box, a read-only view that
/// copies, a hosted page — and the app declares only where they stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edit {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl Edit {
    /// The six, in the order every platform's Edit menu lists them. The
    /// rule every platform draws between Redo and Cut is the app's to put
    /// there, like any other separator.
    pub const ALL: [Edit; 6] =
        [Edit::Undo, Edit::Redo, Edit::Cut, Edit::Copy, Edit::Paste, Edit::SelectAll];

    /// The item's title, as the platforms all write it.
    pub const fn title(self) -> &'static str {
        match self {
            Edit::Undo => "Undo",
            Edit::Redo => "Redo",
            Edit::Cut => "Cut",
            Edit::Copy => "Copy",
            Edit::Paste => "Paste",
            Edit::SelectAll => "Select All",
        }
    }

    /// The stroke the item stands for, spelled with the framework's
    /// accelerator (`command` — ⌘ on the mac, Ctrl on a PC, as the shells
    /// report it).
    ///
    /// A box that reads strokes is offered this exact stroke when the item
    /// is chosen, so the editor's own undo and select-all answer the menu
    /// the way they answer the keys. Redo is ⇧⌘Z here; the PC's Ctrl+Y is
    /// its shell's caption, not a second stroke.
    pub const fn stroke(self) -> KeyPattern {
        match self {
            Edit::Undo => KeyPattern::command(Key::Char('z')),
            Edit::Redo => KeyPattern::command_shift(Key::Char('z')),
            Edit::Cut => KeyPattern::command(Key::Char('x')),
            Edit::Copy => KeyPattern::command(Key::Char('c')),
            Edit::Paste => KeyPattern::command(Key::Char('v')),
            Edit::SelectAll => KeyPattern::command(Key::Char('a')),
        }
    }
}

// =============================================================================
// Shortcuts
// =============================================================================

/// The keys that reach a command: one stroke, or a sequence of them
/// (`⌘K ⌘S`). Never empty — there is no way to build one that is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    first: KeyPattern,
    rest: Rc<[KeyPattern]>,
}

impl Shortcut {
    /// The strokes in order, or `None` when there are none — which is the
    /// keymap's "unbound", not a shortcut.
    pub fn sequence(strokes: &[KeyPattern]) -> Option<Shortcut> {
        let (first, rest) = strokes.split_first()?;
        Some(Shortcut { first: *first, rest: rest.into() })
    }

    /// The one stroke, when the shortcut is a single stroke. A native menu
    /// can only draw — and only match — one; a sequence it leaves unsaid.
    pub fn single(&self) -> Option<KeyPattern> {
        self.rest.is_empty().then_some(self.first)
    }

    /// Every stroke, in order.
    pub fn strokes(&self) -> impl Iterator<Item = KeyPattern> + '_ {
        std::iter::once(self.first).chain(self.rest.iter().copied())
    }
}

impl From<KeyPattern> for Shortcut {
    fn from(stroke: KeyPattern) -> Shortcut {
        Shortcut { first: stroke, rest: Rc::from([]) }
    }
}

// =============================================================================
// What an item asks of the window
// =============================================================================

/// What a chosen item asks of the window it acts on — the half of a menu
/// the runtime answers ([`Runtime::menu_answers`], [`Runtime::menu_pick`]).
/// The rest (About, the platform's own Hide and Quit) is the shell's.
///
/// [`Runtime::menu_answers`]: crate::runtime::Runtime::menu_answers
/// [`Runtime::menu_pick`]: crate::runtime::Runtime::menu_pick
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pick {
    /// Run the action, as a binding would.
    Command(ActionId),
    /// Make the edit, as its stroke would.
    Edit(Edit),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAVE: ActionId = ActionId("test.save");
    const OPEN: ActionId = ActionId("test.open");
    const KEYS: ActionId = ActionId("test.keys");

    #[test]
    fn a_shortcut_is_never_empty() {
        assert_eq!(Shortcut::sequence(&[]), None, "no strokes is unbound, not a shortcut");
        let save = KeyPattern::command(Key::Char('s'));
        let one = Shortcut::sequence(&[save]).expect("one stroke");
        assert_eq!(one.single(), Some(save));
        assert_eq!(one, Shortcut::from(save), "a sequence of one IS the stroke");
        let chord = [KeyPattern::command(Key::Char('k')), save];
        let two = Shortcut::sequence(&chord).expect("two strokes");
        assert_eq!(two.single(), None, "a sequence has no single stroke to draw");
        assert_eq!(two.strokes().collect::<Vec<_>>(), chord);
    }

    #[test]
    fn the_bar_reads_its_commands_in_order_submenus_included() {
        let bar = MenuBar::new()
            .menu(
                Menu::new("File")
                    .item(Command::new("Open…", OPEN))
                    .separator()
                    .item(Menu::new("Recent").item(Command::new("Keys", KEYS))),
            )
            .menu(Menu::new("Edit").items(Edit::ALL).item(Command::new("Save", SAVE)));
        let actions: Vec<_> = bar.commands().map(Command::action).collect();
        assert_eq!(actions, [OPEN, KEYS, SAVE]);
        assert_eq!(bar.menus()[1].entries().len(), 7, "six edits and the command");
    }

    #[test]
    fn a_command_carries_its_keys_and_its_filing() {
        let settings = Command::new("Settings…", OPEN)
            .shortcut(Shortcut::from(KeyPattern::command(Key::Char(','))))
            .role(Role::Settings);
        assert_eq!(settings.filed_as(), Some(Role::Settings));
        assert_eq!(
            settings.keys().and_then(Shortcut::single),
            Some(KeyPattern::command(Key::Char(',')))
        );
        let bare = Command::new("Save", SAVE).shortcut(None);
        assert_eq!(bare.keys(), None);
        assert_eq!(bare.filed_as(), None);
    }

    #[test]
    fn every_edit_stands_for_its_accelerator_stroke() {
        for edit in Edit::ALL {
            let stroke = edit.stroke();
            assert!(
                stroke.command && !stroke.control && !stroke.option,
                "{edit:?} is an accelerator chord"
            );
        }
        assert!(Edit::Redo.stroke().shift, "redo is undo with shift");
        assert_eq!(Edit::SelectAll.title(), "Select All");
    }

    #[test]
    fn the_help_menu_says_so() {
        assert_eq!(Menu::help("Help").role(), Some(MenuRole::Help));
        assert_eq!(Menu::new("View").role(), None);
    }
}
