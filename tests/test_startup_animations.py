# The startup animation setting sent to the panels (inputmodule/inputmodule/ledmatrix.py): the list
# matches the firmware, and the set of animations is sent in as few bytes as the firmware needs.
import io
from contextlib import contextmanager

import inputmodule.gui.gui_threading  # noqa: F401  (loads first, as in the app, to avoid an import loop)
from inputmodule.inputmodule import ledmatrix
from inputmodule.inputmodule.ledmatrix import (
    STARTUP_ANIMATIONS, STARTUP_ANIMATION_NAMES, ALL_STARTUP_ANIMATIONS, StartupMode,
    set_startup_animation, get_startup_animation)


def sent_args(monkeypatch, *args):
    sent = []
    monkeypatch.setattr(ledmatrix, "send_command", lambda dev, cmd, params=[], **kw: sent.append(params))
    set_startup_animation(None, *args)
    return sent[0]


def test_every_animation_has_a_name_for_the_command_line():
    assert len(STARTUP_ANIMATIONS) == len(STARTUP_ANIMATION_NAMES) == 28
    assert STARTUP_ANIMATIONS[22] == "Rabbit hole" and STARTUP_ANIMATION_NAMES[27] == "campfire"
    assert ALL_STARTUP_ANIMATIONS == (1 << 28) - 1


def test_set_uses_3_bytes_when_the_new_animations_are_not_picked(monkeypatch):
    # So it still works on White Rabbit firmware from before the new animations
    assert sent_args(monkeypatch, StartupMode.RandomSet, 0, 0b101) == [StartupMode.RandomSet, 0, 5, 0, 0]


def test_other_modes_keep_to_3_bytes_for_older_firmware(monkeypatch):
    assert sent_args(monkeypatch, StartupMode.RandomAll) == [StartupMode.RandomAll, 0, 0xFF, 0xFF, 0xFF]
    assert sent_args(monkeypatch, StartupMode.Fixed, 3) == [StartupMode.Fixed, 3, 0xFF, 0xFF, 0xFF]


def test_set_uses_4_bytes_for_the_new_animations(monkeypatch):
    args = sent_args(monkeypatch, StartupMode.RandomSet, 0, (1 << 22) | (1 << 27))
    assert args == [StartupMode.RandomSet, 0, 0, 0, 0x40, 0x08]


class FakeSerial(io.BytesIO):
    def reset_input_buffer(self):
        pass

    def write(self, data):
        pass


def test_reads_all_4_bytes_of_the_set(monkeypatch):
    response = bytes([StartupMode.RandomSet, 0, 0, 0, 0x40, 0x08]) + bytes(26)

    @contextmanager
    def fake_open(*a, **kw):
        yield FakeSerial(response)
    monkeypatch.setattr(ledmatrix, "open_serial", fake_open)

    class Dev:
        device = "COM9"
    assert get_startup_animation(Dev()) == (StartupMode.RandomSet, 0, (1 << 22) | (1 << 27))
