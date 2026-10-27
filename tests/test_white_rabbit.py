# White Rabbit's own parts: sharing the panels between the monitor and the window,
# finding the panels, flashing firmware and choosing what to start.
import os

import pytest
import serial

from led_mon import serial_ports
from white_rabbit import bridge, firmware, main, panels


class FakeSerial:
    """A panel that answers the version command (0x20) and records everything written."""

    def __init__(self, name, baud):
        self.name = name
        self.is_open = True
        self.timeout = None
        self.written = []
        self.replies = bytearray()
        self.resets = 0

    def write(self, data):
        data = bytes(data)
        self.written.append(data)
        if data[:3] == bytes([0x32, 0xAC, 0x20]):
            self.replies += bytes([0, 0x20, 0] + [0] * 29)
        return len(data)

    def read(self, size=1):
        out, self.replies = bytes(self.replies[:size]), self.replies[size:]
        return out

    def reset_input_buffer(self):
        self.resets += 1
        self.replies.clear()

    def close(self):
        self.is_open = False


@pytest.fixture
def fake_ports(monkeypatch):
    monkeypatch.setattr(serial_ports.serial, "Serial", FakeSerial)
    yield
    serial_ports.close_all()
    serial_ports.release()


@pytest.fixture
def monitor(fake_ports):
    """A Link wired straight to the monitor-side handlers, as if over the control channel."""
    handlers = {"status": lambda m: {"ports": []}}
    stop = bridge.serve(handlers)

    def request(message, timeout=None):
        try:
            reply = handlers[message["cmd"]](message) or {}
            reply.setdefault("ok", True)
        except Exception as e:
            reply = {"ok": False, "error": str(e)}
        return reply
    yield bridge.Link(request)
    stop.set()


def test_proxy_sends_writes_with_the_read(monitor):
    drawing = serial_ports.open_port("COM4")  # the monitor's own handle
    s = bridge.ProxySerial(monitor, "COM4", timeout=2)
    s.reset_input_buffer()
    s.write([0x32, 0xAC])
    s.write([0x20])
    assert s.read(32)[:2] == bytes([0, 0x20])
    port = drawing._entry.serial
    assert port.written == [bytes([0x32, 0xAC, 0x20])]
    assert port.resets == 1
    # The monitor's handle keeps its own timeout after a read with a time limit
    assert port.timeout is None
    drawing.close()


def test_proxy_flushes_on_close(monitor):
    held = serial_ports.open_port("COM4")  # the monitor's own handle keeps the port open
    with bridge.ProxySerial(monitor, "COM4") as s:
        s.write(b"\x32\xac\x01\x05")
    assert held._entry.serial.written == [b"\x32\xac\x01\x05"]
    held.close()


def test_inputmodule_goes_through_the_monitor(monitor):
    from inputmodule.inputmodule import get_version
    from inputmodule import inputmodule as im
    old = im.serial_opener
    try:
        bridge.install(monitor)
        assert get_version(panels.Panel("left", "COM4")) == "0.2.0"
    finally:
        im.serial_opener = old


def test_hold_stops_the_monitor_until_release(monitor):
    drawing = serial_ports.open_port("COM4")
    monitor.hold(["COM4"])
    drawing.write(b"frame")
    with bridge.ProxySerial(monitor, "COM4") as s:
        s.write(b"game")
    assert drawing._entry.serial.written == [b"game"]
    monitor.release()
    drawing.write(b"frame")
    assert drawing._entry.serial.written == [b"game", b"frame"]
    drawing.close()


def test_closed_window_gives_panels_back(monkeypatch, fake_ports):
    monkeypatch.setattr(bridge, "_pid_alive", lambda pid: pid != 999)
    serial_ports.hold(["COM4"], owner=999)
    serial_ports.hold(["COM5"], owner=os.getpid())
    import threading
    stop = threading.Event()
    t = threading.Thread(target=bridge._watch_holders, args=(stop, 0.01))
    t.start()
    for _ in range(200):
        if not serial_ports.is_held("COM4"):
            break
        stop.wait(0.01)
    stop.set()
    t.join()
    assert not serial_ports.is_held("COM4")
    assert serial_ports.is_held("COM5")


def test_errors_reach_the_window(monitor):
    monitor.request = lambda message, timeout=None: {"ok": False, "error": "port gone"}
    with pytest.raises(serial.SerialException):
        bridge.ProxySerial(monitor, "COM9").read(1)


