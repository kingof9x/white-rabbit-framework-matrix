# The tray icon's fixed GUID, which lets Windows remember it (Settings > Taskbar > Other system tray icons)
from led_mon import tray


def test_guid_is_stable_for_one_exe_and_ignores_case():
    a = tray.tray_icon_guid(r"C:\Users\me\AppData\Local\Programs\WhiteRabbit\WhiteRabbit.exe")
    b = tray.tray_icon_guid(r"c:\users\ME\appdata\local\programs\whiterabbit\WHITERABBIT.EXE")
    assert a == b
    assert a == tray.tray_icon_guid(r"C:\Users\me\AppData\Local\Programs\WhiteRabbit\WhiteRabbit.exe")


def test_guid_differs_for_another_copy():
    installed = tray.tray_icon_guid(r"C:\Program Files\WhiteRabbit\WhiteRabbit.exe")
    portable = tray.tray_icon_guid(r"C:\Users\me\Downloads\WhiteRabbit.exe")
    assert installed != portable


class FakeIcon(tray.StableTrayIdMixin):
    def __init__(self, guid, refuse_guid=False):
        self._tray_guid = guid
        self.refuse_guid = refuse_guid
        self.sent = []

    def _send(self, code, flags, **kwargs):
        self.sent.append((code, flags, kwargs))
        if self.refuse_guid and "guidItem" in kwargs:
            return False
        return True


def test_add_clears_a_leftover_icon_then_adds_with_guid():
    icon = FakeIcon("G")
    icon._message(tray.NIM_ADD, 0x7, szTip="White Rabbit")
    assert icon.sent == [
        (tray.NIM_DELETE, tray.NIF_GUID, {"guidItem": "G"}),
        (tray.NIM_ADD, 0x7 | tray.NIF_GUID, {"szTip": "White Rabbit", "guidItem": "G"}),
    ]
    icon.sent.clear()
    icon._message(tray.NIM_MODIFY, 0x2, hIcon=5)
    assert icon.sent == [(tray.NIM_MODIFY, 0x2 | tray.NIF_GUID, {"hIcon": 5, "guidItem": "G"})]


def test_refused_guid_falls_back_so_the_icon_still_shows():
    icon = FakeIcon("G", refuse_guid=True)
    icon._message(tray.NIM_ADD, 0x7, szTip="White Rabbit")
    assert icon.sent[-1] == (tray.NIM_ADD, 0x7, {"szTip": "White Rabbit"})
    assert icon._tray_guid is None
    icon.sent.clear()
    icon._message(tray.NIM_DELETE, 0)
    assert icon.sent == [(tray.NIM_DELETE, 0, {})]


def test_without_guid_messages_pass_through():
    icon = FakeIcon(None)
    icon._message(tray.NIM_ADD, 0x7, szTip="x")
    assert icon.sent == [(tray.NIM_ADD, 0x7, {"szTip": "x"})]
