# The fps app: reading PresentMon's CSV output, and drawing
import sys
import time
import logging
import textwrap
import importlib

import pytest

np = pytest.importorskip("numpy")
pytest.importorskip("psutil")

from led_mon import fps
from led_mon.editor import catalog
from led_mon.platform_utils import IS_WINDOWS

if IS_WINDOWS:
    pytest.importorskip("wmi")

HEADER = ["Application", "ProcessID", "SwapChainAddress", "PresentRuntime", "CPUStartTime", "MsBetweenPresents"]


@pytest.fixture
def drawing(monkeypatch):
    monkeypatch.setattr(sys, "argv", ["led_mon", "--disable-plugins"])
    return importlib.import_module("led_mon.drawing")


def stats_with(frames, now=100.0):
    stats = fps.FrameStats()
    stats.add_header(HEADER)
    for app, pid, swapchain, t, ms in frames:
        assert stats.add_row([app, str(pid), swapchain, "DXGI", str(t), str(ms)], now=now)
    return stats


def test_fps_from_the_busiest_swap_chain_over_the_last_second():
    frames = [("game.exe", 7, "0xA", i / 60, 1000 / 60) for i in range(120)]
    frames += [("game.exe", 7, "0xB", i / 10, 100.0) for i in range(20)]
    frames += [("other.exe", 9, "0xC", i / 30, 1000 / 30) for i in range(30)]
    stats = stats_with(frames)
    assert stats.fps(7, now=100.0) == pytest.approx(60)
    assert stats.fps(9, now=100.0) == pytest.approx(30)
    assert stats.pids_named("GAME.EXE") == [7]
    # Stopped presenting: 0
    assert stats.fps(7, now=100.0 + fps.STALE_SECONDS + 1) == 0.0
    assert stats.fps(12345, now=100.0) == 0.0


def test_rows_that_arent_frames_are_ignored():
    stats = fps.FrameStats()
    assert not stats.add_row(["anything"])  # before the header
    stats.add_header(HEADER)
    assert not stats.add_row(["game.exe", "not a pid", "0xA", "DXGI", "1.0", "16.6"])
    assert not stats.add_row(["game.exe"])


def test_draws_digits_gpu_graph_and_question_mark(drawing):
    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_fps(grid, 144.4, [1.0, 0.5, None], True, 160)
    rows = grid.T
    for slot, digit in enumerate("144"):
        assert (rows[1 + slot * 8:8 + slot * 8, :] == drawing.numerals[digit] * 160).all()
    assert (grid[1:8, 26] == 160).all()           # GPU at 100%: full row
    assert list(grid[1:5, 27]) == [160, 160, 160, 80]  # 50%: 3.5 pixels
    assert not grid[:, 28].any()                    # no reading

    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_fps(grid, 60, [], True, 160)
    assert not grid.T[1:8, :].any()                 # leading blank for two digits
    assert (grid.T[9:16, :] == drawing.numerals["6"] * 160).all()

    # FPS can't be measured: a "?", and the GPU graph still shows
    grid = np.zeros((9, 34), dtype=int)
    drawing.draw_fps(grid, None, [1.0], False, 160)
    assert (grid.T[13:19, :] == drawing.symbols["?"] * 160).all()
    assert (grid[1:8, 26] == 160).all()


def test_fps_graph_shows_the_chosen_gpu(drawing, monkeypatch):
    from led_mon import gpu
    from led_mon.rendering import make_base_app_functions, render_panel
    from led_mon.monitors import CPUMonitor, MemoryMonitor, BatteryMonitor, DiskMonitor, NetworkMonitor
    asked = []

    class FakeGpus:
        def get(self, name=None):
            asked.append(name)
            return {"RX 7700S": 1.0}.get(name, 0.0)
    monkeypatch.setattr(gpu, "shared_monitor", lambda: FakeGpus())
    monkeypatch.setattr(fps, "find_presentmon", lambda: None)
    functions = make_base_app_functions(CPUMonitor(), MemoryMonitor(), BatteryMonitor(), DiskMonitor(), NetworkMonitor())
    grid = np.zeros((9, 34), dtype=int)
    render_panel(grid, [{"name": "fps", "scope": "panel", "args": {"gpu": "RX 7700S"}}], functions, 160, 35)
    assert asked == ["RX 7700S"]
    assert (grid[1:8, 26] == 160).all()