def test_direct_when_monitor_not_running():
    link = bridge.Link(lambda message, timeout=None: None)
    opened = []
    opener = bridge.make_opener(link, lambda port, timeout: opened.append(port) or "direct")
    assert opener("COM4") == "direct"
    assert opened == ["COM4"]
    link.hold(["COM4"])  # nothing to hold, and no error


class Port:
    def __init__(self, device, location, serial_number=""):
        self.device = device
        self.location = location
        self.vid, self.pid = 0x32AC, 0x0020
        self.serial_number = serial_number

    def __str__(self):
        return self.device


def test_panels_left_right_order():
    found = panels.find_panels({}, [Port("COM7", "1-3.3:1.0"), Port("COM4", "1-3.2:1.0", "ABC")])
    assert [(p.side, p.device) for p in found] == [("left", "COM4"), ("right", "COM7")]
    assert found[0].serial_number == "ABC"
    assert found[0].label == "Left panel (COM4)"


def test_panels_follow_the_layout_port_setting():
    found = panels.find_panels({"left-port": "COM7"}, [Port("COM7", "1-3.3:1.0"), Port("COM4", "1-3.2:1.0")])
    assert [p.device for p in found] == ["COM7", "COM4"]
    panels.find_panels({})  # reset the overrides for other tests


def test_bundled_firmware_lists_what_is_there(tmp_path):
    (tmp_path / "white-rabbit").mkdir()
    (tmp_path / "white-rabbit" / "ledmatrix.uf2").write_bytes(b"uf2")
    found = firmware.bundled_firmware(str(tmp_path))
    assert len(found) == 1 and found[0][1].endswith(os.path.join("white-rabbit", "ledmatrix.uf2"))


def test_flash_enters_bootloader_and_copies(tmp_path):
    uf2 = tmp_path / "ledmatrix.uf2"
    uf2.write_bytes(b"firmware")
    drive = tmp_path / "RPI-RP2"
    drive.mkdir()
    jumped = []
    drives = []

    def jump(panel):
        jumped.append(panel.device)
        drives.append(str(drive))

    message = firmware.flash(panels.Panel("left", "COM4"), str(uf2), get_drives=lambda: list(drives), jump=jump)
    assert jumped == ["COM4"]
    assert (drive / "NEW.UF2").read_bytes() == b"firmware"
    assert "Flashed 1 panel" in message


def test_flash_without_bootloader_drive(tmp_path, monkeypatch):
    uf2 = tmp_path / "ledmatrix.uf2"
    uf2.write_bytes(b"firmware")
    monkeypatch.setattr(firmware, "wait_for_drives", lambda get_drives: [])
    with pytest.raises(RuntimeError):
        firmware.flash(panels.Panel("left", "COM4"), str(uf2), get_drives=lambda: [], jump=lambda p: None)


@pytest.fixture
def started(monkeypatch):
    calls = []
    monkeypatch.setattr(main.platform_utils, "load_env_file", lambda: None)
    monkeypatch.setattr(main.platform_utils, "setup_logging", lambda: None)
    monkeypatch.setattr(main.platform_utils, "launch_self", lambda *a: calls.append(("launch",) + a))
    monkeypatch.setattr(main, "run_monitor", lambda argv: calls.append(("monitor",) + tuple(argv)))
    import white_rabbit.window
    monkeypatch.setattr(white_rabbit.window, "run_window",
                        lambda args, start_tab=None, wait=False: calls.append(("window", start_tab, wait)))
    return calls


def test_opening_the_app_on_windows_starts_monitor_and_window(monkeypatch, started):
    monkeypatch.setattr(main.platform_utils, "IS_WINDOWS", True)
    from led_mon import control
    monkeypatch.setattr(control, "request", lambda message, timeout=None: None)
    main.run([])
    assert started == [("launch", "--window", "--wait-for-monitor"), ("monitor",)]


def test_opening_the_app_again_only_opens_the_window(monkeypatch, started):
    monkeypatch.setattr(main.platform_utils, "IS_WINDOWS", True)
    from led_mon import control
    monkeypatch.setattr(control, "request", lambda message, timeout=None: {"ok": True})
    main.run([])
    assert started == [("launch", "--window")]


def test_background_and_window_options(started):
    main.run(["--background"])
    main.run(["--window", "--tab", "firmware"])
    main.run(["--edit-layout"])
    assert started == [("monitor",), ("window", "firmware", False), ("window", None, False)]
