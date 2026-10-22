#!/usr/bin/env python3
"""Bind a global KDE shortcut that launches a .desktop file: bind-shortcut.py snip-shot.desktop Meta+Shift+X

Several alternative keys can be given comma-separated, e.g. "Dead_Acute,´" (dead key and plain character).
Remove a binding again with: bind-shortcut.py --unbind snip-shot.desktop"""
import sys
from gi.repository import Gio, GLib

MODS = {"meta": 0x10000000, "ctrl": 0x04000000, "alt": 0x08000000, "shift": 0x02000000}
NAMED = {"print": 0x01000009, "space": 0x20, "return": 0x01000004, "enter": 0x01000004,
         "dead_acute": 0x01001251, "dead_grave": 0x01001250, "dead_diaeresis": 0x01001257,
         "dead_circumflex": 0x01001252, "dead_tilde": 0x01001253}


def qt_key(spec: str) -> int:
    *mods, key = spec.split("+") if spec != "+" else ["+"]
    code = sum(MODS[m.lower()] for m in mods)
    k = key.lower()
    if k in NAMED:
        return code | NAMED[k]
    if len(key) == 1:
        return code | ord(key.upper())
    if k[0] == "f" and k[1:].isdigit():
        return code | (0x01000030 + int(k[1:]) - 1)
    sys.exit(f"unsupported key: {key}")


bus = Gio.bus_get_sync(Gio.BusType.SESSION)


def call(method, args, sig):
    return bus.call_sync("org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel", method,
                         GLib.Variant(sig, args), None, Gio.DBusCallFlags.NONE, -1, None).unpack()


if sys.argv[1:2] == ["--unbind"]:
    desktop = sys.argv[2]
    action = [desktop, "_launch", desktop, desktop]
    try:
        call("setForeignShortcut", (action, []), "(asai)")
        call("unregister", (desktop, "_launch"), "(ss)")
    except GLib.Error as e:
        print(f"{desktop}: {e.message}", file=sys.stderr)
    sys.exit(0)

desktop, spec = sys.argv[1], sys.argv[2]
name = sys.argv[3] if len(sys.argv) > 3 else desktop
keys = [qt_key(s) for s in spec.split(",")]
taken = [(c, a) for k in keys for a, c, *_ in call("getGlobalShortcutsByKey", (k,), "(i)")[0] if c != desktop]
if taken:
    sys.exit(f"{spec} is already used by {taken}; pick another key (System Settings > Shortcuts)")
action = [desktop, "_launch", name, name]
call("doRegister", (action,), "(as)")
call("setForeignShortcut", (action, keys), "(asai)")
print(f"{spec} -> {name}")
