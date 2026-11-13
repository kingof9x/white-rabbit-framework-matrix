# Effects over the panels (led_mon/effects.py) and working out which port something was plugged into
# (led_mon/plug_events.py). No hardware or Windows needed.
import time

import numpy as np
import pytest

from led_mon import effects, plug_events


def frame(seed=1):
    rng = np.random.default_rng(seed)
    g = np.zeros((9, 34), dtype=int)
    g[rng.integers(0, 9, 60), rng.integers(0, 34, 60)] = 160
    return g


def run(manager, side, grid, seconds, step=1 / 30, clock=None):
    out = None
    t0 = time.monotonic()
    while time.monotonic() - t0 < seconds:
        out = manager.apply(side, grid, new_frame=False)
        time.sleep(step)
    return out


def test_settings_are_filled_in_and_clamped():
    s = effects.normalize({"text_speed": 99, "port_rows": {"back": -4}, "learned_ports": {"k": "nowhere", "j": "Left back"},
                           "bogus": 1})
    assert s["text_speed"] == 10
    assert s["port_rows"]["back"] == 0 and s["port_rows"]["front"] == 27
    assert s["learned_ports"] == {"j": "Left back"}
    assert "bogus" not in s
    assert s["plug_animation"] and s["plug_usb"]


@pytest.mark.parametrize("text,expected", [
    ("left-back", ("left", "back")), ("Right Middle", ("right", "middle")), ("right", ("right", None)),
    (None, (None, None)), ("middle", (None, "middle")),
])
def test_positions(text, expected):
    assert effects.parse_position(text) == expected


def test_nothing_changes_without_an_effect():
    m = effects.Effects()
    m.panel_present("left")
    g = frame()
    assert m.apply("left", g) is g
    assert m.interval("left") is None


def test_ripple_moves_the_picture_then_puts_it_back():
    m = effects.Effects()
    m.panel_present("left")
    m.panel_present("right")
    g = frame()
    m.apply("right", g)
    m.plug("right-middle", True)
    assert m.interval("right") == effects.FRAME_SECONDS
    assert m.interval("left") is None          # only the panel by the port
    moved = False
    deadline = time.monotonic() + 4
    while m.players["right"].ripples and time.monotonic() < deadline:
        out = m.apply("right", g, new_frame=False)
        moved = moved or not np.array_equal(out, g)
        time.sleep(1 / 60)
    assert moved
    assert not m.players["right"].ripples
    assert np.array_equal(m.apply("right", g), g)


def test_ripple_starts_at_the_ports_edge():
    # A port on the left starts the ripple at the left panel's left edge: the left columns move first
    m = effects.Effects()
    m.panel_present("left")
    m.panel_present("right")
    ripple, flipped = None, None
    m.apply("left", frame())
    m.plug("left-back", True)
    ripple, flipped = m.players["left"].ripples[0]
    assert flipped is True
    m.plug("right-front", True)
    ripple, flipped = m.players["right"].ripples[0]
    assert flipped is False
    # Back is near the top rows, front near the bottom
    assert np.argmax(ripple.splash.max(axis=1)) // ripple.S > 20


def test_one_panel_gets_both_sides():
    m = effects.Effects()
    m.panel_present("left")
    m.apply("left", frame())
    m.plug("right-back", True)
    assert m.players["left"].ripples


def test_unknown_port_ripples_from_the_middle_of_every_panel():
    m = effects.Effects()
    m.panel_present("left")
    m.panel_present("right")
    m.plug(None, True)
    assert m.players["left"].ripples and m.players["right"].ripples


def test_drain_empties_the_panel_then_it_comes_back():
    m = effects.Effects()
    m.panel_present("right")
    g = frame()
    m.apply("right", g)
    m.plug("right-back", False)
    assert m.players["right"].phase == "drain"
    phases = set()
    deadline = time.monotonic() + 5
    while m.players["right"].phase is not None and time.monotonic() < deadline:
        out = m.apply("right", g, new_frame=False)
        phases.add(m.players["right"].phase)
        time.sleep(1 / 60)
    assert {"drain", "empty", "rise"} <= phases | {"drain"}
    assert m.players["right"].phase is None
    assert np.array_equal(m.apply("right", g), g)


