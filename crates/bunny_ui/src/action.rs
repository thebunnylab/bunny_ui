//! Named actions + key patterns — the smallest keymap with the right shape.
//!
//! A key becomes INTENT (`KeyPattern → ActionId`, in the `Runtime`'s
//! keymap); intent finds the HANDLER in force (`.on_action`, retained in
//! the reconciler like the click actions — the innermost wins). The
//! shell only translates and composes: match → dispatch → repaint. A
//! binding with no mounted handler does not consume the key — the screen
//! without the palette types normally.

/// An action's nominal identity — declared as a const by the app:
/// `const SELECT_NEXT: ActionId = ActionId("finder.select_next");`
/// Namespace by convention (`"app.action"`); the string prints and
/// serializes (map debugging today, a configurable keymap tomorrow).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ActionId(pub &'static str);

/// What the keymap makes of one stroke. A chord is why this is not an
/// `Option`: a stroke that STARTS a sequence fires nothing and belongs
/// to nobody else — the keyboard is held until the next one resolves
/// it or lets it go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyMatch {
    /// A binding answered. The caller dispatches it.
    Action(ActionId),
    /// The stroke opened (or continued) a sequence. Nothing fired, and
    /// the stroke is SPENT — the field must not type it, and the app's
    /// which-key can read [`Runtime::pending_chord`] to show what is
    /// still on offer.
    ///
    /// The same answer comes back for a stroke that ENDED a sequence
    /// leading nowhere: the runtime has already given the held strokes
    /// their meaning — the longest one bound on its own fired, or the
    /// first went to the text door — and offered the rest again, this
    /// one included. It is spent here because it was answered there.
    ///
    /// [`Runtime::pending_chord`]: crate::runtime::Runtime::pending_chord
    Pending,
    /// A binding says the stroke means NOTHING here — a keymap's `null`,
    /// answered by a [`KeyResolver`]. It is spent like a pending one:
    /// nothing runs, and the field does not type it. A key that falls
    /// through to the text instead is not disabled, only unbound.
    Disabled,
    /// Nothing is bound. The stroke walks on, as it always did.
    None,
}

/// One stroke as the keymap saw it — what a key-context debugger lists:
/// the key, the contexts in force when it arrived, and what the keymap
/// made of it.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyReport {
    /// The stroke, as the shell read it.
    pub stroke: Stroke,
    /// The key contexts active when it arrived, OUTERMOST first — the
    /// view nearest the root leads, and each context is named once.
    pub contexts: Vec<&'static str>,
    /// The keymap's answer: the action it resolved to, the middle of a
    /// sequence, or nothing bound. Whether a handler then ran is the
    /// dispatch's business, and a stroke is reported the same either way.
    pub matched: KeyMatch,
}

/// Closes the innermost open popover. Every mounted popover registers
/// a handler for it, and the runtime pre-binds Escape inside the
/// reserved [`OVERLAY_CONTEXT`] — apps never wire this themselves.
pub const OVERLAY_DISMISS: ActionId = ActionId("bunny.popover.dismiss");

/// The key context every open popover declares. The `bunny.` prefix is
/// reserved for the framework's own contexts.
pub const OVERLAY_CONTEXT: &str = "bunny.popover";

/// The key context every open alert declares — and while one is up, the
/// ONLY context the keymap answers from. An alert is modal to the
/// keyboard as well as to the pointer: the page's bindings under it (a
/// close, a chord, a palette) are inert until it is answered. The
/// runtime pre-binds Escape and ⌘. to [`ALERT_CANCEL`] and Return to
/// [`ALERT_DEFAULT`] here; apps never wire these keys themselves.
pub const ALERT_CONTEXT: &str = "bunny.alert";

/// The open alert's cancel answer — Escape and ⌘. — on the same road its
/// window's close button takes: the alert's binding is set to `false`,
/// and what the app does on that write IS its cancel answer.
pub const ALERT_CANCEL: ActionId = ActionId("bunny.alert.cancel");

