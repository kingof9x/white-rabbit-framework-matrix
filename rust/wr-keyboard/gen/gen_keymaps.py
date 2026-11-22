#!/usr/bin/env python3
"""Writes app/core/src/keymap_data.rs from the QMK sources.

The keyboard firmware lives in kingof9x/framework-16-keyboard-customization. Run firmware/build.sh
there first (it fetches Framework's QMK into firmware/.qmk_firmware), then, from White Rabbit's
repository root:

    python3 rust/wr-keyboard/gen/gen_keymaps.py path/to/framework-16-keyboard-customization

It reads the keycode numbers (quantum/keycodes.h), the matrix position of every
key (the LAYOUT macros) and the default keymaps (firmware/keymaps/*/f16custom).
"""
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path("../framework-16-keyboard-customization").resolve()
QMK = ROOT / "firmware" / ".qmk_firmware"
OUT = Path(__file__).resolve().parents[1] / "src" / "keymap_data.rs"

# ---------------------------------------------------------------- keycodes
codes = {}
text = (QMK / "quantum" / "keycodes.h").read_text()
for name, val in re.findall(r"^\s+(\w+)\s*=\s*(0x[0-9A-Fa-f]+|\w+),", text, re.M):
    if val.startswith("0x"):
        codes[name] = int(val, 16)
    elif val in codes:
        codes[name] = codes[val]
# keyboard (framework.h) and keymap (macropad) custom keycodes start at SAFE_RANGE = QK_USER
user = codes["QK_USER"]
for i, n in enumerate(["KC_SCRN", "FN_LOCK", "KC_CPLT", "PAD_SWAP", "PAD_LIGHT"]):
    codes[n] = user + i
codes["_______"] = codes["KC_TRANSPARENT"]
MODS = {"LCTL": 0x0100, "LSFT": 0x0200, "LALT": 0x0400, "LGUI": 0x0800}
LAYERS = {"_FN": 1, "_FN_LOCK": 2, "_FM": 3}


def kc(expr):
    expr = expr.strip()
    m = re.fullmatch(r"(\w+)\((.*)\)", expr)
    if m:
        f, arg = m.groups()
        if f == "MO":
            return codes["QK_MOMENTARY"] | LAYERS[arg]
        return MODS[f] | kc(arg)
    if expr in codes:
        return codes[expr]
    sys.exit(f"unknown keycode {expr}")


# ---------------------------------------------------------------- matrix
def matrix(header):
    t = header.read_text()
    m = re.search(r"#define LAYOUT\(\s*\\(.*?)\)\s*\\\s*\{(.*?)\n\}", t, re.S)
    args = re.findall(r"\b([KH]\d+)\b", m.group(1))
    pos = {}
    for r, row in enumerate(re.findall(r"\{([^{}]*)\}", m.group(2))):
        for c, cell in enumerate(x.strip().rstrip("\\").strip() for x in row.split(",")):
            if cell != "KC_NO":
                pos[cell] = (r, c)
    return [pos[a] for a in args]


def keymap_layers(path):
    t = path.read_text()
    t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)
    t = re.sub(r"//[^\n]*", "", t)
    body = t[t.index("keymaps[]"):]
    layers = []
    for m in re.finditer(r"\[\s*\w+\s*\]\s*=\s*LAYOUT\(", body):
        depth, i = 1, m.end()
        while depth:
            depth += {"(": 1, ")": -1}.get(body[i], 0)
            i += 1
        args, depth, cur = [], 0, ""
        for ch in body[m.end():i - 1]:
            if ch == "," and depth == 0:
                args.append(cur)
                cur = ""
                continue
            depth += {"(": 1, ")": -1}.get(ch, 0)
            cur += ch
        args.append(cur)
        layers.append([kc(a) for a in args if a.strip()])
    return layers


kb_matrix = matrix(QMK / "keyboards/framework/ansi/ansi.h")
pad_matrix = matrix(QMK / "keyboards/framework/macropad/macropad.h")
kb_layers = keymap_layers(ROOT / "firmware/keymaps/ansi/f16custom/keymap.c")
pad_layers = keymap_layers(ROOT / "firmware/keymaps/macropad/f16custom/keymap.c")
assert len(kb_matrix) == 78 and all(len(l) == 78 for l in kb_layers) and len(kb_layers) == 4
assert len(pad_matrix) == 24 and len(pad_layers) == 4

# The app's keyboard_keys() lists the arrow cluster as Left, Up, Right, Down;
# the LAYOUT macro has Left, Up, Down, Right.
APP_ORDER = list(range(76)) + [77, 76]