FAKE_PRESENTMON = '''
import sys, time
args = sys.argv[1:]
assert "--output_stdout" in args and "--session_name" in args
if "--terminate_existing_session" in args:
    sys.exit(0)
if "MODE" == "denied":
    sys.stderr.write("error: failed to start trace session (access denied).\\n")
    sys.exit(5)
print("Application,ProcessID,SwapChainAddress,PresentRuntime,CPUStartTime,MsBetweenPresents", flush=True)
t = 0.0
while True:
    for _ in range(10):
        t += 0.02
        print(f"game.exe,4242,0x1,DXGI,{t:.4f},20.0", flush=True)
    time.sleep(0.2)
'''


def fake_presentmon(tmp_path, mode):
    script = tmp_path / "fake_presentmon.py"
    script.write_text(textwrap.dedent(FAKE_PRESENTMON).replace("MODE", mode))
    return [sys.executable, str(script)]


@pytest.fixture
def fake_capture(monkeypatch, tmp_path):
    def make(mode, process_name=None):
        cmd = fake_presentmon(tmp_path, mode)
        monkeypatch.setattr(fps, "IS_WINDOWS", True)
        monkeypatch.setattr(fps, "find_presentmon", lambda: cmd[0])
        monkeypatch.setattr(fps, "foreground_process_id", lambda: None)
        monkeypatch.setattr(fps, "foreground_is_full_screen", lambda: False)
        capture = fps.PresentMonCapture(process_name)
        # Run the fake through Python, with PresentMon's arguments
        original = capture.command
        monkeypatch.setattr(capture, "command", lambda exe: cmd + original(exe)[1:])
        monkeypatch.setattr(fps.platform_utils, "subprocess_flags", lambda: 0)
        return capture
    return make