/// Return, inside an open alert: runs what the alert's content mounted
/// under this id — the default button's own action, registered beside it
/// with `.on_action(ALERT_DEFAULT, …)`. An alert that mounts none has no
/// default, and Return does nothing there: the right answer for an ask
/// that arrives unasked, where a stroke meant for the page underneath
/// must not become a decision.
pub const ALERT_DEFAULT: ActionId = ActionId("bunny.alert.default");

/// The key context the days of a calendar declare while the keyboard is
/// on one of them. The runtime pre-binds here the keys that leave the
/// month: an arrow at the edge of the days, Page Up and Page Down. An
/// arrow inside the month never reaches the keymap; the days are a group
/// and the group moves it.
pub const CALENDAR_CONTEXT: &str = "bunny.calendar";

/// An arrow that left the days of a calendar by an edge: the date moves
/// one week or one day, into the month before or after.
pub const CALENDAR_UP: ActionId = ActionId("bunny.calendar.up");
/// See [`CALENDAR_UP`].
pub const CALENDAR_DOWN: ActionId = ActionId("bunny.calendar.down");
/// See [`CALENDAR_UP`].
pub const CALENDAR_LEFT: ActionId = ActionId("bunny.calendar.left");
/// See [`CALENDAR_UP`].
pub const CALENDAR_RIGHT: ActionId = ActionId("bunny.calendar.right");
/// Page Up on a day of a calendar: the same day of the month before.
pub const CALENDAR_PREVIOUS_MONTH: ActionId = ActionId("bunny.calendar.previous-month");
/// Page Down on a day of a calendar: the same day of the month after.
pub const CALENDAR_NEXT_MONTH: ActionId = ActionId("bunny.calendar.next-month");

/// The prefix the framework keeps for itself. A key context under it
/// belongs to the house: an app never declares one, and emptying the
/// key table leaves them standing.
pub const RESERVED_PREFIX: &str = "bunny.";

impl std::fmt::Display for ActionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Named keys + the printable case (lowercase, no modifier).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Key {
    Down,
    Up,
    Left,
    Right,
    Enter,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Home,
    End,
    /// A function key, by its number: `F(1)` is F1, `F(12)` is F12.
    ///
    /// Every platform numbers the row the same way and none goes past
    /// 24, so neither does this: a shell reports `F(1)` to `F(24)` and
    /// nothing outside them. A keymap that says `f12` means the key a
    /// product's go-to-definition lives on, on any keyboard.
    ///
    /// What arrives is the KEY. On a laptop the row is brightness and
    /// volume until `fn` is held, and those belong to the system — they
    /// never become a stroke, here or in any other app.
    F(u8),
    /// Insert — the key a PC keyboard toggles overwriting with (Help on
    /// an Apple extended keyboard, the same key code). A laptop without
    /// one simply never sends it.
    Insert,
    /// Shift pressed and let go ON ITS OWN, with no key and no click in
    /// between — the stroke behind a double-shift palette. Held with
    /// another key it is that key's modifier and never a stroke of its
    /// own, so the pattern carries no modifier flag: the key IS the
    /// modifier. The runtime makes this stroke from the shell's modifier
    /// reports ([`Runtime::modifiers_changed`]); no shell sends it.
    ///
    /// [`Runtime::modifiers_changed`]: crate::runtime::Runtime::modifiers_changed
    Shift,
    /// Command alone — Ctrl on a PC, where Ctrl is the primary modifier.
    /// The same law as [`Key::Shift`].
    Command,
    /// Option alone — Alt on a PC. The same law as [`Key::Shift`].
    Option,
    /// Control alone — the Mac's ⌃. The same law as [`Key::Shift`].
    Control,
    /// A printable key, named by THE CHARACTER IT TYPES WITH NO
    /// MODIFIER APPLIED, lowercased.
    ///
    /// This is the contract every shell owes, and it is what makes a
    /// chord spellable: `command_shift(Char('\\'))` is "the backslash
    /// key, with shift", not "the pipe character". A shell that reports
    /// the SHIFTED character instead makes every chord on shifted
    /// punctuation unmatchable — the letters survive by accident,
    /// because lowercasing repairs them, and the punctuation does not.
    ///
    /// Each platform has the right question to ask: `ToUnicode` with a
    /// clean keyboard state on Windows, `charactersByApplyingModifiers:0`
    /// on macOS, the keyboard layout map in a browser that has one. All
    /// three read the USER'S OWN LAYOUT, which is why none of them is a
    /// table of US pairs — a table would be wrong on the Brazilian
    /// keyboard this framework is written on.
    Char(char),
}

