# Where this computer is, from the Windows Location service (Settings > Privacy & security > Location).
# The weather app uses it when its "Location" setting is "Windows Location". When the service is off,
# White Rabbit isn't allowed to use it, or it can't find a position, the weather falls back to the
# zip code / coordinates typed in, or to the internet address.

# Built In Dependencies
import asyncio
import logging
import threading
import time

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

# Where in Windows Settings the user turns location on for desktop apps
SETTINGS_URI = "ms-settings:privacy-location"

# A position found is reused for this long (the weather refreshes every 30 seconds)
POSITION_MAX_AGE = 15 * 60
# After a failure, wait this long before asking Windows again
FAILURE_RETRY = 2 * 60
# Longest wait for Windows to find a position
LOOKUP_TIMEOUT = 15

ALLOWED = "allowed"
DENIED = "denied"
OFF = "off"
UNAVAILABLE = "unavailable"   # not Windows, or the WinRT parts are missing
NO_POSITION = "no_position"   # allowed, but Windows didn't find where the computer is

# Plain words for each outcome, shown in the layout editor and the log
MESSAGES = {
    ALLOWED: "Windows Location is on.",
    DENIED: "White Rabbit isn't allowed to use your location. In Windows Settings > Privacy & security > "
            "Location, turn on Location services and \"Let desktop apps access your location\".",
    OFF: "Location services are off in Windows. Turn them on in Settings > Privacy & security > Location.",
    UNAVAILABLE: "Windows Location can't be used on this computer.",
    NO_POSITION: "Windows couldn't find where this computer is right now (no Wi-Fi or GPS fix yet).",
}

E_ACCESSDENIED = -2147024891  # 0x80070005


class LocationUnavailable(Exception):
    def __init__(self, reason, detail=""):
        self.reason = reason
        self.detail = detail
        super().__init__(MESSAGES.get(reason, reason) + (f" ({detail})" if detail else ""))


_lock = threading.Lock()
_position = None        # (lat, lon, accuracy in metres)
_position_time = 0.0
_failure = None         # the last LocationUnavailable
_failure_time = 0.0


def _geolocation():
    if not platform_utils.IS_WINDOWS:
        raise LocationUnavailable(UNAVAILABLE, "only on Windows")
    try:
        from winrt.windows.devices import geolocation
    except Exception as e:
        raise LocationUnavailable(UNAVAILABLE, f"WinRT geolocation isn't installed: {e}")
    return geolocation


def _is_access_denied(error):
    code = getattr(error, "winerror", None)
    if code is None and error.args and isinstance(error.args[0], int):
        code = error.args[0]
    if code is not None and (code == E_ACCESSDENIED or code == E_ACCESSDENIED & 0xFFFFFFFF):
        return True
    return isinstance(error, PermissionError)


def _run(make_coro):
    """Run a WinRT call on a thread of its own: it gets its own COM apartment and event loop, whichever
    thread (the weather's, the tray's, the window's) asked."""
    result = {}

    def work():
        try:
            try:
                import winrt.runtime
                winrt.runtime.init_apartment(winrt.runtime.ApartmentType.MULTI_THREADED)
            except Exception:
                pass  # already set up on this thread
            result["value"] = asyncio.run(asyncio.wait_for(make_coro(), LOOKUP_TIMEOUT))
        except BaseException as e:
            result["error"] = e

    thread = threading.Thread(target=work, name="windows-location", daemon=True)
    thread.start()
    thread.join(LOOKUP_TIMEOUT + 5)
    if thread.is_alive():
        raise asyncio.TimeoutError()
    if "error" in result:
        raise result["error"]
    return result["value"]


async def _access_status(geolocation):
    status = await geolocation.Geolocator.request_access_async()
    access = geolocation.GeolocationAccessStatus
    if status == access.ALLOWED:
        return ALLOWED
    if status == access.DENIED:
        return DENIED
    # Unspecified: Windows hasn't decided (e.g. the user hasn't answered); treat like denied
    return DENIED


async def _lookup(geolocation):
    status = await _access_status(geolocation)
    if status != ALLOWED:
        raise LocationUnavailable(status)
    locator = geolocation.Geolocator()
    if locator.location_status == geolocation.PositionStatus.DISABLED:
        raise LocationUnavailable(OFF)
    try:
        found = await locator.get_geoposition_async()
    except OSError as e:
        if _is_access_denied(e):
            raise LocationUnavailable(DENIED)
        raise LocationUnavailable(NO_POSITION, str(e))
    coordinate = found.coordinate
    point = coordinate.point.position
    accuracy = getattr(coordinate, "accuracy", None)
    return float(point.latitude), float(point.longitude), accuracy


def access_status():
    """ALLOWED, DENIED or UNAVAILABLE, without looking up a position. For the layout editor."""
    try:
        geolocation = _geolocation()
        status = _run(lambda: _access_status(geolocation))
        if status == ALLOWED and geolocation.Geolocator().location_status == geolocation.PositionStatus.DISABLED:
            return OFF
        return status
    except LocationUnavailable as e:
        return e.reason
    except Exception as e:
        log.debug(f"Could not ask Windows about location access: {e}")
        return UNAVAILABLE


def get_position(refresh=False):
    """(lat, lon, accuracy in metres or None) from Windows Location; raises LocationUnavailable.

    A position is reused for 15 minutes, and after a failure Windows isn't asked again for 2 minutes."""
    global _position, _position_time, _failure, _failure_time
    with _lock:
        now = time.monotonic()
        if not refresh:
            if _position and now - _position_time < POSITION_MAX_AGE:
                return _position
            if _failure and now - _failure_time < FAILURE_RETRY:
                raise _failure
        try:
            geolocation = _geolocation()
            position = _run(lambda: _lookup(geolocation))
        except LocationUnavailable as e:
            failure = e
        except asyncio.TimeoutError:
            failure = LocationUnavailable(NO_POSITION, f"no answer in {LOOKUP_TIMEOUT} seconds")
        except Exception as e:
            failure = LocationUnavailable(DENIED) if _is_access_denied(e) else LocationUnavailable(NO_POSITION, str(e))
        else:
            _position, _position_time = position, now
            _failure = None
            log.info(f"Windows Location: {position[0]:.3f}, {position[1]:.3f}")
            return position
        if _failure is None or _failure.reason != failure.reason:
            log.warning(f"Windows Location not used: {failure}")
        _failure, _failure_time = failure, now
        # A recent position is still better than none (e.g. Wi-Fi dropped for a moment)
        if failure.reason == NO_POSITION and _position and now - _position_time < 4 * POSITION_MAX_AGE:
            return _position
        raise failure


def describe():
    """One line about Windows Location for --diagnose and the layout editor."""
    try:
        lat, lon, accuracy = get_position(refresh=True)
    except LocationUnavailable as e:
        return f"not available: {e}"
    near = f" (within about {accuracy:.0f} m)" if accuracy else ""
    return f"on, this computer is near {lat:.2f}, {lon:.2f}{near}"


def open_settings():
    """Open Windows Settings at the Location page."""
    import os
    try:
        os.startfile(SETTINGS_URI)
    except Exception as e:
        log.warning(f"Could not open Windows location settings: {e}")


def _reset_for_tests():
    global _position, _position_time, _failure, _failure_time
    _position, _position_time, _failure, _failure_time = None, 0.0, None, 0.0
