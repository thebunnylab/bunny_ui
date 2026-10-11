"""Real libatspi client: registration, discovery and model-changing requests."""
import os
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, Gio, GLib


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

    if os.environ.get("BUNNY_PROBE_MODAL_FIRST") == "1":
        modal = wait_for(lambda: named("Dismiss modal"), "initial modal not exposed")
        assert named("Description") is None
        assert modal.get_action_iface().do_action(0)
        print("AT-SPI first query reached the initial modal", flush=True)

    field = wait_for(lambda: named("Description"), "native AT-SPI accessible element missing: Description")
    assert field.get_role() == Atspi.Role.ENTRY
    assert Atspi.Text.get_text(field, 0, -1) == "Lunch"
    print("AT-SPI discovery: Description, ENTRY, Lunch", flush=True)
    identity = field.get_accessible_id()
    form_root = field.get_parent()
    app_root = field.get_application()
    assert app_root.get_child_at_index(0).get_name() == "Bunny AT-SPI witness"
    assert app_root.get_child_at_index(1).get_name() == "AT-SPI keeper"
    assert form_root.get_index_in_parent() == 0
    assert app_root.get_child_at_index(1).get_index_in_parent() == 1
    session = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    address = session.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress",
                                None, None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]
    wire = Gio.DBusConnection.new_for_address_sync(address,
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
        None, None)
    names = wire.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames",
                          None, None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]
    bus_name = None
    for name in names:
        if not name.startswith(":"):
            continue
        pid = wire.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetConnectionUnixProcessID",
                             GLib.Variant("(s)", (name,)), None, Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]
        if pid == process.pid:
            bus_name = name
            break
    assert bus_name is not None, "fixture did not own an accessibility-bus connection"

    def wire_unavailable(path, interface, method, arguments):
        try:
            wire.call_sync(bus_name, path, interface, method, arguments, None, Gio.DBusCallFlags.NONE, 3000, None)
        except GLib.Error as error:
            assert Gio.DBusError.get_remote_error(error) == "org.freedesktop.DBus.Error.UnknownObject", error
            return
        raise AssertionError("retired object accepted a direct D-Bus request")
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
    stale_path = stale.get_accessible_id()
    assert named("Reverse rows").get_action_iface().do_action(0)
    order = [node.get_name() for node in descendants(form_root)]
    assert order.index("Row 2") < order.index("Row 1")
    assert named("Row 2").get_accessible_id() == stale_path
    assert named("Remove row").get_action_iface().do_action(0)
    unavailable(lambda: Atspi.Action.do_action(stale, 0))
    wire_unavailable(stale_path, "org.a11y.atspi.Action", "DoAction", GLib.Variant("(i)", (0,)))
    assert named("Open modal").get_action_iface().do_action(0)
    dismiss = wait_for(lambda: named("Dismiss modal"), "modal not exposed")
    assert named("Updated name") is None
    unavailable(lambda: Atspi.EditableText.set_text_contents(updated, "blocked"))
    wire_unavailable(identity, "org.a11y.atspi.EditableText", "SetTextContents", GLib.Variant("(s)", ("blocked",)))
    assert dismiss.get_action_iface().do_action(0)
    restored = wait_for(lambda: named("Updated name"), "form not restored")
    assert Atspi.Text.get_text(restored, 0, -1) == "Dinner 👩‍🚀"
    restored_path = restored.get_accessible_id()
    assert restored_path != identity, "a modal-retired handle was revived"
    wire_unavailable(identity, "org.a11y.atspi.EditableText", "SetTextContents", GLib.Variant("(s)", ("revived",)))
    assert named("Close form").get_action_iface().do_action(0)
    unavailable(lambda: Atspi.EditableText.set_text_contents(restored, "closed"))
    restored.clear_cache()
    # libatspi normalizes a defunct object's name to an empty string. The
    # separate wire assertion below requires the server's exact error.
    try:
        assert restored.get_name() == ""
    except GLib.Error:
        pass
    wire_unavailable(restored_path, "org.freedesktop.DBus.Properties", "Get", GLib.Variant("(ss)", ("org.a11y.atspi.Accessible", "Name")))
    print("AT-SPI removal, modal isolation and closed-window safety passed", flush=True)
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