/// What the hand was holding — the same four the keymap names, in the
/// same order, because a press and a stroke ask the same question with
/// different hardware.
///
/// A press needs them for the same reason a stroke does: only the box
/// under the pointer knows what command and shift MEAN over its own
/// content. Command and a click is a jump to a definition in one box
/// and nothing at all in the next, and no gate above can decide that.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub command: bool,
    pub option: bool,
    pub control: bool,
}

impl Modifiers {
    /// Nothing held.
    pub const NONE: Modifiers =
        Modifiers { shift: false, command: false, option: false, control: false };

    /// Shift alone — the one a press used to carry, kept as a name so
    /// the ordinary case reads as a word.
    pub const SHIFT: Modifiers = Modifiers { shift: true, ..Modifiers::NONE };

    /// Is anything at all held? A plain press is the common case and
    /// the one a box answers fastest.
    pub const fn any(&self) -> bool {
        self.shift || self.command || self.option || self.control
    }
}

impl From<bool> for Modifiers {
    /// Shift, and nothing else — what the press door carried before it
    /// could carry the rest, so every caller that says `false` or
    /// hands over a shift flag keeps meaning what it meant.
    fn from(shift: bool) -> Modifiers {
        Modifiers { shift, ..Modifiers::NONE }
    }
}

/// The pattern the keymap matches — EXACT modifiers (Cmd+Enter does not
/// match the Enter binding). `Eq + Hash`: a direct map key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyPattern {
    pub key: Key,
    pub shift: bool,
    pub command: bool,
    pub option: bool,
    pub control: bool,
}

/// One keystroke, as the platform read it: the pattern that NAMES the
/// key, and the character that key TYPES under the modifiers held.
///
/// Two fields because they are two questions, and one answer cannot
/// serve both. The pattern carries the key's OWN character, read
/// through the user's layout, so a chord spelled with a backslash
/// matches the key the user pressed and not the `|` that key would
/// have typed. The typed character is the other half — what a box that
/// REFUSES text still needs, because what a `{` means is the box's
/// business, and which key makes a `{` is the layout's: on ABNT2, US,
/// AZERTY and Dvorak it is a different key, and no table in an app can
/// know which.
///
/// `typed` is `None` when command or control is held. Those are the
/// chord modifiers: a chord types nothing, and every platform asked
/// under them answers with the key's base character, which would be a
/// lie about what reached the screen.
///
/// It is what the key ALONE types. A dead key answers with its mark
/// and not with the letter it will compose — the composed letter is
/// the input system's, and it arrives by the text door.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stroke {
    pub pattern: KeyPattern,
    pub typed: Option<char>,
}

impl From<KeyPattern> for Stroke {
    /// A stroke that types nothing — what a test spells, and what a
    /// shell answers for a key its platform gives no character for.
    fn from(pattern: KeyPattern) -> Stroke {
        Stroke { pattern, typed: None }
    }
}

impl From<&KeyPattern> for Stroke {
    fn from(pattern: &KeyPattern) -> Stroke {
        Stroke::from(*pattern)
    }
}

impl From<&Stroke> for Stroke {
    fn from(stroke: &Stroke) -> Stroke {
        *stroke
    }
}