def arr(xs, fmt):
    return "[" + ", ".join(fmt(x) for x in xs) + "]"


# ---------------------------------------------------------------- key names
NAMED = [
    # group, QMK name, label in lists, short legend
    *[("Letters", f"KC_{c}", c, c) for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ"],
    *[("Numbers", f"KC_{n}", n, n) for n in "1234567890"],
    *[("F-keys", f"KC_F{n}", f"F{n}", f"F{n}") for n in range(1, 25)],
    ("Symbols", "KC_MINS", "- and _", "-"), ("Symbols", "KC_EQL", "= and +", "="),
    ("Symbols", "KC_LBRC", "[ and {", "["), ("Symbols", "KC_RBRC", "] and }", "]"),
    ("Symbols", "KC_BSLS", "\\\\ and |", "\\\\"), ("Symbols", "KC_SCLN", "; and :", ";"),
    ("Symbols", "KC_QUOT", "' and \\\"", "'"), ("Symbols", "KC_GRV", "` and ~", "`"),
    ("Symbols", "KC_COMM", ", and <", ","), ("Symbols", "KC_DOT", ". and >", "."),
    ("Symbols", "KC_SLSH", "/ and ?", "/"),
    ("Numpad", "KC_NUM", "Num Lock", "Num"),
    *[("Numpad", f"KC_P{n}", f"Numpad {n}", n) for n in "0123456789"],
    ("Numpad", "KC_PSLS", "Numpad /", "/"), ("Numpad", "KC_PAST", "Numpad *", "*"),
    ("Numpad", "KC_PMNS", "Numpad -", "-"), ("Numpad", "KC_PPLS", "Numpad +", "+"),
    ("Numpad", "KC_PDOT", "Numpad .", "."), ("Numpad", "KC_PENT", "Numpad Enter", "Enter"),
    ("Editing", "KC_ENT", "Enter", "Enter"), ("Editing", "KC_ESC", "Esc", "Esc"),
    ("Editing", "KC_BSPC", "Backspace", "Bksp"), ("Editing", "KC_TAB", "Tab", "Tab"),
    ("Editing", "KC_SPC", "Space", "Space"), ("Editing", "KC_DEL", "Delete", "Del"),
    ("Editing", "KC_INS", "Insert", "Ins"), ("Editing", "KC_CAPS", "Caps Lock", "Caps"),
    ("Navigation", "KC_LEFT", "Left arrow", "←"), ("Navigation", "KC_RGHT", "Right arrow", "→"),
    ("Navigation", "KC_UP", "Up arrow", "↑"), ("Navigation", "KC_DOWN", "Down arrow", "↓"),
    ("Navigation", "KC_HOME", "Home", "Home"), ("Navigation", "KC_END", "End", "End"),
    ("Navigation", "KC_PGUP", "Page Up", "PgUp"), ("Navigation", "KC_PGDN", "Page Down", "PgDn"),
    ("System", "KC_PSCR", "Print Screen", "PrtSc"), ("System", "KC_SCRL", "Scroll Lock", "ScrLk"),
    ("System", "KC_PAUS", "Pause / Break", "Pause"), ("System", "KC_APP", "Menu key", "Menu"),
    ("Modifiers", "KC_LCTL", "Ctrl (left)", "Ctrl"), ("Modifiers", "KC_LSFT", "Shift (left)", "Shift"),
    ("Modifiers", "KC_LALT", "Alt (left)", "Alt"), ("Modifiers", "KC_LGUI", "Super (left)", "Super"),
    ("Modifiers", "KC_RCTL", "Ctrl (right)", "Ctrl"), ("Modifiers", "KC_RSFT", "Shift (right)", "Shift"),
    ("Modifiers", "KC_RALT", "Alt (right)", "Alt"), ("Modifiers", "KC_RGUI", "Super (right)", "Super"),
    ("Media", "KC_MUTE", "Mute", "Mute"), ("Media", "KC_VOLD", "Volume down", "Vol-"),
    ("Media", "KC_VOLU", "Volume up", "Vol+"), ("Media", "KC_MPRV", "Previous track", "Prev"),
    ("Media", "KC_MPLY", "Play / pause", "Play"), ("Media", "KC_MNXT", "Next track", "Next"),
    ("Media", "KC_MSTP", "Stop", "Stop"), ("Media", "KC_MSEL", "Framework key (Framework's website)", "Frmwk"),
    ("Screen", "KC_BRID", "Screen brightness down", "Bri-"), ("Screen", "KC_BRIU", "Screen brightness up", "Bri+"),
    ("Screen", "KC_SCRN", "Display mode (Super+P)", "Display"), ("Screen", "KC_AIRP", "Airplane mode", "Airplane"),
    ("Windows apps", "KC_CALC", "Calculator", "Calc"), ("Windows apps", "KC_MYCM", "File Explorer", "Files"),
    ("Windows apps", "KC_MAIL", "Mail", "Mail"), ("Windows apps", "KC_WHOM", "Web browser", "Browser"),
    ("Windows apps", "KC_WSCH", "Web search", "Search"),
    ("Lighting", "RGB_TOG", "Lighting on/off", "Light"), ("Lighting", "RGB_MOD", "Next effect", "Effect+"),
    ("Lighting", "RGB_RMOD", "Previous effect", "Effect-"), ("Lighting", "RGB_HUI", "Hue up", "Hue+"),
    ("Lighting", "RGB_HUD", "Hue down", "Hue-"), ("Lighting", "RGB_SAI", "Saturation up", "Sat+"),
    ("Lighting", "RGB_SAD", "Saturation down", "Sat-"), ("Lighting", "RGB_VAI", "Brightness up", "Bright+"),
    ("Lighting", "RGB_VAD", "Brightness down", "Bright-"), ("Lighting", "RGB_SPI", "Speed up", "Speed+"),
    ("Lighting", "RGB_SPD", "Speed down", "Speed-"), ("Lighting", "BL_STEP", "Backlight step", "Backlight"),
    ("Lighting", "BL_BRTG", "Backlight breathing", "Breathe"),
    ("Fn", "FN_LOCK", "Fn lock", "Fn lock"),
]
named_rows = "\n".join(
    f'    Named {{ group: "{g}", code: 0x{kc(n):04X}, label: "{l}", short: "{s}" }},' for g, n, l, s in NAMED
)

OUT.write_text(f"""// @generated by rust/wr-keyboard/gen/gen_keymaps.py from Framework's QMK v0.3.1 and the
// f16custom keymaps. Do not edit by hand.

/// A key the app offers in its lists.
pub struct Named {{
    pub group: &'static str,
    pub code: u16,
    pub label: &'static str,
    pub short: &'static str,
}}

pub const NAMED: &[Named] = &[
{named_rows}
];

pub const KC_NO: u16 = 0x0000;
pub const KC_TRNS: u16 = 0x{codes['KC_TRANSPARENT']:04X};
pub const KC_F13: u16 = 0x{codes['KC_F13']:04X};
pub const KC_F24: u16 = 0x{codes['KC_F24']:04X};
pub const QK_MODS: u16 = 0x{codes['QK_MODS']:04X};
pub const QK_MODS_MAX: u16 = 0x{codes['QK_MODS_MAX']:04X};
pub const QK_MOMENTARY: u16 = 0x{codes['QK_MOMENTARY']:04X};
pub const QK_LIGHTING: u16 = 0x{codes['QK_LIGHTING']:04X};
pub const QK_LIGHTING_MAX: u16 = 0x{codes['QK_LIGHTING_MAX']:04X};
pub const MIDI_NOTE_FIRST: u16 = 0x{codes['QK_MIDI_NOTE_C_0']:04X};
pub const MIDI_NOTE_LAST: u16 = 0x{codes['QK_MIDI_NOTE_B_5']:04X};

/// Keyboard: (matrix row, column) of each key, in `keys::keyboard_keys()` order.
pub const KEYBOARD_MATRIX: [(u8, u8); 78] = {arr([kb_matrix[i] for i in APP_ORDER], lambda p: f"({p[0]}, {p[1]})")};
/// Keyboard matrix size (rows, columns).
pub const KEYBOARD_MATRIX_SIZE: (u8, u8) = (8, 16);
/// Keyboard default keymap, layers 0..=3 (base, Fn, Fn lock, Fn while locked), in app order.
pub const KEYBOARD_DEFAULT: [[u16; 78]; 4] = [
{chr(10).join("    " + arr([l[i] for i in APP_ORDER], lambda c: f"0x{c:04X}") + "," for l in kb_layers)}
];

/// Macropad: (matrix row, column) of each key, top left to bottom right.
pub const MACROPAD_MATRIX: [(u8, u8); 24] = {arr(pad_matrix, lambda p: f"({p[0]}, {p[1]})")};
pub const MACROPAD_MATRIX_SIZE: (u8, u8) = (4, 8);
/// Macropad default keymap, layers A (MIDI) and B (numpad).
pub const MACROPAD_DEFAULT: [[u16; 24]; 2] = [
{chr(10).join("    " + arr(l, lambda c: f"0x{c:04X}") + "," for l in pad_layers[:2])}
];
""")
print("wrote", OUT)
