# Keyboard controls

Native layout records text fields, buttons, checkboxes and eligible custom
controls in reading order. Tab moves forward, Shift+Tab moves backward, and
both wrap within the current popup or modal. Navigation reveals controls in
nonvirtualized scroll content, including nested scroll regions. Rows that have
not been materialized by a virtual list do not participate.

A focused field's `EditingStrategy` or custom element receives the stroke
first. Returning `handled` preserves an editor's own Tab behavior. Otherwise
the runtime traverses the controls before offering the stroke to application
key bindings. Generic `.on_click` regions are not implicitly keyboard controls.
Custom elements participate when `accepts_keys()` is true and
`leaves_keyboard()` is false. Flow-DOM browser navigation remains browser-owned.

`runtime.focus_named("save")` resolves an `.id("save")` after layout and uses
the same eligibility and scroll reveal. An exact field name retains priority;
a name enclosing multiple eligible controls is otherwise ambiguous. Controls
behind the active popup or modal cannot be focused by this method. Closing a
popup returns focus to its previous owner when that control still exists. A
popover that places no control — a completion list, a preview card — is a
picture beside the keyboard's owner, not a dialog: it opens no scope, focus
stays where it was, and the walk never enters it.

An interactive completion list uses `.popover_with_focus(...,
PopoverFocus::Preserve, ...)`: opening it keeps the editor focused even when
its choices are real buttons. Those controls remain in the existing Tab order
and keep their accessibility actions. Use `.leaves_keyboard()` on a suggestion
button to retain the editor during pointer selection. The ordinary `.popover`
and `.popover_on_dismiss` continue to create a scope when they contain controls.
The explicit policy changes native navigation only; flow DOM remains browser-owned.

```rust,ignore
vstack!(
    text_field("Description", description.binding()),
    checkbox(text("Reimbursable"), reimbursable.binding()),
    button(text("Save"), save).disabled(!valid).id("save"),
)
```

Buttons activate with Enter or Space. Checkboxes toggle their `Binding<bool>`
with Space and expose a real checkbox role and checked value to native
accessibility adapters. Pointer activation uses the same callback and gives
the control focus; an explicit `.leaves_keyboard()` preserves the previous
owner on pointer activation. Focus rings do not start a caret timer.

`.disabled(true)` on a button or checkbox prevents focus and activation through
pointer, keyboard and accessibility requests. Disabling a pressed control
cancels its pending click; disabling a focused control releases the keyboard.
Its label and current value remain visible to assistive technology, with
`enabled = false`. Changing the application's binding updates both rendering
and the checkbox's native value.

A toggle is a checkbox drawn as a switch: one Tab stop, Space flips it, and
Enter is left to the form around it.

### Controls that travel as a group

Some controls are several targets and one Tab stop. A segmented control is the
first: `segmented(mode.binding(), [(Mode::List, "List"), (Mode::Grid, "Grid")])`.

| Key | Inside a group |
| --- | --- |
| Tab, Shift+Tab | Leaves the group whole. Arriving, it lands on the selected member, or on the first when none is selected. |
| Left, Right | Moves by one member. The two swap in a right-to-left scene. |
| Up, Down | Moves by one member in a group of one row or one column; moves by one row in a grid. |
| Home, End | Jumps to the first or the last member. |
| Space, Enter | Selects the member that has the keyboard. |

A group of one row or one column goes round at its ends. A grid stops at its
edges. In a segmented control the member the arrows reach is selected as they
reach it, as in a set of radio buttons. A key that has nowhere to go is not
handled, so the app's own bindings can take it.

In a group that selects as the arrows move, the keyboard and the selection are
one thing. When the app writes another value while the keyboard is on a member,
the keyboard goes to the member that is now selected. A write never takes the
keyboard from a control outside the group.

A group stays one stop when its members change. When the member under the
keyboard leaves the scene, or can no longer be chosen, and the group is still
there, the keyboard goes to the selected member of the group, or to its first.

### The days of a calendar

`calendar(due.binding())` shows one month. Its two buttons are Tab stops, and
its days are one more: a group of seven columns that selects as the arrows
move. The days are a window on a longer line, so the group does not go round.

| Key | On a day |
| --- | --- |
| Left, Right | One day back or forward. The two swap in a right-to-left scene. At the first or the last day of the month, into the month before or after. |
| Up, Down | One week back or forward, into the month before or after where the week leaves the month. |
| Page Up, Page Down | The same day of the month before or after, or its last day where that month is shorter. |
| Home, End | The first or the last day of the month that can be chosen. |
| Space, Enter | In the list of a date picker, on the chosen day: closes the list. |

A key that would reach a day out of the calendar's range does nothing. Page Up
and Page Down stop at the first and the last day of the range.

`date_picker(due.binding())` is a button that opens the same calendar in a
list. The list opens with the keyboard on the chosen day, also though the two
buttons stand before the days. The arrows write the day as they move. Escape
closes the list and leaves what they wrote.

### The pages of a navigation stack

`navigation_stack(path, root)` shows one page. A page under another one has no
Tab stop: Tab goes round the bar and the page on top. On a page that was
pushed, the first stop is the button that goes back.

| Key | With a page to leave |
| --- | --- |
| ⌘[ on the Mac, Ctrl+[ elsewhere | The page on top leaves. |
| Alt+Left | The same. A text field that has the keyboard keeps it: there it moves one word. |
| Escape | Nothing. It closes a popover and answers an alert, and it never leaves a page. |

At the root the two keys are not the stack's, and an app's own bindings take
them. When a page leaves holding the keyboard, the keyboard goes back to the
link that opened it.

These contracts have runtime and native protocol regression tests. They do not
establish human screen-reader or operating-system keyboard qualification.

## Application-drawn controls

`button(label, action).plain()` keeps the native focus, keyboard, pointer/touch
and accessibility contract while the label supplies all geometry and visuals.
It adds no theme chrome or padding; ordinary buttons keep their default style.
A custom two-state label can use `.checked(value)` to expose its real checked
state without adding a glyph. The application still owns the value and callback;
Space and accessibility activation invoke that callback. Disabled controls remain
unavailable through every activation path.

## Edit commands from panels

A containing editor can declare `.on_edit(Edit::Undo, undo)` and
`.on_edit(Edit::Redo, redo)`. When a non-text control inside it holds focus,
the nearest ancestor registered for that command answers both its standard
accelerator and the native Edit menu. Sibling panels and other window scenes
do not share responders. Registrations survive retained renders and disappear
with their view. An open alert prevents dispatch into the application behind it.

Fields and custom editors keep their own commands, even when they cannot
currently undo. A read-only `.on_copy` target keeps its local copy answer.
Ordinary fields retain bounded local text and selection history across renders
and focus changes. An empty history never falls through to application source.
Custom editing strategies keep ownership of their history. An ancestor handler
owns its operation, including clipboard work for Cut, Copy or Paste.
This runtime contract applies to native/canvas controls; flow DOM keeps the
browser's native editing behavior.

Inline transient forms can use `.restore_focus_on_close()` on their containing
view. It remembers the control focused when the form appears, independently of
which child later owns the keyboard. Closing from Apply, Cancel or a field
returns focus to that invoker if it still exists. Moving focus outside the form
before closing preserves the new owner. Nested forms can close together without
reviving missing controls. The modifier neither takes focus on opening nor traps
Tab; child fields keep their text history and composition. Like panel responders,
this contract governs native/canvas input; flow DOM remains browser-owned.