impl Stroke {
    /// The stroke, and what it typed. A shell may hand over whatever
    /// its platform said: the two things that are never a typed
    /// character are dropped HERE, once, so no shell has to remember
    /// them and no shell can forget them.
    ///
    /// A chord types nothing. Asked under command, the Mac hands back
    /// the key's own character — measured, not assumed — which would
    /// be a lie about what reached the screen. A control character is
    /// not text either: control and the letter A make U+0001, and a
    /// box asking what was typed must not be told that.
    ///
    /// A chord that DOES type is not this door's business: the shells
    /// where one exists — AltGr is control and alt together on two of
    /// them — send it down the character road before the gate ever
    /// runs, and it arrives as text.
    pub fn new(pattern: KeyPattern, typed: Option<char>) -> Stroke {
        let chord = pattern.command || pattern.control;
        Stroke { pattern, typed: typed.filter(|typed| !chord && !typed.is_control()) }
    }

    /// How the same key would be spelled by what it TYPES — the
    /// modifiers that made the character are SPENT, so a binding
    /// written as the character alone matches it.
    ///
    /// There is a second spelling only where a modifier CHANGED the
    /// character. Shift and the space bar still make a space, and a
    /// binding on the space bar must not fire for shift-space; the same
    /// for any held modifier a layout ignores, which is how a plain
    /// Alt reads on the platforms where Alt types nothing. Shift and a
    /// letter only change its case, and a keymap spells a letter in one
    /// case anyway.
    ///
    /// `None` when the key typed nothing, and `None` when what it typed
    /// is the key's own name said again.
    pub fn typed_pattern(&self) -> Option<KeyPattern> {
        let typed = self.typed?;
        let renamed = match self.pattern.key {
            Key::Char(base) => !base.eq_ignore_ascii_case(&typed),
            _ => true,
        };
        renamed.then(|| KeyPattern::key(Key::Char(typed)))
    }
}

impl Key {
    /// Is this a lone modifier ([`Key::Shift`] and its siblings) — a key
    /// the keymap reads and no box ever hears?
    pub const fn is_modifier(&self) -> bool {
        matches!(self, Key::Shift | Key::Command | Key::Option | Key::Control)
    }
}

impl KeyPattern {
    /// The bare key.
    pub const fn key(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: false, control: false }
    }

    /// Cmd + key.
    pub const fn command(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: true, option: false, control: false }
    }

    /// Shift + key.
    pub const fn shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: false, option: false, control: false }
    }

    /// Cmd + Shift + key — a product's second shelf of shortcuts.
    pub const fn command_shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: true, option: false, control: false }
    }

    /// Control + key.
    pub const fn control(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: false, control: true }
    }

    /// Control + Shift + key.
    pub const fn control_shift(key: Key) -> KeyPattern {
        KeyPattern { key, shift: true, command: false, option: false, control: true }
    }

    /// Option (Alt) + key.
    pub const fn option(key: Key) -> KeyPattern {
        KeyPattern { key, shift: false, command: false, option: true, control: false }
    }

    /// A bare Char (no cmd/ctrl) is TYPING: with a field focused, the
    /// gate lets it through to the text without consulting the map —
    /// bound or not. (Option counts as typing: option+a composes "å" on
    /// macOS.)
    pub fn is_text_input(&self) -> bool {
        matches!(self.key, Key::Char(_)) && !self.command && !self.control
    }

    /// No accelerator held. Shift does not count — it types, it does
    /// not command. This is the shape a focused field may claim: a bare
    /// Enter is a break, `⌘↵` belongs to the app.
    pub fn is_plain(&self) -> bool {
        !self.command && !self.control && !self.option
    }
}

// MARK: - Key contexts as frames, and the resolver port (D435)

