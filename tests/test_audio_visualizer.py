# The audio visualizer: its looks, settings, sound input sharing, music mode and the tray item
import importlib
import sys

import pytest

np = pytest.importorskip("numpy")

from led_mon.equalizer_files import looks, audio
from led_mon.editor import catalog
from led_mon import app_settings, shortcuts

RATE = looks.SAMPLE_RATE


def tone(freq, amplitude=0.5, n=looks.FFT_SIZE, stereo=True):
    t = np.arange(n) / RATE
    x = amplitude * np.sin(2 * np.pi * freq * t)
    return np.stack([x, x], axis=1) if stereo else x


def run(visualizer, samples, frames=20):
    grid = None
    for _ in range(frames):
        grid = visualizer.frame(samples, 0.03)
    return grid


def test_settings_are_filled_in_and_made_valid():
    s = looks.normalize({"style": "nonsense", "sensitivity": "99", "span-panels": "true", "fall-speed": "FAST",
                         "input-mode": "mic", "input-device": "  USB Mic  ", "level-gain": 2})
    assert s["style"] == "classic" and s["sensitivity"] == 10 and s["span-panels"] is True
    assert s["fall-speed"] == "fast" and s["input-mode"] == "microphone" and s["input-device"] == "USB Mic"
    assert s["level-gain"] == 2  # older equalizer settings still count
    assert looks.normalize(None) == looks.DEFAULTS


@pytest.mark.parametrize("style", looks.STYLE_KEYS)
@pytest.mark.parametrize("span", [False, True])
def test_every_look_draws_a_panel_and_goes_dark_in_silence(style, span):
    for side in ("left", "right"):
        options = {"style": style, "span-panels": span, "peak-dots": True, "shaded": True, "flip": True}
        music = run(looks.Visualizer(options, side, seed=1), looks.demo_samples(1.0))
        assert music.shape == (9, 34) and music.min() >= 0.0 and music.max() <= 1.0
        assert music.max() > 0, "music should light something"
        quiet = run(looks.Visualizer(options, side, seed=1), np.zeros((looks.FFT_SIZE, 2)), frames=60)
        if style in ("wave", "mirror-wave", "double-wave", "radial"):
            # A flat line (or the circle in the middle) stays, like an oscilloscope's
            assert (quiet > 0).sum() <= 110
        elif style != "rings":
            assert quiet.max() <= 0.3, "silence should leave the panel (nearly) dark"


def test_every_look_has_a_label_and_description():
    for key in looks.STYLE_KEYS:
        assert looks.STYLE_LABELS[key] and looks.STYLE_DESCRIPTIONS[key]
        assert hasattr(looks.Visualizer, "_draw_" + key.replace("-", "_"))


def test_a_note_lights_the_right_band():
    analyzer = looks.Analyzer(9, auto=False)
    levels = analyzer.levels(tone(1000), 0.03)
    # Band 5 is centred on 1 kHz
    assert int(np.argmax(levels)) == 5
    assert levels[0] == 0 and levels[8] == 0


def test_18_bars_across_both_panels_put_bass_on_the_left():
    assert looks.column_bands("bars", True, "left") == (18, list(range(9)))
    assert looks.column_bands("bars", True, "right") == (18, list(range(9, 18)))
    bass = tone(40)
    left = run(looks.Visualizer({"style": "bars", "span-panels": True}, "left"), bass)
    right = run(looks.Visualizer({"style": "bars", "span-panels": True}, "right"), bass)
    assert left.sum() > 0 and right.sum() == 0
    treble = tone(7000)
    left = run(looks.Visualizer({"style": "bars", "span-panels": True}, "left"), treble)
    right = run(looks.Visualizer({"style": "bars", "span-panels": True}, "right"), treble)
    assert right.sum() > 0 and left.sum() == 0


def test_centre_out_mirrors_the_bands():
    n, columns = looks.column_bands("centre", False, "left")
    assert n == 5 and columns == [4, 3, 2, 1, 0, 1, 2, 3, 4]
    n, columns = looks.column_bands("centre", True, "left")
    assert n == 9 and columns[-1] == 0 and looks.column_bands("centre", True, "right")[1][0] == 0