def test_turned_off_plays_nothing():
    m = effects.Effects()
    m.configure({"plug_animation": False})
    m.panel_present("left")
    m.plug("left-back", True)
    assert not m.players["left"].ripples


def test_slide_brings_the_new_app_up_from_the_bottom():
    m = effects.Effects()
    m.panel_present("left")
    old = np.zeros((9, 34), dtype=int)
    old[:, :] = 50
    new = np.zeros((9, 34), dtype=int)
    new[:, :] = 200
    m.apply("left", old)
    m.start_slide("left")
    out = m.apply("left", new)
    assert out[0, 0] == 50                       # still the old app at the top straight away
    time.sleep(0.25)
    mid = m.apply("left", new)
    assert mid[0, 0] == 50 and mid[0, 33] == 200  # the new one coming up from the bottom
    time.sleep(0.3)
    m.apply("left", new)
    assert np.array_equal(m.apply("left", new), new)


def test_slide_can_be_turned_off():
    m = effects.Effects()
    m.configure({"slide": False})
    m.panel_present("left")
    m.apply("left", frame())
    m.start_slide("left")
    assert m.players["left"].slide is None


def test_text_line_spans_both_panels_and_goes_away():
    m = effects.Effects()
    m.configure({"text_speed": 10})
    m.panel_present("left")
    m.panel_present("right")
    m.foreground = 200
    g = np.zeros((9, 34), dtype=int)
    m.say("Hi")
    assert m.interval("left") == effects.FRAME_SECONDS
    seen = {"left": False, "right": False}
    deadline = time.monotonic() + 5
    while m.text.active() and time.monotonic() < deadline:
        for side in ("right", "left"):
            out = m.apply(side, g)
            if out[:, :effects.TEXT_ROWS].any():
                seen[side] = True
            assert not out[:, effects.TEXT_ROWS:].any()
        time.sleep(1 / 60)
    assert seen == {"left": True, "right": True}
    assert not m.text.active()


def test_text_line_off_says_nothing():
    m = effects.Effects()
    m.configure({"text_line": False})
    m.say("Hello")
    assert not m.text.active()


def test_every_letter_has_a_glyph():
    for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 .,:!?-+%'/()":
        assert len(effects.FONT[ch]) == 15
    assert effects.text_width("AB") == 7


# plug_events

def test_instance_id_from_interface_name():
    name = r"\\?\USB#VID_32AC&PID_0010#123ABC#{a5dcbf10-6530-11d2-901f-00c04fb951ed}"
    assert plug_events.instance_id_from_interface(name) == r"USB\VID_32AC&PID_0010\123ABC"
    assert plug_events.is_audio_card(r"USB\VID_32AC&PID_0010\123ABC")
    assert plug_events.is_ignored(r"USB\VID_32AC&PID_0020\X")        # the LED matrix itself
    assert not plug_events.is_ignored(r"USB\VID_046D&PID_C52B\5&1")


def test_location_key_is_the_root_port():
    paths = ["PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)#USB(1)", "ACPI(_SB_)#ACPI(PCI0)"]
    assert plug_events.location_key(paths) == "PCIROOT(0)#PCI(0801)#PCI(0004)#USBROOT(0)#USB(2)"
    assert plug_events.location_key(["ACPI(_SB_)"]) is None


def pld(panel, vertical=0, horizontal=0):
    bits = 2 | (panel << 67) | (vertical << 70) | (horizontal << 72)
    return bits.to_bytes(20, "little")


def test_pld_side():
    assert plug_events.pld_panel(pld(2))[0] == "left"
    assert plug_events.pld_panel(pld(3, 1, 2)) == ("right", {"panel": 3, "vertical": 1, "horizontal": 2})
    assert plug_events.pld_panel(pld(4))[0] is None
    assert plug_events.pld_panel(None) == (None, None)


PD_INFO = """USB-C Port 0 (Right Back):
  Role:          Disconnected
  Charging Type: None
USB-C Port 1 (Right Middle):
  Role:          Disconnected
USB-C Port 2 (Left Middle):
  Role:          Sink
  Charging Type: PD
USB-C Port 3 (Left Back):
  Role:          Source
"""


