"""Isolated Settings protocol fixture; never connects to a user's portal."""
import os
from gi.repository import Gio, GLib

BUS = 'org.freedesktop.portal.Desktop'
PATH = '/org/freedesktop/portal/desktop'
IFACE = 'org.freedesktop.portal.Settings'
value = 1
connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
rogue = Gio.DBusConnection.new_for_address_sync(
    os.environ['DBUS_SESSION_BUS_ADDRESS'],
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    None, None)
xml = '''<node>
<interface name="org.freedesktop.portal.Settings"><method name="Read">
<arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="out"/>
</method><signal name="SettingChanged"><arg type="s"/><arg type="s"/><arg type="v"/></signal></interface>
<interface name="org.bunnyui.AppearanceProbe"><method name="Control">
<arg type="s" direction="in"/><arg type="u" direction="in"/></method></interface>
</node>'''

def name_call(method, signature, values):
    return connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
        'org.freedesktop.DBus', method, GLib.Variant(signature, values), None,
        Gio.DBusCallFlags.NONE, 1000, None)

def handle(conn, sender, path, interface, method, parameters, invocation):
    global value
    if interface == IFACE:
        assert method == 'Read'
        assert parameters.unpack() == ('org.freedesktop.appearance', 'color-scheme')
        invocation.return_value(GLib.Variant('(v)', (GLib.Variant('v', GLib.Variant('u', value)),)))
        return
    mode, number = parameters.unpack()
    if mode == 'release':
        value = number
        name_call('ReleaseName', '(s)', (BUS,))
    elif mode == 'acquire':
        value = number
        assert name_call('RequestName', '(su)', (BUS, 0)).unpack() == (1,)
    else:
        if mode == 'set':
            value = number
        source = rogue if mode == 'rogue' else conn
        source.emit_signal(None, PATH + '/wrong' if mode == 'path' else PATH, IFACE, 'SettingChanged',
            GLib.Variant('(ssv)', ('org.freedesktop.appearance',
                'unrelated-key' if mode == 'key' else 'color-scheme',
                GLib.Variant('s', 'bad type') if mode == 'type' else GLib.Variant('u', number))))
        source.flush_sync(None)
    invocation.return_value(GLib.Variant('()', ()))

for info in Gio.DBusNodeInfo.new_for_xml(xml).interfaces:
    connection.register_object(PATH, info, handle, None, None)
assert name_call('RequestName', '(su)', (BUS, 0)).unpack() == (1,)
print(connection.get_unique_name(), flush=True)
GLib.MainLoop().run()