def test_upside_down_flips_the_bars():
    normal = run(looks.Visualizer({"style": "bars"}, "left"), tone(1000))
    flipped = run(looks.Visualizer({"style": "bars", "flip": True}, "left"), tone(1000))
    assert normal[:, 33].sum() > 0 and normal[:, 0].sum() == 0
    assert np.array_equal(flipped, normal[:, ::-1])


@pytest.mark.parametrize("span", [False, True])
def test_radial_bars_on_the_left_panel_mirror_the_right(span):
    sound = tone(60) + tone(4000, amplitude=0.2)
    left = run(looks.Visualizer({"style": "radial", "span-panels": span}, "left"), sound)
    right = run(looks.Visualizer({"style": "radial", "span-panels": span}, "right"), sound)
    # Not the same either way round, so the mirroring shows
    assert not np.array_equal(right, right[::-1, :])
    if not span:
        assert np.array_equal(left, right[::-1, :])
    else:
        assert np.array_equal(left, run(looks.Visualizer({"style": "radial", "span-panels": True}, "left"),
                                        sound)) and not np.array_equal(left, right)


def test_bars_fall_at_the_chosen_speed():
    def after_silence(speed):
        v = looks.Visualizer({"style": "bars", "fall-speed": speed, "auto-sensitivity": False}, "left")
        run(v, tone(1000))
        v.frame(np.zeros((looks.FFT_SIZE, 2)), 0.1)
        return v.shown.max()
    assert after_silence("slow") > after_silence("normal") > after_silence("fast")


def test_automatic_sensitivity_turns_up_quiet_music():
    quiet_music = tone(1000, amplitude=0.01)
    fixed = looks.Analyzer(9, "playback", auto=False)
    auto = looks.Analyzer(9, "playback", auto=True)
    for _ in range(200):
        auto.levels(quiet_music, 0.03)
    assert auto.levels(quiet_music, 0.03).max() > fixed.levels(quiet_music, 0.03).max()


def microphone(noise, music, frames, rng, start=0.0):
    for i in range(frames):
        x = rng.normal(0, noise, (looks.FFT_SIZE, 2))
        if music:
            x = x + looks.demo_samples(start + i * 0.03) * music
        yield x


@pytest.mark.parametrize("noise, music", [(0.00003, 0.0003), (0.0003, 0.002), (0.003, 0.02)])
def test_microphone_shows_music_above_the_room_noise_at_any_volume(noise, music):
    rng = np.random.default_rng(0)
    v = looks.Visualizer({"style": "bars", "input-mode": "microphone"}, "left")
    quiet = []
    for x in microphone(noise, 0, 300, rng):
        v.frame(x, 0.03)
        quiet.append(v.is_silent())
    # The room's hiss alone counts as silence nearly all the time
    assert sum(quiet[-100:]) >= 90
    heard = []
    for x in microphone(noise, music, 150, rng, start=9.0):
        v.frame(x, 0.03)
        heard.append(v.loudness)
    assert np.mean(heard) > 3 * v.silence_sum


# (0.00003, 0.0003): a Framework 16's built-in microphone array peaks at about 0.0003
@pytest.mark.parametrize("noise, music", [(0.00003, 0.0003), (0.0003, 0.002), (0.003, 0.02)])
@pytest.mark.parametrize("style", ["wave", "mirror-wave", "double-wave"])
def test_waveforms_move_with_a_microphone_at_any_volume(style, noise, music):
    rng = np.random.default_rng(0)
    v = looks.Visualizer({"style": style, "input-mode": "microphone"}, "left")
    for x in microphone(noise, 0, 300, rng):
        flat = v.frame(x, 0.03)
    widths = []
    for x in microphone(noise, music, 150, rng, start=9.0):
        grid = v.frame(x, 0.03)
        # How many of the 9 columns the wave reaches
        widths.append(int((grid.max(axis=1) > 0.4).sum()))
    # The room's hiss alone: one still line. Music: the wave swings across the panel
    assert int((flat.max(axis=1) > 0.4).sum()) <= 2
    assert np.mean(widths[-100:]) >= 4