def test_charger_port_from_framework_tool():
    assert plug_events.parse_pd_info(PD_INFO) == "left-middle"
    assert plug_events.parse_pd_info(PD_INFO.replace("Sink", "Source")) is None
    assert plug_events.charger_port(runner=lambda: PD_INFO) == "left-middle"
    assert plug_events.charger_port(runner=lambda: (_ for _ in ()).throw(OSError("no"))) is None


def test_resolve_position_prefers_what_the_user_picked():
    assert plug_events.resolve_position("K", {"K": "left-front"}, "right") == ("left-front", "learned")
    assert plug_events.resolve_position("K", {}, "right") == ("right", "side")
    assert plug_events.resolve_position(None, {}, None) == (None, "unknown")


def test_watcher_events(monkeypatch):
    calls = []
    learned = {}
    w = plug_events.PlugWatcher(lambda *a: calls.append(a), learned=lambda: learned)
    monkeypatch.setattr(plug_events, "_device_properties", lambda i: {
        "location_paths": ["PCIROOT(0)#USBROOT(0)#USB(3)"], "pld": pld(3), "name": "USB stick"})
    w.device_arrived(r"USB\VID_0781&PID_5581\1")
    assert calls[-1] == ("usb", "right", True, {"name": "USB stick", "key": "PCIROOT(0)#USBROOT(0)#USB(3)", "how": "side"})
    # A second device on the same side straight away (a hub) doesn't ripple again
    w.device_arrived(r"USB\VID_0781&PID_5581\2")
    assert len(calls) == 1
    # Corrected on the Effects tab, then unplugged: the drain uses the corrected port
    learned["PCIROOT(0)#USBROOT(0)#USB(3)"] = "right-front"
    w.device_removed(r"USB\VID_0781&PID_5581\1")
    assert calls[-1][:3] == ("usb", "right-front", False)
    # Moving an input module isn't plugging something in
    w.device_arrived(r"USB\VID_32AC&PID_0020\LED")
    assert calls[-1][:3] == ("usb", "right-front", False)
    # Nothing for a while after waking up
    w.resumed()
    w.last_side_time.clear()
    w.device_arrived(r"USB\VID_0781&PID_5581\3")
    assert calls[-1][:3] == ("usb", "right-front", False)


def test_headphones_are_the_audio_card(monkeypatch):
    calls = []
    w = plug_events.PlugWatcher(lambda *a: calls.append(a))
    monkeypatch.setattr(plug_events, "_device_properties", lambda i: {"location_paths": [], "pld": None, "name": "Audio"})
    w.device_arrived(r"USB\VID_32AC&PID_0010\A")
    assert calls[-1][:3] == ("headphones", None, True)


def test_charger(monkeypatch):
    calls = []
    w = plug_events.PlugWatcher(lambda *a: calls.append(a), learned=lambda: {"charger": "right-back"})
    w.ac_online = False
    monkeypatch.setattr(plug_events, "charger_port", lambda: None)
    w._charger_in()
    assert calls[-1][:3] == ("charger", "right-back", True)
    monkeypatch.setattr(plug_events, "charger_port", lambda: "left-middle")
    w._charger_in()
    assert calls[-1][:3] == ("charger", "left-middle", True)
    monkeypatch.setattr(plug_events, "_ac_online", lambda: False)
    w.ac_online = True
    w.power_changed()
    assert calls[-1][:3] == ("charger", "left-middle", False)


def test_monitor_plays_the_wanted_kinds(monkeypatch):
    from led_mon import led_system_monitor as mon
    played = []
    monkeypatch.setattr(mon.effects.manager, "plug", lambda pos, inn: played.append((pos, inn)))
    said = []
    monkeypatch.setattr(mon.effects.manager, "say", lambda t: said.append(t))
    mon.effects.manager.configure({"plug_usb": False})
    mon.on_plug("usb", "left-back", True, {"name": "Mouse"})
    assert played == [] and mon.last_plug["name"] == "Mouse"
    mon.on_plug("charger", "right-back", True, {"name": "Charger"})
    assert played == [("right-back", True)] and said == ["Charging"]
    mon.effects.manager.configure({})