def wait_for(condition, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if condition():
            return True
        time.sleep(0.1)
    return False


def test_capture_by_app_name(fake_capture):
    capture = fake_capture("ok", "Game.exe")
    try:
        assert wait_for(lambda: (capture.get()[0] or 0) > 40)
        value, status = capture.get()
        assert status == fps.OK and value == pytest.approx(50, rel=0.1)
    finally:
        capture.stop()
    assert capture.process is None


def test_capture_follows_the_app_in_front(fake_capture, monkeypatch):
    capture = fake_capture("ok")
    monkeypatch.setattr(fps, "foreground_process_id", lambda: 4242)
    try:
        assert wait_for(lambda: (capture.get()[0] or 0) > 40)
        monkeypatch.setattr(fps, "foreground_process_id", lambda: 1)
        assert capture.get() == (0.0, fps.OK)
    finally:
        capture.stop()


def test_automatic_keeps_the_full_screen_game_while_other_windows_are_in_front(monkeypatch):
    now = time.monotonic()
    frames = [("game.exe", 7, "0xA", i / 60, 1000 / 60) for i in range(60)]
    frames += [("chrome.exe", 8, "0xB", i / 30, 1000 / 30) for i in range(30)]
    capture = fps.PresentMonCapture()
    capture.stats = stats_with(frames, now=now)
    front = {"pid": 7, "full": True}
    monkeypatch.setattr(fps, "foreground_process_id", lambda: front["pid"])
    monkeypatch.setattr(fps, "foreground_is_full_screen", lambda: front["full"])

    assert capture.automatic() == pytest.approx(60)    # the game, full screen
    front.update(pid=8, full=False)                    # Alt-Tab to a browser playing a video
    assert capture.automatic() == pytest.approx(60)
    front.update(pid=99, full=False)                   # this app's window, not drawing
    assert capture.automatic() == pytest.approx(60)

    # The game stops drawing (closed): back to whatever is in front
    capture.stats.last_seen[7] = now - fps.STALE_SECONDS - 1
    front.update(pid=8, full=False)
    assert capture.automatic() == pytest.approx(30)
    assert capture.game_pid is None


def test_automatic_without_a_game_follows_the_window_in_front(monkeypatch):
    now = time.monotonic()
    capture = fps.PresentMonCapture()
    capture.stats = stats_with([("windowed.exe", 5, "0xA", i / 40, 25.0) for i in range(40)], now=now)
    monkeypatch.setattr(fps, "foreground_process_id", lambda: 5)
    monkeypatch.setattr(fps, "foreground_is_full_screen", lambda: False)
    assert capture.automatic() == pytest.approx(40)
    assert capture.game_pid is None
    # The desktop covers the screen but draws nothing: not a game
    monkeypatch.setattr(fps, "foreground_process_id", lambda: 1)
    monkeypatch.setattr(fps, "foreground_is_full_screen", lambda: True)
    assert capture.automatic() == 0.0
    assert capture.game_pid is None


def test_game_picker_lists_open_apps_drawing_frames_first(monkeypatch):
    now = time.monotonic()
    stats = stats_with([("eldenring.exe", 7, "0xA", i / 60, 1000 / 60) for i in range(60)]
                       + [("chrome.exe", 9, "0xB", i / 30, 1000 / 30) for i in range(30)], now=now)
    windows = [
        (3, "Discord.exe", "Discord"),
        (8, "chrome.exe", "Gmail - Google Chrome"),
        (9, "chrome.exe", "YouTube - Google Chrome"),
        (7, "eldenring.exe", "ELDEN RING"),
        (4, "explorer.exe", "Downloads"),
        (5, "WhiteRabbit.exe", "White Rabbit"),
        (6, "Notepad.exe", "notes.txt - Notepad"),
    ]
    apps = fps.running_apps(stats=stats, windows=windows)
    assert [(exe, title, round(rate)) for exe, title, rate in apps] == [
        ("eldenring.exe", "ELDEN RING", 60),
        ("chrome.exe", "YouTube - Google Chrome", 30),  # one entry per program, named by the window that draws
        ("Discord.exe", "Discord", 0),
        ("Notepad.exe", "notes.txt - Notepad", 0),
    ]


def test_access_denied_shows_no_permission(fake_capture, caplog):
    capture = fake_capture("denied")
    with caplog.at_level(logging.WARNING):
        capture.get()
        assert wait_for(lambda: capture.status == fps.NO_PERMISSION)
    assert capture.get() == (None, fps.NO_PERMISSION)
    assert "Performance Log Users" in caplog.text


def test_not_available_without_presentmon(monkeypatch):
    monkeypatch.setattr(fps, "find_presentmon", lambda: None)
    capture = fps.PresentMonCapture()
    assert capture.get() == (None, fps.NOT_AVAILABLE)


def test_fps_app_renders_without_errors(drawing, caplog, monkeypatch):
    monkeypatch.setattr(fps, "find_presentmon", lambda: None)
    from led_mon.rendering import make_base_app_functions, render_panel
    from led_mon.monitors import CPUMonitor, MemoryMonitor, BatteryMonitor, DiskMonitor, NetworkMonitor
    functions = make_base_app_functions(CPUMonitor(), MemoryMonitor(), BatteryMonitor(), DiskMonitor(), NetworkMonitor())
    for app in ({"name": "fps", "scope": "panel"}, {"name": "fps", "scope": "panel", "args": {"process": "game.exe"}}):
        grid = np.zeros((9, 34), dtype=int)
        with caplog.at_level(logging.ERROR):
            render_panel(grid, [app], functions, 160, 35)
        assert not [r for r in caplog.records if r.levelno >= logging.ERROR]
        assert grid.any()  # the "?"


def test_editor_offers_the_fps_app(drawing):
    info = catalog.get("fps")
    assert info.size == catalog.FULL
    assert catalog.new_app("fps", "left") == {"name": "fps"}
    assert [s.key for s in info.settings] == ["process", "gpu"]
    assert drawing.id_patterns["fps"].shape == (9, 34)
