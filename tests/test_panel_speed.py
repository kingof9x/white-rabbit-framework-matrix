# Keeping the panels up to date: only the newest frame waits, frames already on a panel aren't
# sent again, dark columns are left out, and several panels are written in turns.
import time

import numpy as np
import pytest

from led_mon import serial_ports
try:
    from led_mon import drawing
except Exception as e:  # the monitor's full dependencies aren't installed
    pytest.skip(f"full dependencies not installed: {e}", allow_module_level=True)
from tests.test_white_rabbit import FakeSerial, fake_ports, monitor  # noqa: F401


def frame(value):
    return np.full((9, 34), value, dtype=int)


def test_new_frame_replaces_the_waiting_one():
    q = drawing.LatestFrameQueue()
    q.put((frame(1), None))
    q.put((frame(2), None))
    q.put((frame(3), None))
    assert q.qsize() == 1
    grid, animate = q.get()
    assert grid[0, 0] == 3 and animate is None


def test_animation_changes_and_other_items_keep_their_place():
    q = drawing.LatestFrameQueue()
    q.put((frame(1), False))
    q.put((frame(2), None))      # replaces it and keeps the False
    q.put((frame(3), True))      # an animation change: waits behind it
    q.put(drawing.SLEEP_PANEL)
    q.put((frame(4), None))
    q.put_nowait(None)
    items = [q.get_nowait() for _ in range(q.qsize())]
    assert items[0][0][0, 0] == 2 and items[0][1] is False
    assert items[1][0][0, 0] == 3 and items[1][1] is True
    assert items[2] == drawing.SLEEP_PANEL
    assert items[3][0][0, 0] == 4
    assert items[4] is None
    with pytest.raises(Exception):
        q.get_nowait()


def test_unchanged_frame_is_not_sent_again(fake_ports):
    s = serial_ports.open_port("COM4")
    port = s._entry.serial
    grid = np.zeros((9, 34), dtype=int)
    grid[2, :] = 100
    drawing.draw_to_LEDs(s, grid)
    # One lit column and the commit; the dark columns are left out
    assert [w[:4] for w in port.written] == [bytes([0x32, 0xAC, 0x07, 2]), bytes([0x32, 0xAC, 0x08])[:4]]
    port.written.clear()
    drawing.draw_to_LEDs(s, grid)
    assert port.written == []
    # Any other command (brightness, sleep...) means the frame is drawn again next time
    s.write(bytes([0x32, 0xAC, 0x00, 0x40]))
    drawing.draw_to_LEDs(s, grid)
    assert len(port.written) == 3


def test_unchanged_frame_is_still_sent_now_and_then(fake_ports, monkeypatch):
    s = serial_ports.open_port("COM4")
    port = s._entry.serial
    drawing.draw_to_LEDs(s, frame(5))
    now = time.monotonic()
    monkeypatch.setattr(serial_ports.time, "monotonic", lambda: now + drawing.RESEND_UNCHANGED_SEC + 1)
    port.written.clear()
    drawing.draw_to_LEDs(s, frame(5))
    assert len(port.written) == 10  # keeps the panel from going to sleep


def test_frame_is_drawn_again_after_the_window_gives_a_panel_back(fake_ports):
    s = serial_ports.open_port("COM4")
    port = s._entry.serial
    drawing.draw_to_LEDs(s, frame(5))
    serial_ports.hold(["COM4"], owner=1)
    drawing.draw_to_LEDs(s, frame(6))   # dropped while the window has the panel
    serial_ports.release(["COM4"])
    port.written.clear()
    drawing.draw_to_LEDs(s, frame(6))
    assert len(port.written) == 10


def test_math_frames_take_turns_between_panels(monitor):
    from white_rabbit import math_tab
    from white_rabbit.panels import Panel
    left = serial_ports.open_port("COM4")
    right = serial_ports.open_port("COM5")
    order = []
    for name, handle in (("L", left), ("R", right)):
        original = handle._entry.serial.write

        def write(data, name=name, original=original):
            order.append(name)
            return original(data)
        handle._entry.serial.write = write
    wide = np.zeros((34, 18), dtype=np.uint8)
    wide[:, :9] = 50
    wide[:, 9:] = 90
    wide[:, 12] = 0  # a dark column on the right panel is left out
    math_tab.send_frames(monitor, [Panel("left", "COM4"), Panel("right", "COM5")], wide)
    assert order[:6] == ["L", "R", "L", "R", "L", "R"]
    lw, rw = left._entry.serial.written, right._entry.serial.written
    assert len(lw) == 10 and len(rw) == 9
    assert all(list(w[4:]) == [50] * 34 for w in lw[:9])
    assert all(list(w[4:]) == [90] * 34 for w in rw[:8])
    assert 3 not in [w[3] for w in rw[:8]]
    assert lw[-1] == rw[-1] == bytes([0x32, 0xAC, 0x08, 0x00])
    left.close()
    right.close()
