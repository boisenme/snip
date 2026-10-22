#!/usr/bin/env python3
"""Snip settings: record global shortcuts for `snip shot` / `snip ocr` and toggle dead keys.

Shortcuts are stored by KDE's kglobalaccel (as launch shortcuts of the snip-*.desktop files),
so they work without this window running.
"""
import subprocess
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gdk, Gio, GLib, Gtk  # noqa: E402

ACTIONS = [
    ("snip-shot.desktop", "Screenshot", "Select a region, annotate with 1 / 2 / 3, copy"),
    ("snip-ocr.desktop", "Copy text (OCR)", "Select a region and copy the text in it"),
]

# Qt modifier bits and key codes, as kglobalaccel expects them.
META, CTRL, ALT, SHIFT = 0x10000000, 0x04000000, 0x08000000, 0x02000000
MOD_MASK = META | CTRL | ALT | SHIFT
SPECIAL = {  # GDK keyval -> (Qt key, label)
    0xFF61: (0x01000009, "Print"), 0xFF13: (0x01000008, "Pause"), 0xFF14: (0x01000026, "Scroll Lock"),
    0xFF63: (0x01000006, "Insert"), 0xFFFF: (0x01000007, "Delete"), 0xFF50: (0x01000010, "Home"),
    0xFF57: (0x01000011, "End"), 0xFF51: (0x01000012, "Left"), 0xFF52: (0x01000013, "Up"),
    0xFF53: (0x01000014, "Right"), 0xFF54: (0x01000015, "Down"), 0xFF55: (0x01000016, "Page Up"),
    0xFF56: (0x01000017, "Page Down"), 0xFF0D: (0x01000004, "Return"), 0xFF09: (0x01000001, "Tab"),
    0xFF67: (0x01000055, "Menu"), 0x20: (0x20, "Space"),
}
QT_LABEL = {q: l for q, l in SPECIAL.values()}
# Dead keys: GDK 0xfe50.. and Qt 0x01001250.. share their order; the plain character each one
# becomes on a layout without dead keys.
DEAD_CHARS = "`´^~¯˘˙¨˚˝ˇ¸˛"
DEAD_PLAIN = {0: 0x60, 1: 0xB4, 2: 0x5E, 3: 0x7E, 7: 0xA8}

MODIFIER_KEYVALS = {
    Gdk.KEY_Shift_L, Gdk.KEY_Shift_R, Gdk.KEY_Control_L, Gdk.KEY_Control_R, Gdk.KEY_Alt_L,
    Gdk.KEY_Alt_R, Gdk.KEY_Super_L, Gdk.KEY_Super_R, Gdk.KEY_Meta_L, Gdk.KEY_Meta_R,
    Gdk.KEY_ISO_Level3_Shift, Gdk.KEY_Caps_Lock, Gdk.KEY_Num_Lock,
}

bus = Gio.bus_get_sync(Gio.BusType.SESSION)


def kga(method, sig=None, args=None):
    params = GLib.Variant(sig, args) if sig else None
    return bus.call_sync("org.kde.kglobalaccel", "/kglobalaccel", "org.kde.KGlobalAccel", method,
                         params, None, Gio.DBusCallFlags.NONE, -1, None).unpack()


def action_id(desktop, name):
    return [desktop, "_launch", name, name]


def qt_label(code):
    parts = [n for bit, n in ((META, "Meta"), (CTRL, "Ctrl"), (ALT, "Alt"), (SHIFT, "Shift")) if code & bit]
    k = code & ~MOD_MASK
    if 0x01001250 <= k < 0x01001250 + len(DEAD_CHARS):
        parts.append(DEAD_CHARS[k - 0x01001250])
    elif 0x01000030 <= k <= 0x01000052:
        parts.append(f"F{k - 0x01000030 + 1}")
    elif k in QT_LABEL:
        parts.append(QT_LABEL[k])
    elif k < 0x110000:
        parts.append(chr(k))
    else:
        parts.append(hex(k))
    return "+".join(parts)


