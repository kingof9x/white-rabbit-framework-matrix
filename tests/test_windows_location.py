# The weather app's "Windows Location" option: led_mon/windows_location.py with a stand-in for the
# WinRT geolocation API, and how the weather falls back when Windows can't give a location.
import enum
import importlib.util
import os
from types import SimpleNamespace

import pytest

from led_mon import windows_location as wl
from led_mon.editor import catalog


class AccessStatus(enum.IntEnum):
    UNSPECIFIED = 0
    ALLOWED = 1
    DENIED = 2


class PositionStatus(enum.IntEnum):
    READY = 0
    DISABLED = 3


def fake_geolocation(access=AccessStatus.ALLOWED, status=PositionStatus.READY, position=(51.5, -0.12),
                     error=None, calls=None):
    """Something shaped like winrt.windows.devices.geolocation."""
    calls = calls if calls is not None else []

    class Geolocator:
        @staticmethod
        async def request_access_async():
            return access

        @property
        def location_status(self):
            return status

        async def get_geoposition_async(self):
            calls.append("lookup")
            if error:
                raise error
            point = SimpleNamespace(position=SimpleNamespace(latitude=position[0], longitude=position[1]))
            return SimpleNamespace(coordinate=SimpleNamespace(point=point, accuracy=40.0))

    return SimpleNamespace(Geolocator=Geolocator, GeolocationAccessStatus=AccessStatus, PositionStatus=PositionStatus)


@pytest.fixture(autouse=True)
def fresh():
    wl._reset_for_tests()
    yield
    wl._reset_for_tests()


def use(monkeypatch, geolocation):
    monkeypatch.setattr(wl, "_geolocation", lambda: geolocation)


def test_position_when_allowed(monkeypatch):
    use(monkeypatch, fake_geolocation())
    assert wl.get_position() == (51.5, -0.12, 40.0)
    assert wl.access_status() == wl.ALLOWED


def test_position_is_reused_between_weather_refreshes(monkeypatch):
    calls = []
    use(monkeypatch, fake_geolocation(calls=calls))
    wl.get_position()
    wl.get_position()
    assert calls == ["lookup"]
    wl.get_position(refresh=True)
    assert calls == ["lookup", "lookup"]


@pytest.mark.parametrize("access", [AccessStatus.DENIED, AccessStatus.UNSPECIFIED])
def test_not_allowed(monkeypatch, access):
    use(monkeypatch, fake_geolocation(access=access))
    with pytest.raises(wl.LocationUnavailable) as e:
        wl.get_position()
    assert e.value.reason == wl.DENIED
    assert "Let desktop apps access your location" in str(e.value)
    assert wl.access_status() == wl.DENIED


def test_location_services_off(monkeypatch):
    use(monkeypatch, fake_geolocation(status=PositionStatus.DISABLED))
    with pytest.raises(wl.LocationUnavailable) as e:
        wl.get_position()
    assert e.value.reason == wl.OFF
    assert wl.access_status() == wl.OFF


def test_access_denied_error_from_windows(monkeypatch):
    error = OSError(wl.E_ACCESSDENIED, "Access is denied")
    use(monkeypatch, fake_geolocation(error=error))
    with pytest.raises(wl.LocationUnavailable) as e:
        wl.get_position()
    assert e.value.reason == wl.DENIED


def test_failure_is_not_retried_straight_away(monkeypatch):
    calls = []
    use(monkeypatch, fake_geolocation(error=OSError(-2147023436, "This operation returned because the timeout period expired"), calls=calls))
    for _ in range(3):
        with pytest.raises(wl.LocationUnavailable) as e:
            wl.get_position()
        assert e.value.reason == wl.NO_POSITION
    assert calls == ["lookup"]


def test_recent_position_kept_when_windows_briefly_has_no_fix(monkeypatch):
    use(monkeypatch, fake_geolocation())
    first = wl.get_position()
    use(monkeypatch, fake_geolocation(error=OSError(-2147023436, "This operation returned because the timeout period expired")))
    assert wl.get_position(refresh=True) == first


def test_recent_position_not_kept_once_location_is_turned_off(monkeypatch):
    use(monkeypatch, fake_geolocation())
    wl.get_position()
    use(monkeypatch, fake_geolocation(access=AccessStatus.DENIED))
    with pytest.raises(wl.LocationUnavailable):
        wl.get_position(refresh=True)


def test_unavailable_off_windows(monkeypatch):
    monkeypatch.setattr(wl.platform_utils, "IS_WINDOWS", False)
    with pytest.raises(wl.LocationUnavailable) as e:
        wl.get_position()
    assert e.value.reason == wl.UNAVAILABLE
    assert wl.access_status() == wl.UNAVAILABLE
    assert wl.describe().startswith("not available")


def test_every_outcome_has_a_message():
    for reason in (wl.ALLOWED, wl.DENIED, wl.OFF, wl.UNAVAILABLE, wl.NO_POSITION):
        assert wl.MESSAGES[reason]


@pytest.mark.skipif(os.name != "nt", reason="the real Windows Location API")
def test_real_windows_api_answers():
    pytest.importorskip("winrt.windows.devices.geolocation")
    # The CI runner usually has location off; any clear answer is fine, as long as it doesn't crash
    assert wl.access_status() in (wl.ALLOWED, wl.DENIED, wl.OFF)
    assert wl.describe()


def test_new_weather_app_uses_the_typed_in_location():
    entry = catalog.new_app("weather", "left")
    assert entry["args"]["location_source"] == "manual"
    setting = next(s for s in catalog.get("weather").settings if s.key == "location_source")
    assert setting.options == ("manual", "windows")
    assert len(setting.labels) == 2


def load_weather_plugin():
    pytest.importorskip("requests")
    path = os.path.join(os.path.dirname(wl.__file__), "plugins", "time_weather_plugin.py")
    spec = importlib.util.spec_from_file_location("time_weather_plugin_test", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_weather_follows_windows_location(monkeypatch):
    plugin = load_weather_plugin()
    use(monkeypatch, fake_geolocation(position=(40.7, -74.0)))
    assert plugin.get_location(None, [1.0, 2.0], None, None, "windows") == (40.7, -74.0)


def test_weather_falls_back_to_typed_in_location(monkeypatch):
    plugin = load_weather_plugin()
    use(monkeypatch, fake_geolocation(access=AccessStatus.DENIED))
    assert plugin.get_location(None, [1.0, 2.0], None, None, "windows") == (1.0, 2.0)


def test_weather_falls_back_to_internet_address(monkeypatch):
    plugin = load_weather_plugin()
    use(monkeypatch, fake_geolocation(status=PositionStatus.DISABLED))
    monkeypatch.setattr(plugin, "get_location_by_ip_keyless", lambda: (3.0, 4.0))
    assert plugin.get_location(None, None, None, None, "windows") == (3.0, 4.0)


def test_typed_in_location_ignores_windows(monkeypatch):
    plugin = load_weather_plugin()
    calls = []
    use(monkeypatch, fake_geolocation(calls=calls))
    assert plugin.get_location(None, [1.0, 2.0], None, None, "manual") == (1.0, 2.0)
    assert plugin.get_location(None, [1.0, 2.0], None, None) == (1.0, 2.0)
    assert calls == []