def test_sensitivity_makes_bars_taller():
    low = looks.Analyzer(9, sensitivity=1, auto=False).levels(tone(1000, 0.01), 0.03).max()
    high = looks.Analyzer(9, sensitivity=10, auto=False).levels(tone(1000, 0.01), 0.03).max()
    assert high > low


def test_led_values_keep_faint_leds_visible():
    leds = looks.to_leds(np.array([[0.0, 0.01, 0.05, 1.0]]), 100)
    assert leds.tolist() == [[0, 0, 5, 100]]
    assert looks.to_leds(np.array([[0.03]]), 10).tolist() == [[1]]


def test_microphone_names_match_exactly_then_by_part():
    names = ["Microphone Array (Realtek(R) Audio)", "Headset Microphone (USB Audio)"]
    assert audio.match_name("Headset Microphone (USB Audio)", names) == names[1]
    assert audio.match_name("usb", names) == names[1]
    assert audio.match_name("Webcam", names) is None
    assert audio.match_name("", names) is None


def test_capture_keeps_the_latest_sound_and_reports_silence_when_it_stops(monkeypatch):
    capture = audio.Capture()
    capture.push(np.ones(100) * 0.5)  # mono is copied to both channels
    snap = capture.snapshot()
    assert snap.shape == (looks.FFT_SIZE, 2)
    assert (snap[-100:] == 0.5).all() and (snap[:-100] == 0).all()
    capture.updated -= audio.STALE_AUDIO_SEC + 1
    assert not capture.snapshot().any()


def test_panels_and_preview_share_one_recording(monkeypatch):
    started, stopped = [], []
    monkeypatch.setattr(audio.Capture, "start", lambda self: started.append(self))
    monkeypatch.setattr(audio.Capture, "stop", lambda self: stopped.append(self))
    a = audio.acquire("playback")
    b = audio.acquire("playback", "ignored for playback")
    c = audio.acquire("microphone", "USB")
    assert a is b and a is not c and len(started) == 2
    audio.release(a)
    assert not stopped
    audio.release(b)
    audio.release(c)
    assert stopped == [a, c]
    assert audio.acquire("playback") is not a
    audio.release(audio.acquire("playback"))


def test_layout_editor_knows_the_audio_visualizer(monkeypatch):
    info = catalog.get("equalizer")
    assert info.label == "Audio visualizer"
    keys = [s.key for s in info.settings]
    for key in ("input-mode", "input-device", "style", "span-panels", "peak-dots", "shaded", "flip",
                "fall-speed", "sensitivity", "auto-sensitivity"):
        assert key in keys
    for setting in info.settings:
        if setting.labels:
            assert len(setting.labels) == len(setting.options)
    mic = next(s for s in info.settings if s.key == "input-device")
    assert mic.kind == "microphone"
    monkeypatch.setattr(audio, "list_microphones", lambda: ["USB Mic"])
    assert catalog.microphone_options() == [("", catalog.DEFAULT_MICROPHONE), ("USB Mic", "USB Mic")]
    entry = catalog.new_app("equalizer", "right")
    assert entry["args"]["side"] == "right" and entry["args"]["style"] == "classic"
    assert "input-device" not in entry["args"]  # blank: the default microphone


def test_music_settings_are_saved_with_the_other_settings(tmp_path):
    path = str(tmp_path / "settings.json")
    assert app_settings.load_music(path) == looks.DEFAULTS
    assert app_settings.save_music({"style": "radial", "span-panels": True}, path)
    assert app_settings.load_music(path)["style"] == "radial"
    assert app_settings.load_music(path)["span-panels"] is True
    assert app_settings.load_settings(path)["brightness_level"] == 70


def test_music_mode_has_a_shortcut():
    assert "m" in shortcuts.LETTERS


def test_music_mode_control_command():
    pytest.importorskip("psutil")
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    monitor.set_music_mode(False)
    assert monitor.handle_music_request({"action": "status"}) == {"music_mode": False}
    assert monitor.handle_music_request({"action": "toggle"}) == {"music_mode": True}
    monitor.music_restart.clear()
    assert monitor.handle_music_request({"action": "reload"}) == {"music_mode": True}
    assert monitor.music_restart.is_set()
    assert monitor.handle_music_request({"action": "off"}) == {"music_mode": False}
    monitor.music_restart.clear()