/// A key context as a view declares it: the NAMES it answers to and the
/// FACTS it states about itself, read at the moment a stroke arrives.
///
/// A bare name was enough while a keymap only asked "is the palette up".
/// A modal editor's keymap asks more of the same view at once — which
/// mode, which operator is waiting, whether a count is typed, which
/// language the file is — and a name per combination multiplies without
/// end. A frame states each fact once; the [`KeyResolver`] reads them.
///
/// ```ignore
/// editor.key_context_focused_with(
///     ContextFrame::new("Editor").fact("vim_mode", "normal").fact("extension", ext),
/// )
/// ```
///
/// Order is kept, and it is the declaration's: a resolver that prints a
/// frame prints what the view said, in the order it said it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContextFrame {
    idents: Vec<&'static str>,
    facts: Vec<(&'static str, std::borrow::Cow<'static, str>)>,
}

impl ContextFrame {
    /// A frame answering to one name — what `.key_context(name)` declares.
    pub fn new(ident: &'static str) -> ContextFrame {
        ContextFrame { idents: vec![ident], facts: Vec::new() }
    }

    /// One more name this frame answers to. Saying a name twice is saying
    /// it once.
    #[must_use]
    pub fn ident(mut self, ident: &'static str) -> ContextFrame {
        if !self.idents.contains(&ident) {
            self.idents.push(ident);
        }
        self
    }

    /// A fact, `key = value`. A second value for the same key replaces
    /// the first: a frame states one thing about each key, or a
    /// predicate on it could be true and false at once.
    #[must_use]
    pub fn fact(mut self, key: &'static str, value: impl Into<std::borrow::Cow<'static, str>>) -> ContextFrame {
        let value = value.into();
        match self.facts.iter_mut().find(|(stated, _)| *stated == key) {
            Some(fact) => fact.1 = value,
            None => self.facts.push((key, value)),
        }
        self
    }

    /// The names, in the order they were declared.
    pub fn idents(&self) -> &[&'static str] {
        &self.idents
    }

    /// Does the frame answer to `ident`?
    pub fn has_ident(&self, ident: &str) -> bool {
        self.idents.contains(&ident)
    }

    /// The facts, in the order they were declared.
    pub fn facts(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.facts.iter().map(|(key, value)| (*key, value.as_ref()))
    }

    /// The value the frame states for `key`, if it states one.
    pub fn fact_value(&self, key: &str) -> Option<&str> {
        self.facts.iter().find(|(stated, _)| *stated == key).map(|(_, value)| value.as_ref())
    }
}

impl std::fmt::Display for ContextFrame {
    /// `Editor vim_mode=normal` — the names, then the facts, the way a
    /// key-context debugger lists them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for ident in &self.idents {
            if !first {
                f.write_str(" ")?;
            }
            f.write_str(ident)?;
            first = false;
        }
        for (key, value) in &self.facts {
            if !first {
                f.write_str(" ")?;
            }
            write!(f, "{key}={value}")?;
            first = false;
        }
        Ok(())
    }
}

/// The key contexts in force for one stroke, INNERMOST FIRST.
///
/// The order is the precedence, and it has three bands:
///
/// 1. the declarations on the FOCUS PATH — the views the focused field
///    or box sits inside — deepest first;
/// 2. then the views declared MOUNTED that the keyboard is not inside
///    (a palette's list beside its field, a terminal in a dock), deeper
///    first and then in the tree's order;
/// 3. the global layer is beneath all of them, and is not a level: a
///    binding with no context is the resolver's own floor.
///
/// A declaration that waits for the keyboard (`.key_context_focused`)
/// and does not have it is not in the stack at all. Each level knows its
/// PARENT — the nearest declaration whose view holds its own — so a
/// predicate such as `Workspace > Editor` reads the tree, not the list.
#[derive(Clone, Debug, Default)]
pub struct ContextStack {
    levels: Vec<ContextLevel>,
}

/// One declaration in a [`ContextStack`].
#[derive(Clone, Debug)]
pub struct ContextLevel {
    frame: std::rc::Rc<ContextFrame>,
    parent: Option<usize>,
    on_focus_path: bool,
}

