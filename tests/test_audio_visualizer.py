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
        if style in ("wave", "mirror-wave", "double-wave", "radial", "radial-fan", "radial-facing",
                     "heartbeat", "dancing-rabbit"):
            # A flat line (or the circle in the middle, or the rabbit) stays, like an oscilloscope's
            assert (quiet > 0).sum() <= 110
        elif style != "rings":
            assert quiet.max() <= 0.3, "silence should leave the panel (nearly) dark"


def test_every_look_has_a_label_and_description():
    for key in looks.STYLE_KEYS:
        assert looks.STYLE_LABELS[key] and looks.STYLE_DESCRIPTIONS[key]
        assert hasattr(looks.Visualizer, "_draw_" + key.replace("-", "_"))


def beats(seconds=4.0, frame=0.03):
    """Bass drum hits twice a second, as one slice of sound per frame."""
    out = []
    for i in range(int(seconds / frame)):
        t = i * frame
        loud = 0.6 if (t % 0.5) < 0.12 else 0.0
        out.append(tone(50, amplitude=loud) if loud else np.zeros((looks.FFT_SIZE, 2)))
    return out


def test_the_dancing_rabbit_jumps_on_the_beat():
    v = looks.Visualizer({"style": "dancing-rabbit"}, "right", seed=1)
    tops = []
    for samples in beats():
        grid = v.frame(samples, 0.03)
        lit = np.nonzero(grid[:, :30].max(axis=0) >= 1.0)[0]
        tops.append(int(lit.min()) if len(lit) else 99)
    assert max(tops) - min(tops) >= 5, "the rabbit should jump up and land again"


def test_the_beat_tunnel_sends_out_a_ring_per_beat():
    v = looks.Visualizer({"style": "beat-tunnel"}, "right", seed=1)
    for samples in beats(seconds=1.0):
        v.frame(samples, 0.03)
    assert 1 <= len(v.fx["rings"]) <= 3


@pytest.mark.parametrize("style", ["bonfire", "beat-tunnel", "fountain", "lava-lamp", "dancing-rabbit"])
def test_new_looks_mirror_across_the_keyboard(style):
    # The same sound in both channels and the same random numbers: the left is the right's mirror image
    sound = tone(60) + tone(2000, amplitude=0.2)
    left = run(looks.Visualizer({"style": style}, "left", seed=3), sound)
    right = run(looks.Visualizer({"style": style}, "right", seed=3), sound)
    assert right.max() > 0 and np.array_equal(left, right[::-1, :])


def test_bonfire_is_taller_when_louder():
    quiet = run(looks.Visualizer({"style": "bonfire", "auto-sensitivity": False}, "right", seed=1), tone(60, 0.003), 40)
    loud = run(looks.Visualizer({"style": "bonfire", "auto-sensitivity": False}, "right", seed=1), tone(60, 0.5), 40)
    assert (loud > 0.1).sum() > (quiet > 0.1).sum()


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


@pytest.mark.parametrize("style", ["radial-fan", "radial-facing"])
@pytest.mark.parametrize("span", [False, True])
def test_radial_fan_and_facing_mirror_across_the_keyboard(style, span):
    sound = tone(60) + tone(4000, amplitude=0.2)
    left = run(looks.Visualizer({"style": style, "span-panels": span}, "left"), sound)
    right = run(looks.Visualizer({"style": style, "span-panels": span}, "right"), sound)
    assert right.max() > 0
    # Lopsided on its own, so the mirror image is easy to see
    assert not np.array_equal(right, right[::-1, :])
    assert np.array_equal(left, right[::-1, :])


def test_radial_fan_starts_at_the_keyboard_edge():
    right = run(looks.Visualizer({"style": "radial-fan"}, "right"), tone(60) + tone(4000, amplitude=0.3))
    # The right panel's keyboard edge is its first column
    assert (right[:3] > 0.5).sum() > (right[6:] > 0.5).sum()


def test_radial_facing_points_bass_at_the_keyboard():
    def lit(sound):
        right = run(looks.Visualizer({"style": "radial-facing"}, "right"), sound)
        return (right[:4] > 0.5).sum(), (right[5:] > 0.5).sum()
    towards, outwards = lit(tone(60))
    assert towards > outwards
    towards, outwards = lit(tone(8000))
    assert outwards > towards


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


def steady_music(t, rng, amplitude=0.003, noise=0.00005):
    """Music with a beat every half second, heard by a microphone in a quiet room."""
    n = np.arange(looks.FFT_SIZE) / RATE + t
    beat = 1 + 3 * np.exp(-(t % 0.5) / 0.08)
    x = amplitude * beat * (np.sin(2 * np.pi * 110 * n) + 0.5 * np.sin(2 * np.pi * 440 * n)
                            + 0.3 * np.sin(2 * np.pi * 2000 * n))
    x = x + noise * rng.standard_normal(looks.FFT_SIZE)
    return np.stack([x, x], axis=1)