def test_tray_music_mode_checkbox(monkeypatch):
    pytest.importorskip("PIL")
    monkeypatch.setenv("PYSTRAY_BACKEND", "dummy")
    sys.modules.pop("pystray", None)
    try:
        pystray = importlib.import_module("pystray")
    except ImportError:
        pytest.skip("pystray is not installed")
    from led_mon import tray
    state = {"on": False}
    noop = lambda icon, item: None
    actions = {name: noop for name in (
        "edit_layout", "brightness", "toggle_animation", "identify", "next_app", "toggle_freeze", "open_lhm",
        "open_fc", "open_folder", "open_log", "reload_config", "toggle_startup", "quit")}

    def toggle(icon, item):
        state["on"] = not state["on"]
    actions["toggle_music"] = toggle
    menu = tray.build_menu(pystray, actions, tray.AppFinder(), lambda: False, lambda: False, True,
                           startup_supported=True, is_music_mode=lambda: state["on"])
    item = [i for i in menu.items if str(i.text) == "Music mode"][0]
    assert item.visible and not item.checked
    item(None)
    assert state["on"] and item.checked
    # Without music mode support the item is hidden
    menu = tray.build_menu(pystray, {k: v for k, v in actions.items() if k != "toggle_music"}, tray.AppFinder(),
                           lambda: False, lambda: False, True, startup_supported=True)
    assert not [i for i in menu.items if str(i.text) == "Music mode" and i.visible]


def test_music_tab_says_what_music_mode_hears():
    pytest.importorskip("tkinter")
    from white_rabbit.music import hearing_text
    assert hearing_text(None) == "Music mode is starting..."
    text = hearing_text({"left": {"device": "USB Mic", "silent": True},
                         "right": {"device": "USB Mic", "silent": False, "problem": None}})
    assert "left panel: too quiet (USB Mic)" in text and "right panel: hearing sound (USB Mic)" in text
    text = hearing_text({"left": {"device": "Microphone Array", "problem": "USB Mic wasn't found"}})
    assert "USB Mic wasn't found" in text


def test_the_visualizer_app_reports_what_it_hears():
    try:
        importlib.import_module("led_mon.drawing")  # loads the plugins
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    plugin = sys.modules.get("equalizer")
    if plugin is None:
        pytest.skip("plugins not loaded")

    class Fake:
        hearing = {"device": "USB Mic", "loudness": 12, "silent": False}
    plugin.equalizers["left"] = Fake()
    try:
        assert plugin.hearing() == {"left": {"device": "USB Mic", "loudness": 12, "silent": False}}
    finally:
        plugin.equalizers.pop("left", None)


def test_music_mode_takes_back_panels_lent_to_the_window():
    pytest.importorskip("psutil")
    try:
        from led_mon import led_system_monitor as monitor
    except Exception as e:
        pytest.skip(f"full dependencies not installed: {e}")
    from led_mon import serial_ports
    serial_ports.hold(["COM9"], owner=1234)
    try:
        monitor.set_music_mode(True)
        assert not serial_ports.is_held("COM9")
    finally:
        monitor.set_music_mode(False)
        serial_ports.release(["COM9"])


def test_soundcard_is_imported_when_the_app_loads_on_windows():
    # Imported inside a recording thread, soundcard fails with "Error 0x100000001"
    from led_mon.platform_utils import IS_WINDOWS
    if not IS_WINDOWS:
        pytest.skip("Windows audio")
    pytest.importorskip("soundcard")
    assert audio.sc is not None, audio._soundcard_error


def test_recording_thread_can_use_soundcard_on_windows():
    from led_mon.platform_utils import IS_WINDOWS
    if not IS_WINDOWS:
        pytest.skip("Windows audio")
    pytest.importorskip("soundcard")
    import time
    capture = audio.Capture("playback")
    capture.start()
    try:
        time.sleep(3)
        # The CI machine may have no sound device, but the COM error from a late import must be gone
        assert "0x100000001" not in str(capture.problem or "")
    finally:
        capture.stop()
