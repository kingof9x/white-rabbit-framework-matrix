# "Keep every shade when dim": stretch the picture, dim with the LED current (firmware command 0x30).
import numpy as np

from led_mon import dimming


class FakePanel:
    def __init__(self, answers):
        self.answers = answers
        self.written = []
        self.timeout = 1.0
        self.port = "COM9"

    def reset_input_buffer(self):
        pass

    def write(self, data):
        self.written.append(bytes(data))
        return len(data)

    def read(self, size):
        return b"\x00" * size if self.answers else b""


def test_new_firmware_gets_full_range_values_and_lower_current():
    panel = FakePanel(answers=True)
    grid = np.array([[40, 20, 0]])
    out = dimming.prepare(panel, grid, True, 40)
    assert out.tolist() == [[255, 128, 0]]
    assert panel.written[0] == bytes([0x32, 0xAC, 0x30])        # The query
    assert panel.written[1] == bytes([0x32, 0xAC, 0x30, 40])    # Current set to the brightness
    assert panel.timeout == 1.0
    # The same brightness again: the current isn't sent again
    dimming.prepare(panel, grid, True, 40)
    assert len(panel.written) == 2


def test_older_firmware_is_left_alone():
    panel = FakePanel(answers=False)
    grid = np.array([[40, 20, 0]])
    assert dimming.prepare(panel, grid, True, 40) is grid
    assert dimming.prepare(panel, grid, True, 40) is grid
    assert panel.written == [bytes([0x32, 0xAC, 0x30])]         # Asked once only


def test_turning_it_off_puts_the_current_back_to_full():
    panel = FakePanel(answers=True)
    grid = np.array([[40]])
    dimming.prepare(panel, grid, True, 40)
    assert dimming.prepare(panel, grid, False, 40) is grid
    assert panel.written[-1] == bytes([0x32, 0xAC, 0x30, 255])
