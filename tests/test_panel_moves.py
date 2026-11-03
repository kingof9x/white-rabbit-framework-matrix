# Moving the input modules to other slots (e.g. the keyboard to the far left or far right) changes the
# panels' USB locations. Each drawing thread must find its panel again instead of staying blank.
import pytest
from serial.tools.list_ports_common import ListPortInfo

# led_mon.drawing loads the plugins, which need the full dependencies (the light test job skips this file)
pytest.importorskip("requests")
from led_mon import drawing, shared_state  # noqa: E402


def port(device, location, vid=0x32AC, pid=0x0020):
    info = ListPortInfo(device, skip_link_detection=True)
    info.vid, info.pid, info.location = vid, pid, location
    return info


KEYBOARD = dict(vid=0x32AC, pid=0x0012)  # not an LED panel


class FakeSerial:
    def __init__(self, name):
        self.port = name
        self.writes = []

    def write(self, data):
        self.writes.append(bytes(data))
        return len(data)

    def close(self):
        pass


@pytest.fixture
def usb(monkeypatch):
    """The fake serial ports the drawing threads see; set state['ports'] to 'move' modules."""
    state = {"ports": []}
    monkeypatch.setattr(drawing.list_ports, "comports", lambda: list(state["ports"]))
    monkeypatch.setattr(shared_state.list_ports, "comports", lambda: list(state["ports"]))
    monkeypatch.setattr(drawing, "open_port", lambda name: FakeSerial(name))
    monkeypatch.setattr(shared_state, "port_overrides", {})
    monkeypatch.setattr(shared_state, "location_overrides", {})
    drawing._claimed_ports.clear()
    yield state
    drawing._claimed_ports.clear()


def threads(usb):
    found = shared_state.discover_led_devices()
    locations = [loc or dev for loc, dev in found]
    left = drawing.DrawingThread(locations[0] if locations else None, drawing.LatestFrameQueue(), side_index=0)
    right = drawing.DrawingThread(locations[1] if len(locations) > 1 else None, drawing.LatestFrameQueue(),
                                  side_index=1)
    return left, right


def unplug_and_reconnect(*ts):
    for t in ts:
        t._close_serial_port()
    for t in ts:
        t._attempt_reconnect(force=True)


def test_keyboard_moved_far_left_both_panels_drawn(usb):
    # Keyboard in the middle: panel | keyboard | panel
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM9", "1-4.3:x.0", **KEYBOARD), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    assert (left.port_name, right.port_name) == ("COM4", "COM5")
    # Keyboard moved to the far left: keyboard | panel | panel. The left panel's old slot now has the keyboard.
    usb["ports"] = [port("COM9", "1-4.2:x.0", **KEYBOARD), port("COM4", "1-4.3:x.0"), port("COM5", "1-4.4:x.0")]
    unplug_and_reconnect(left, right)
    assert {left.port_name, right.port_name} == {"COM4", "COM5"}


def test_keyboard_moved_far_right_both_panels_drawn(usb):
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM9", "1-4.3:x.0", **KEYBOARD), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    # panel | panel | keyboard: the right panel's old slot now has the keyboard
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM5", "1-4.3:x.0"), port("COM9", "1-4.4:x.0", **KEYBOARD)]
    unplug_and_reconnect(left, right)
    assert (left.port_name, right.port_name) == ("COM4", "COM5")


def test_only_one_thread_reconnects_after_move(usb):
    # Only the moved panel errors out; the other keeps drawing. They must not end up on the same panel.
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM9", "1-4.3:x.0", **KEYBOARD), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM5", "1-4.3:x.0"), port("COM9", "1-4.4:x.0", **KEYBOARD)]
    unplug_and_reconnect(right)
    assert (left.port_name, right.port_name) == ("COM4", "COM5")


def test_new_com_numbers_after_move(usb):
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    usb["ports"] = [port("COM7", "1-4.3:x.0"), port("COM8", "1-4.4:x.0")]
    unplug_and_reconnect(left, right)
    assert {left.port_name, right.port_name} == {"COM7", "COM8"}


def test_second_panel_that_appears_later_gets_drawn(usb):
    # Only one panel was up when the app started (another slot came up a few seconds later)
    usb["ports"] = [port("COM4", "1-4.2:x.0")]
    left, right = threads(usb)
    assert (left.port_name, right.port_name) == ("COM4", None)
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM5", "1-4.4:x.0")]
    right._attempt_reconnect(force=True)
    assert right.port_name == "COM5"


def test_two_threads_never_share_a_panel_while_one_is_missing(usb):
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    usb["ports"] = [port("COM5", "1-4.4:x.0")]  # left panel being moved
    unplug_and_reconnect(left)
    assert left.port_name is None and right.port_name == "COM5"
    usb["ports"] = [port("COM4", "1-4.3:x.0"), port("COM5", "1-4.4:x.0")]
    left._attempt_reconnect(force=True)
    assert left.port_name == "COM4"


def test_frames_reach_the_moved_panel(usb):
    usb["ports"] = [port("COM4", "1-4.2:x.0"), port("COM9", "1-4.3:x.0", **KEYBOARD), port("COM5", "1-4.4:x.0")]
    left, right = threads(usb)
    usb["ports"] = [port("COM9", "1-4.2:x.0", **KEYBOARD), port("COM4", "1-4.3:x.0"), port("COM5", "1-4.4:x.0")]
    unplug_and_reconnect(left, right)
    import numpy as np
    for t in (left, right):
        t.start()
        t.input_queue.put((np.zeros((9, 34), dtype=int), None))
        t.input_queue.put(None)
        t.join(timeout=3)
    assert {left.serial_port, right.serial_port} == {None}  # closed cleanly at the end
