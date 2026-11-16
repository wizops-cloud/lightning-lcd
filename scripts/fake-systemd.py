#!/usr/bin/python3
"""Faux gestionnaire systemd pour les captures de la documentation.

Lancé par `scripts/screenshots.sh` sur un bus de session PRIVÉ
(`dbus-run-session`) : il réclame `org.freedesktop.systemd1` et répond aux
quelques appels de `lightning-settings` pour que la page « Service » montre le
service « en cours d'exécution » et « lancé à l'ouverture de session ».

Il ne connaît que l'unité `lightning-dashboard.service`, ne démarre ni
n'arrête rien et refuse de tourner sur le vrai bus de session (il exige
`LIGHTNING_SCREENSHOT_BUS=1`, posé par le script de captures).
"""

import os
import sys
import warnings

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

# register_object (forme à rappels) reste la plus portable entre versions de PyGObject.
warnings.filterwarnings("ignore", category=DeprecationWarning)

UNIT = "lightning-dashboard.service"
UNIT_PATH = "/org/freedesktop/systemd1/unit/lightning_2ddashboard_2eservice"

XML = """
<node>
  <interface name="org.freedesktop.systemd1.Manager">
    <method name="GetUnitFileState"><arg type="s" direction="in"/><arg type="s" direction="out"/></method>
    <method name="LoadUnit"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="GetUnit"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="StartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="StopUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="RestartUnit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
    <method name="Reload"/>
  </interface>
  <interface name="org.freedesktop.systemd1.Unit">
    <property name="Id" type="s" access="read"/>
    <property name="ActiveState" type="s" access="read"/>
    <property name="LoadState" type="s" access="read"/>
    <property name="UnitFileState" type="s" access="read"/>
  </interface>
</node>
"""

UNIT_PROPS = {
    "Id": UNIT,
    "ActiveState": "active",
    "LoadState": "loaded",
    "UnitFileState": "enabled",
}


def manager_call(conn, sender, path, iface, method, params, invocation):
    args = params.unpack()
    if args and args[0] != UNIT:
        invocation.return_dbus_error(
            "org.freedesktop.systemd1.NoSuchUnit", f"Unit {args[0]} not found."
        )
        return
    if method == "GetUnitFileState":
        invocation.return_value(GLib.Variant("(s)", ("enabled",)))
    elif method == "Reload":
        invocation.return_value(None)
    else:
        # LoadUnit, GetUnit, Start/Stop/RestartUnit : rien n'est lancé.
        invocation.return_value(GLib.Variant("(o)", (UNIT_PATH,)))


def unit_property(conn, sender, path, iface, name):
    return GLib.Variant("s", UNIT_PROPS[name])


def main():
    if os.environ.get("LIGHTNING_SCREENSHOT_BUS") != "1":
        sys.exit("fake-systemd : réservé au bus privé de scripts/screenshots.sh")
    info = Gio.DBusNodeInfo.new_for_xml(XML)
    loop = GLib.MainLoop()

    def on_bus(conn, _name):
        conn.register_object(
            "/org/freedesktop/systemd1", info.interfaces[0], manager_call, None, None
        )
        conn.register_object(UNIT_PATH, info.interfaces[1], None, unit_property, None)

    Gio.bus_own_name(
        Gio.BusType.SESSION,
        "org.freedesktop.systemd1",
        Gio.BusNameOwnerFlags.DO_NOT_QUEUE,
        on_bus,
        None,
        lambda *_: loop.quit(),
    )
    loop.run()


if __name__ == "__main__":
    main()
