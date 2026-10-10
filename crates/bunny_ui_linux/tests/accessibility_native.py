"""Real libatspi client: registration, discovery and model-changing requests."""
import os
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib


def descendants(node):
    yield node
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            yield from descendants(child)


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        assert process.poll() is None, "native fixture exited before the client completed"
        result = check()
        if result is not None:
            return result
        time.sleep(0.05)
    raise AssertionError(message)


process = subprocess.Popen([sys.argv[1]], env={**os.environ, "BUNNY_ACCESSIBILITY_PROBE": "1"})
try:
    Atspi.init()
    events = []

    def record(event, *_):
        events.append(event.type)

    listener = Atspi.EventListener.new(record)
    for kind in ["object:property-change:accessible-name", "object:text-changed", "object:state-changed:focused", "object:children-changed"]:
        assert listener.register(kind)
    desktop = Atspi.get_desktop(0)

    def named(name):
        desktop.clear_cache()
        return next((node for node in descendants(desktop)
                     if node.get_name() == name), None)

    field = wait_for(lambda: named("Description"), "native AT-SPI accessible element missing: Description")
    assert field.get_role() == Atspi.Role.ENTRY
    assert Atspi.Text.get_text(field, 0, -1) == "Lunch"
    print("AT-SPI discovery: Description, ENTRY, Lunch", flush=True)
    identity = field.get_accessible_id()
    assert field.get_component_iface().grab_focus()
    field.clear_cache()
    assert field.get_state_set().contains(Atspi.StateType.FOCUSED)
    bounds = field.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
    assert bounds.width > 0 and bounds.height > 0
    if os.environ["BUNNY_BACKEND"] == "x11":
        screen = field.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
        assert screen.width == bounds.width and screen.height == bounds.height
    else:
        try:
            field.get_component_iface().get_extents(Atspi.CoordType.SCREEN)
            raise AssertionError("Wayland returned fabricated screen coordinates")
        except GLib.Error:
            pass
    secret = named("Password")
    assert secret.get_role() == Atspi.Role.PASSWORD_TEXT
    assert Atspi.Text.get_text(secret, 0, -1) == ""
    assert secret.get_text_iface().get_character_count() == 0
    assert field.get_editable_text_iface().set_text_contents("Dinner 👩‍🚀")
    assert Atspi.Text.get_text(field, 0, -1) == "Dinner 👩‍🚀"
    assert named("Save").get_action_iface().do_action(0)
    updated = wait_for(lambda: named("Updated name"), "native name did not follow Save")
    assert updated.get_accessible_id() == identity
    wait_for(lambda: True if any("accessible-name" in event for event in events) and any("text-changed" in event for event in events) else None,
             "AT-SPI name/text events were not delivered")
    print("AT-SPI model edit, focus, bounds, privacy, stable identity and events passed", flush=True)

    def unavailable(call):
        try:
            value = call()
        except GLib.Error:
            return
        assert value is False, f"retired accessible operation succeeded: {value!r}"

    stale = named("Row 2")
    assert named("Remove row").get_action_iface().do_action(0)
    unavailable(lambda: stale.get_action_iface().do_action(0))
    assert named("Open modal").get_action_iface().do_action(0)
    dismiss = wait_for(lambda: named("Dismiss modal"), "modal not exposed")
    assert named("Updated name") is None
    unavailable(lambda: updated.get_editable_text_iface().set_text_contents("blocked"))
    assert dismiss.get_action_iface().do_action(0)
    restored = wait_for(lambda: named("Updated name"), "form not restored")
    assert Atspi.Text.get_text(restored, 0, -1) == "Dinner 👩‍🚀"
    assert named("Close form").get_action_iface().do_action(0)
    unavailable(lambda: restored.get_editable_text_iface().set_text_contents("closed"))
    restored.clear_cache()
    unavailable(restored.get_name)
    print("AT-SPI removal, modal isolation and closed-window safety passed", flush=True)
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