def to_qt(keyval, state):
    """GDK key press -> list of Qt codes (a dead key also gets its no-dead-keys twin), or None."""
    mods = 0
    if state & Gdk.ModifierType.SUPER_MASK or state & Gdk.ModifierType.META_MASK:
        mods |= META
    if state & Gdk.ModifierType.CONTROL_MASK:
        mods |= CTRL
    if state & Gdk.ModifierType.ALT_MASK:
        mods |= ALT
    if state & Gdk.ModifierType.SHIFT_MASK:
        mods |= SHIFT
    if 0xFE50 <= keyval < 0xFE50 + len(DEAD_CHARS):
        i = keyval - 0xFE50
        codes = [mods | (0x01001250 + i)]
        if i in DEAD_PLAIN:
            codes.append(mods | DEAD_PLAIN[i])
        return codes
    if 0xFFBE <= keyval <= 0xFFE0:
        return [mods | (0x01000030 + keyval - 0xFFBE)]
    if keyval in SPECIAL:
        return [mods | SPECIAL[keyval][0]]
    ch = chr(Gdk.keyval_to_unicode(keyval) or 0)
    if ch > " ":
        up = ch.upper()
        return [mods | ord(up if len(up) == 1 else ch)]
    return None


def current_keys(desktop, name):
    try:
        return [k for k in kga("shortcut", "(as)", (action_id(desktop, name),))[0] if k]
    except GLib.Error:
        return []


def dead_keys_off():
    r = subprocess.run(["kreadconfig6", "--file", "kxkbrc", "--group", "Layout", "--key", "VariantList"],
                       capture_output=True, text=True)
    return "nodeadkeys" in r.stdout


def current_layout():
    r = subprocess.run(["kreadconfig6", "--file", "kxkbrc", "--group", "Layout", "--key", "LayoutList"],
                       capture_output=True, text=True)
    if r.stdout.strip():
        return r.stdout.strip().split(",")[0]
    r = subprocess.run(["localectl", "status"], capture_output=True, text=True)
    for line in r.stdout.splitlines():
        if "X11 Layout:" in line:
            return line.split(":", 1)[1].strip().split(",")[0]
    return "us"


def set_dead_keys_off(off):
    layout = current_layout()
    w = ["kwriteconfig6", "--file", "kxkbrc", "--group", "Layout", "--key"]
    subprocess.run(w + ["Use", "true"], check=True)
    subprocess.run(w + ["LayoutList", layout], check=True)
    subprocess.run(w + ["VariantList", "nodeadkeys" if off else ""], check=True)
    bus.emit_signal(None, "/Layouts", "org.kde.keyboard", "reloadConfig", None)


class ShortcutRow(Adw.ActionRow):
    def __init__(self, win, desktop, name, subtitle):
        super().__init__(title=name, subtitle=subtitle)
        self.win, self.desktop, self.name = win, desktop, name
        self.button = Gtk.Button(valign=Gtk.Align.CENTER)
        self.button.connect("clicked", lambda *_: win.start_recording(self))
        clear = Gtk.Button(icon_name="edit-clear-symbolic", valign=Gtk.Align.CENTER, tooltip_text="Remove shortcut")
        clear.add_css_class("flat")
        clear.connect("clicked", lambda *_: self.apply([]))
        self.add_suffix(self.button)
        self.add_suffix(clear)
        self.refresh()

    def refresh(self):
        keys = current_keys(self.desktop, self.name)
        # a dead key and its plain twin are one shortcut to the user
        shown = [k for k in keys if not any(k == ((d & MOD_MASK) | DEAD_PLAIN.get((d & ~MOD_MASK) - 0x01001250, -1))
                                             for d in keys)]
        self.button.set_label(" or ".join(qt_label(k) for k in shown) if shown else "Not set")
        self.button.remove_css_class("suggested-action")

    def apply(self, codes):
        for c in codes:
            owners = [(comp, act) for act, comp, *_ in kga("getGlobalShortcutsByKey", "(i)", (c,))[0]
                      if comp != self.desktop]
            if owners:
                self.win.toast(f"{qt_label(c)} is already used by {owners[0][0]} ({owners[0][1]})")
                self.refresh()
                return
        aid = action_id(self.desktop, self.name)
        kga("doRegister", "(as)", (aid,))
        kga("setForeignShortcut", "(asai)", (aid, codes))
        self.refresh()
        self.win.update_dead_key_hint()


