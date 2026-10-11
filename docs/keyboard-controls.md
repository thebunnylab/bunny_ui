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
popup returns focus to its previous owner when that control still exists.

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

These contracts have runtime and native protocol regression tests. They do not
establish human screen-reader or operating-system keyboard qualification.