impl ContextLevel {
    /// A level of a stack built by hand — what a resolver's own tests feed
    /// it. The runtime builds the live one ([`Runtime::context_stack`]).
    ///
    /// [`Runtime::context_stack`]: crate::runtime::Runtime::context_stack
    pub fn new(frame: std::rc::Rc<ContextFrame>, parent: Option<usize>, on_focus_path: bool) -> ContextLevel {
        ContextLevel { frame, parent, on_focus_path }
    }

    /// What the view declared.
    pub fn frame(&self) -> &ContextFrame {
        &self.frame
    }

    /// The index, in the same stack, of the nearest declaration whose
    /// view holds this one's — `None` at a root of the tree.
    pub fn parent(&self) -> Option<usize> {
        self.parent
    }

    /// Does the keyboard sit inside the declaring view?
    pub fn on_focus_path(&self) -> bool {
        self.on_focus_path
    }
}

impl ContextStack {
    /// A stack from its levels, innermost first, each `parent` an index into
    /// the same list — a resolver's test fixture; the runtime builds the
    /// live one.
    pub fn new(levels: Vec<ContextLevel>) -> ContextStack {
        ContextStack { levels }
    }

    /// The levels, innermost first.
    pub fn levels(&self) -> &[ContextLevel] {
        &self.levels
    }

    /// Is any level answering to `ident`?
    pub fn has_ident(&self, ident: &str) -> bool {
        self.levels.iter().any(|level| level.frame.has_ident(ident))
    }

    /// The level at `index` and its ancestors, innermost first — the
    /// chain a `Parent > Child` predicate walks.
    pub fn ancestry(&self, index: usize) -> impl Iterator<Item = &ContextLevel> + '_ {
        std::iter::successors(self.levels.get(index), |level| level.parent.and_then(|parent| self.levels.get(parent)))
    }
}

/// What the strokes held in the air mean when the wait for more ends —
/// the stroke that leads nowhere, or the slow clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Fallback {
    /// Nothing by themselves: the first goes to the text door, as if no
    /// binding had ever seen it, and the rest are offered again.
    #[default]
    Replay,
    /// This action, by themselves: it runs, and what came after is
    /// offered again.
    Fire(ActionId),
    /// Nothing, on purpose (a `null`): they are spent.
    Swallow,
}

/// A resolver's answer for the strokes in the air.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyResolution {
    /// Bound to this action, and no longer binding starts with them.
    Action(ActionId),
    /// Bound to nothing on purpose — the keymap's `null`. The stroke is
    /// spent: nothing runs, and the field does not type it.
    Disabled,
    /// A longer binding starts with them: the keyboard is held. The
    /// fallback is what they mean if the next stroke leads nowhere or
    /// the slow clock runs out.
    Pending(Fallback),
    /// Nothing is bound.
    None,
}

/// The Strategy that turns strokes into intent (D435).
///
/// The runtime keeps what is mechanical — the alert's and the popover's
/// reserved keys, the strokes in the air, the slow clock, the replay —
/// and asks the resolver what the strokes MEAN against the live
/// [`ContextStack`]. A product with its own keymap language (predicates,
/// layers, `null`) answers here in its own words; the runtime's
/// `bind`/`bind_in`/`bind_sequence` tables stay beneath it as the floor,
/// asked only when the resolver answers [`KeyResolution::None`].
///
/// `strokes` is the whole sequence so far, the newest last. Each stroke
/// carries the key and the character it typed: a binding spelled by the
/// character (`~`, `}`) matches by [`Stroke::typed_pattern`], the key's
/// own spelling winning a tie.
pub trait KeyResolver {
    fn resolve(&self, strokes: &[Stroke], contexts: &ContextStack) -> KeyResolution;
}

impl<F> KeyResolver for F
where
    F: Fn(&[Stroke], &ContextStack) -> KeyResolution,
{
    fn resolve(&self, strokes: &[Stroke], contexts: &ContextStack) -> KeyResolution {
        self(strokes, contexts)
    }
}