def test_microphone_music_comes_back_after_the_microphone_gave_nothing():
    # A few seconds with no sound at all from the microphone (muted, taken by another app, or a stalled
    # recording) used to drag the room's noise down to nothing. When the music came back, its level
    # set the bars' full height so high that the music then counted as silence for about a minute,
    # and the panel showed the silence animation instead of the music until music mode was turned off and on.
    rng = np.random.default_rng(0)
    v = looks.Visualizer({"style": "bars", "input-mode": "microphone"}, "right")
    t = 0.0
    for _ in range(170):  # the room
        v.frame(steady_music(t, rng, amplitude=0.0), 0.03)
        t += 0.03
    for _ in range(330):  # music
        v.frame(steady_music(t, rng), 0.03)
        t += 0.03
    for _ in range(170):  # about 5 seconds of nothing at all
        v.frame(np.zeros((looks.FFT_SIZE, 2)), 0.03)
        assert v.is_silent()
        t += 0.03
    silent_run = longest = 0
    for _ in range(2000):  # a minute of music
        v.frame(steady_music(t, rng), 0.03)
        t += 0.03
        silent_run = silent_run + 1 if v.is_silent() else 0
        longest = max(longest, silent_run)
    # Never silent for long enough to switch to the idle look (4 seconds)
    assert longest * 0.03 < 2.0


def test_microphone_bars_full_height_has_a_limit():
    rng = np.random.default_rng(0)
    v = looks.Visualizer({"style": "bars", "input-mode": "microphone"}, "left")
    for x in microphone(0.00003, 0, 150, rng):
        v.frame(x, 0.03)
    v.frame(tone(1000, 0.5), 0.03)  # far louder than anything a laptop microphone hears
    assert v.analyzer.peak_above <= looks.MIC_MAX_SPAN_DB


def test_stalled_microphone_recording_is_noticed():
    capture = audio.Capture("microphone")
    assert not capture.stalled(now=1000.0)  # not recording yet
    capture.current_id = "mic"
    capture.recording_since = 1000.0
    assert not capture.stalled(now=1000.0 + audio.MIC_STALL_SEC - 1)  # just started
    assert capture.stalled(now=1000.0 + audio.MIC_STALL_SEC + 1)
    capture.updated = 1004.0  # sound arrived
    assert not capture.stalled(now=1004.0 + audio.MIC_STALL_SEC - 1)
    # Playback gives nothing whenever nothing plays, so it is never called stalled
    playback = audio.Capture("playback")
    playback.current_id = "speakers"
    assert not playback.stalled(now=1000.0 + audio.MIC_STALL_SEC + 1)


def test_water_look_mirrors_and_lights_up_with_music():
    from led_mon.equalizer_files import looks
    left = looks.Visualizer({"style": "water"}, "left", seed=3)
    right = looks.Visualizer({"style": "water"}, "right", seed=3)
    t = 1.0
    for _ in range(60):
        samples = looks.demo_samples(t)
        a, b = left.frame(samples, 1 / 30), right.frame(samples, 1 / 30)
        t += 1 / 30
    assert b.max() > 0.3
    assert abs(a - b[::-1, :]).max() < 1e-9  # The left panel is the right one mirrored
    assert "water" in looks.NO_BAR_OPTIONS

# The calm idle version of each look, shown in music mode while nothing is heard

@pytest.mark.parametrize("style", looks.STYLE_KEYS)
@pytest.mark.parametrize("span", [False, True])
def test_every_look_has_a_calm_idle_version_that_moves(style, span):
    for side in ("left", "right"):
        v = looks.Visualizer({"style": style, "span-panels": span, "peak-dots": True, "shaded": True}, side, seed=1)
        frames = [v.idle_frame(0.03) for _ in range(200)]  # 6 seconds
        for grid in frames:
            assert grid.shape == (9, 34) and grid.min() >= 0.0 and grid.max() <= 1.0
        assert max(grid.max() for grid in frames) > 0, "the idle version should show something"
        assert any(not np.array_equal(frames[0], grid) for grid in frames), "the idle version should move"


@pytest.mark.parametrize("style", ["classic", "bars", "radial", "pulse", "bonfire"])
def test_idle_version_is_calmer_than_music(style):
    music = looks.Visualizer({"style": style}, "left", seed=1)
    idle = looks.Visualizer({"style": style}, "left", seed=1)
    lit_music = np.mean([(music.frame(looks.demo_samples(i * 0.03), 0.03) > 0.05).sum() for i in range(200)])
    lit_idle = np.mean([(idle.idle_frame(0.03) > 0.05).sum() for _ in range(200)])
    assert 0 < lit_idle < lit_music


def test_music_panel_switches_to_idle_after_silence_and_back_when_sound_returns():
    panel = looks.MusicPanel({"style": "bars"}, "left")
    quiet = np.zeros((looks.FFT_SIZE, 2))
    for i in range(60):
        panel.frame(looks.demo_samples(i * 0.03), 0.03)
    assert not panel.idle
    # Quiet, but not yet for long enough: the look fades out as before
    for _ in range(int((looks.IDLE_AFTER_SEC - 0.5) / 0.03)):
        grid = panel.frame(quiet, 0.03)
    assert not panel.idle and grid.max() <= 0.3
    for _ in range(int(1.0 / 0.03)):
        grid = panel.frame(quiet, 0.03)
    assert panel.idle and grid.max() > 0, "after the wait it shows the look's idle version"
    assert panel.is_silent()
    # A single click of sound doesn't end it
    panel.frame(looks.demo_samples(0.0), 0.03)
    assert panel.idle
    # Music that keeps playing does
    for i in range(int((looks.IDLE_RESUME_SEC + 0.2) / 0.03)):
        panel.frame(looks.demo_samples(i * 0.03), 0.03)
    assert not panel.idle


def test_music_mode_draws_the_idle_look_not_a_separate_silence_animation():
    # Read the source: importing visualize loads the monitor's plugins, which other tests don't expect
    import os
    path = os.path.join(os.path.dirname(looks.__file__), "visualize.py")
    with open(path, encoding="utf-8") as f:
        source = f.read()
    assert "looks.MusicPanel(" in source
    assert "silence_pulse" not in source