class Window(Adw.ApplicationWindow):
    def __init__(self, app):
        super().__init__(application=app, title="Snip Settings", default_width=560, default_height=540)
        self.recording = None
        self.toasts = Adw.ToastOverlay()
        page = Adw.PreferencesPage()

        group = Adw.PreferencesGroup(title="Shortcuts",
                                     description="Click a shortcut, then press the keys. Esc cancels, Backspace clears.")
        self.rows = [ShortcutRow(self, *a) for a in ACTIONS]
        for r in self.rows:
            group.add(r)
        page.add(group)

        kb = Adw.PreferencesGroup(title="Keyboard")
        self.dead = Adw.SwitchRow(title="Type ´ ¨ ^ ~ ` directly (no dead keys)",
                                  subtitle="Dead keys can't trigger shortcuts. Turn this on to use ´ or ¨ as a "
                                           "shortcut. It switches the keyboard to the “no dead keys” variant after your next login.")
        self.dead.set_active(dead_keys_off())
        self.dead.connect("notify::active", self.on_dead_toggled)
        kb.add(self.dead)
        open_kcm = Adw.ActionRow(title="Keyboard layout settings",
                                 subtitle=f"Apply right away: pick the “no dead keys” variant of your layout ({current_layout()}) under Layouts",
                                 activatable=True)
        open_kcm.add_suffix(Gtk.Image(icon_name="go-next-symbolic"))
        open_kcm.connect("activated", lambda *_: subprocess.Popen(["systemsettings", "kcm_keyboard"]))
        kb.add(open_kcm)
        page.add(kb)

        self.hint = Adw.Banner(title="A shortcut uses a dead key. It won't fire until dead keys are off.",
                               button_label="Turn dead keys off")
        self.hint.connect("button-clicked", lambda *_: self.dead.set_active(True))
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        box.append(Adw.HeaderBar())
        box.append(self.hint)
        box.append(page)
        self.toasts.set_child(box)
        self.set_content(self.toasts)
        self.update_dead_key_hint()

        keys = Gtk.EventControllerKey(propagation_phase=Gtk.PropagationPhase.CAPTURE)
        keys.connect("key-pressed", self.on_key)
        self.add_controller(keys)
        self.connect("close-request", lambda *_: self.stop_recording() or False)

    def toast(self, msg):
        self.toasts.add_toast(Adw.Toast(title=msg, timeout=4))

    def update_dead_key_hint(self):
        uses_dead = any(0x01001250 <= (k & ~MOD_MASK) < 0x01001260
                        for r in self.rows for k in current_keys(r.desktop, r.name))
        self.hint.set_revealed(uses_dead and not dead_keys_off())

    def on_dead_toggled(self, row, _):
        try:
            set_dead_keys_off(row.get_active())
            self.toast("Saved. It takes effect after you log out and back in.")
        except (subprocess.CalledProcessError, GLib.Error) as e:
            self.toast(f"Could not change the layout: {e}")
        self.update_dead_key_hint()

    def start_recording(self, row):
        self.stop_recording()
        self.recording = row
        row.button.set_label("Press keys…")
        row.button.add_css_class("suggested-action")
        try:
            kga("blockGlobalShortcuts", "(b)", (True,))
        except GLib.Error:
            pass

    def stop_recording(self):
        if self.recording:
            self.recording.refresh()
            self.recording = None
            try:
                kga("blockGlobalShortcuts", "(b)", (False,))
            except GLib.Error:
                pass

    def on_key(self, ctrl, keyval, keycode, state):
        row = self.recording
        if not row or keyval in MODIFIER_KEYVALS:
            return bool(row)
        mods = state & Gtk.accelerator_get_default_mod_mask()
        if keyval == Gdk.KEY_Escape and not mods:
            self.stop_recording()
            return True
        if keyval == Gdk.KEY_BackSpace and not mods:
            self.stop_recording()
            row.apply([])
            return True
        # record the unshifted key so Shift+1 is stored as Shift+1, not Shift+!
        ok, base, *_ = self.get_display().translate_key(keycode, 0, ctrl.get_group())
        codes = to_qt(base if ok and base else keyval, mods)
        self.stop_recording()
        if codes is None:
            self.toast(f"Unsupported key: {Gdk.keyval_name(keyval)}")
        else:
            row.apply(codes)
        return True


class App(Adw.Application):
    def __init__(self):
        super().__init__(application_id="io.github.snip.Settings")

    def do_activate(self):
        (self.props.active_window or Window(self)).present()


if __name__ == "__main__":
    sys.exit(App().run(sys.argv))