/// How a shell's platform types — what the shared key gate needs to
/// know about a stroke that only the platform can say. Set once per
/// runtime ([`Runtime::set_key_platform`]); the default is the desktop's.
///
/// [`Runtime::set_key_platform`]: crate::runtime::Runtime::set_key_platform
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KeyPlatform {
    /// Option and a letter TYPES here (the Mac's `å`): a field that takes
    /// text keeps it without asking the keymap. Where Alt types nothing
    /// (Linux) it is a menu's access key, and the keymap is asked first.
    pub option_types: bool,
    /// A bare Enter reaches a field by the text road (a touch keyboard's
    /// return key), so the gate leaves it to that road instead of
    /// breaking the line itself.
    pub enter_by_text: bool,
}

impl Default for KeyPlatform {
    fn default() -> KeyPlatform {
        KeyPlatform { option_types: true, enter_by_text: false }
    }
}

impl KeyPlatform {
    /// Would the platform type this stroke into a field that takes text?
    pub fn types(&self, pattern: &KeyPattern) -> bool {
        pattern.is_text_input() && (self.option_types || !pattern.option)
    }
}

/// What a modifier report did ([`Runtime::modifiers_changed`]).
///
/// [`Runtime::modifiers_changed`]: crate::runtime::Runtime::modifiers_changed
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ModifierChange {
    /// The frame is worth presenting: a sink heard the change, or a lone
    /// modifier became a stroke someone took.
    pub present: bool,
    /// A modifier was pressed and let go alone, and the keymap took it —
    /// a shell with its own use for that tap (the menu bar's Alt) leaves
    /// it alone.
    pub tap_taken: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_input_patterns_are_recognized() {
        assert!(KeyPattern::key(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::command(Key::Char('a')).is_text_input());
        assert!(!KeyPattern::key(Key::Down).is_text_input());
        let option_a = KeyPattern { option: true, ..KeyPattern::key(Key::Char('a')) };
        assert!(option_a.is_text_input(), "option composes text on macOS");
    }

    #[test]
    fn a_frame_states_one_value_per_fact_and_each_name_once() {
        let frame = ContextFrame::new("Editor")
            .ident("Editor")
            .ident("Vim")
            .fact("mode", "normal")
            .fact("extension", String::from("rs"))
            .fact("mode", "insert");
        assert_eq!(frame.idents(), ["Editor", "Vim"]);
        assert_eq!(frame.fact_value("mode"), Some("insert"), "the second value replaced the first");
        assert_eq!(frame.fact_value("operator"), None);
        assert!(frame.has_ident("Vim") && !frame.has_ident("Terminal"));
        assert_eq!(frame.to_string(), "Editor Vim mode=insert extension=rs", "in declaration order");
    }

    #[test]
    fn the_platform_says_whether_option_types() {
        let option_a = KeyPattern::option(Key::Char('a'));
        assert!(KeyPlatform::default().types(&option_a), "the Mac composes å");
        let linux = KeyPlatform { option_types: false, ..KeyPlatform::default() };
        assert!(!linux.types(&option_a), "Alt and a letter is a menu's key there");
        assert!(linux.types(&KeyPattern::key(Key::Char('a'))), "a bare letter still types");
        assert!(!linux.types(&KeyPattern::key(Key::Insert)), "a named key never types");
    }

    #[test]
    fn only_the_four_modifiers_are_modifier_keys() {
        assert!([Key::Shift, Key::Command, Key::Option, Key::Control].iter().all(Key::is_modifier));
        assert!(![Key::Insert, Key::Escape, Key::Char('s'), Key::F(1)].iter().any(Key::is_modifier));
    }

    #[test]
    fn a_plain_stroke_holds_no_accelerator() {
        assert!(KeyPattern::key(Key::Enter).is_plain());
        let shifted = KeyPattern { shift: true, ..KeyPattern::key(Key::Enter) };
        assert!(shifted.is_plain(), "shift types, it does not command");
        assert!(!KeyPattern::command(Key::Enter).is_plain(), "the app owns the chord");
    }
}
