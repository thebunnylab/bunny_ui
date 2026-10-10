"""Real libatspi client: registration, discovery and model-changing requests."""
import os
import subprocess
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi


def descendants(node):
    yield node
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            yield from descendants(child)


def wait_for(check, message, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        assert process.poll() is None, "native fixture exited before the client completed"
        result = check()
        if result is not None:
            return result
        time.sleep(0.05)
    raise AssertionError(message)


process = subprocess.Popen([sys.argv[1]], env={**os.environ, "BUNNY_ACCESSIBILITY_PROBE": "1"})
try:
    Atspi.init()
    desktop = Atspi.get_desktop(0)

    def discover():
        desktop.clear_cache()
        return next((node for node in descendants(desktop)
                     if node.get_name() == "Description"), None)

    field = wait_for(discover, "native AT-SPI accessible element missing: Description")
    assert field.get_role() == Atspi.Role.ENTRY
    assert field.get_text_iface().get_text(0, -1) == "Lunch"
    print("AT-SPI discovery: Description, ENTRY, Lunch", flush=True)
finally:
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
