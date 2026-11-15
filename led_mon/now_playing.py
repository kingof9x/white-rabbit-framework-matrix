# The "Now playing" app: the cover of whatever is playing (Spotify, a browser, the Media Player app),
# the song name scrolling along the top and a small visualizer underneath. Put it on both panels:
# the cover is 18 x 18, so each panel shows its half.
#
# What is playing comes from Windows' media controls (the same ones as the volume pop-up), read
# through WinRT. Elsewhere, or when nothing is playing, the app follows its "When nothing is
# playing" setting.

# Built In Dependencies
import io
import time
import asyncio
import logging
import threading
from dataclasses import dataclass

# Internal Dependencies
from led_mon import platform_utils
from led_mon.effects import text_columns

# External Dependencies
import numpy as np

log = logging.getLogger(__name__)

COVER = 18
WIDTH = 9
HEIGHT = 34
# How often to ask Windows what is playing, and how long to keep asking after the last draw
POLL_SECONDS = 1.0
IDLE_STOP_SECONDS = 10.0
# Visualizer bars at the bottom, and the song name along the top
BARS_ROWS = 7
TITLE_ROWS = 5
TITLE_SPEED = 12.0   # pixels per second
# The cover sits under the title; with drift it bobs a row up and down
COVER_TOP = 8

WHEN_IDLE = ("skip", "last", "clock")
WHEN_IDLE_LABELS = {"skip": "Show my other apps", "last": "Keep the last cover", "clock": "Show the clock"}


@dataclass
class Media:
    title: str = ""
    artist: str = ""
    playing: bool = False
    cover: object = None   # 18 x 18 floats from 0 to 1, or None
    app: str = ""          # which app Windows says is playing, e.g. Spotify.exe
    error: str = ""        # why nothing could be read, when it failed

    @property
    def song(self):
        return f"{self.title} - {self.artist}" if self.artist and self.title else (self.title or self.artist)


def cover_from_image(image):
    """An 18 x 18 greyscale cover from a PIL image, stretched so the darkest part is off and the lightest full."""
    from PIL import Image, ImageOps
    grey = ImageOps.fit(image.convert("L"), (COVER, COVER), Image.LANCZOS)
    cover = np.asarray(grey, dtype=float) / 255.0
    low, high = np.percentile(cover, 5), np.percentile(cover, 98)
    if high - low < 0.05:
        return np.clip(cover, 0, 1)
    return np.clip((cover - low) / (high - low), 0, 1)


def cover_from_bytes(data):
    try:
        from PIL import Image
        return cover_from_image(Image.open(io.BytesIO(data)))
    except Exception as e:
        log.debug(f"Now playing: could not read the cover: {e}")
        return None


# ---- Windows media controls ----

def _pick_session(sessions, current, is_playing):
    """The session to show: the one Windows calls current if it is playing, otherwise any that is
    playing (a paused browser tab can be "current" while Spotify plays), otherwise the current one."""
    if current is not None and is_playing(current):
        return current
    for session in sessions:
        if is_playing(session):
            return session
    return current if current is not None else (sessions[0] if sessions else None)


async def _read_windows():
    from winrt.windows.media.control import (GlobalSystemMediaTransportControlsSessionManager as Manager,
                                             GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status)
    manager = await Manager.request_async()

    def is_playing(session):
        try:
            info = session.get_playback_info()
            return info is not None and info.playback_status == Status.PLAYING
        except Exception:
            return False

    try:
        sessions = list(manager.get_sessions())
    except Exception:
        sessions = []
    session = _pick_session(sessions, manager.get_current_session(), is_playing)
    if session is None:
        return Media()
    props = await session.try_get_media_properties_async()
    media = Media(title=props.title or "", artist=props.artist or "", playing=is_playing(session),
                  app=getattr(session, "source_app_user_model_id", "") or "")
    media.thumbnail = props.thumbnail
    return media


def describe(reader=None):
    """One line on what Windows says is playing, for --diagnose."""
    reader = reader or (read_windows_media if platform_utils.IS_WINDOWS else None)
    if reader is None:
        return "only works on Windows"
    try:
        media = reader()
    except Exception as e:
        return f"could not ask Windows ({type(e).__name__}: {e})"
    if not (media.title or media.artist or media.app):
        return "Windows says nothing is playing"
    state = "playing" if media.playing else "paused or stopped"
    return f"{media.app or 'an app'} is {state}: {media.song or '(no song name)'}"


async def _read_thumbnail(thumbnail):
    from winrt.windows.storage.streams import DataReader, InputStreamOptions
    stream = await thumbnail.open_read_async()
    size = int(stream.size)
    if size <= 0 or size > 8 * 1024 * 1024:
        return None
    reader = DataReader(stream.get_input_stream_at(0))
    reader.input_stream_options = InputStreamOptions.READ_AHEAD
    await reader.load_async(size)
    data = bytearray(size)
    reader.read_bytes(data)
    return bytes(data)


def _run(coro_fn, timeout=5.0):
    import winrt.runtime
    try:
        winrt.runtime.init_apartment(winrt.runtime.ApartmentType.MULTI_THREADED)
    except Exception:
        pass
    return asyncio.run(asyncio.wait_for(coro_fn(), timeout))


def read_windows_media():
    return _run(_read_windows)


class Watcher:
    """Asks the system what is playing about once a second, while anything wants to know."""

    def __init__(self, reader=None, on_song=None):
        self.reader = reader or (read_windows_media if platform_utils.IS_WINDOWS else None)
        self.on_song = on_song
        self.lock = threading.Lock()
        self.media = Media()
        self.last_cover = None
        self.last_song = None
        self.wanted_until = 0.0
        self.thread = None
        self.polled = False
        self._cover_key = None
        self._last_error = None

    def want(self, now=None):
        """Keep watching for a while longer; starts the watching thread if needed."""
        now = time.monotonic() if now is None else now
        self.wanted_until = now + IDLE_STOP_SECONDS
        if self.reader is None:
            self.polled = True
            return
        if self.thread is None or not self.thread.is_alive():
            self.thread = threading.Thread(target=self._loop, name="now-playing", daemon=True)
            self.thread.start()

    def current(self):
        with self.lock:
            return self.media, self.last_cover

    def _loop(self):
        while time.monotonic() < self.wanted_until:
            self.poll()
            time.sleep(POLL_SECONDS)

    def poll(self):
        try:
            media = self.reader()
            if self._last_error is not None:
                log.info("Now playing: reading what is playing works again")
            self._last_error = None
        except Exception as e:
            error = f"{type(e).__name__}: {e}"
            # Once per kind of failure, where it shows in the log (debug lines don't)
            if error != self._last_error:
                log.warning(f"Now playing: could not ask Windows what is playing: {error}")
            self._last_error = error
            media = Media(error=error)
        key = (media.title, media.artist)
        if media.cover is None and getattr(media, "thumbnail", None) is not None:
            if key == self._cover_key:
                media.cover = self.media.cover
            else:
                try:
                    data = _run(lambda: _read_thumbnail(media.thumbnail))
                    media.cover = cover_from_bytes(data) if data else None
                except Exception as e:
                    log.debug(f"Now playing: no cover: {e}")
                self._cover_key = key
        song = media.song if media.playing else None
        seen = (media.app, media.playing, media.song)
        if not media.error and seen != getattr(self, "_last_seen", None):
            self._last_seen = seen
            if media.app or media.song:
                state = "playing" if media.playing else "paused or stopped"
                log.info(f"Now playing: {media.app or 'an app'} is {state}: {media.song or '(no song name)'}")
            else:
                log.info("Now playing: Windows says nothing is playing")
        with self.lock:
            self.media = media
            if media.cover is not None:
                self.last_cover = media.cover
            self.polled = True
        if song and song != self.last_song and self.on_song is not None:
            try:
                self.on_song(song)
            except Exception as e:
                log.debug(f"Now playing: song change notice failed: {e}")
        if song:
            self.last_song = song


watcher = Watcher()


def should_skip(args, now=None):
    """True when the app is set to skip and nothing is playing (once the first answer is in)."""
    if (args or {}).get("when_idle", "skip") != "skip":
        return False
    watcher.want(now)
    media, _ = watcher.current()
    return watcher.polled and not media.playing


# ---- Small visualizer: the playback sound, in 9 bands mirrored across the two panels ----

class Bars:
    def __init__(self):
        self.capture = None
        self.analyzer = None
        self.last_used = 0.0
        self.levels = np.zeros(WIDTH)
        self.last_time = None

    def get(self, now):
        self.last_used = now
        if self.capture is None:
            try:
                from led_mon.equalizer_files import audio, looks
                self.capture = audio.acquire("playback")
                self.analyzer = looks.Analyzer(WIDTH, "playback")
                threading.Thread(target=self._release_when_unused, name="now-playing-audio", daemon=True).start()
            except Exception as e:
                log.debug(f"Now playing: no sound for the visualizer: {e}")
                self.capture = False
        if not self.capture:
            return self.levels
        dt = 0.1 if self.last_time is None else max(0.01, min(0.5, now - self.last_time))
        self.last_time = now
        heights = self.analyzer.levels(self.capture.snapshot(), dt) / 34.0 * BARS_ROWS
        # Fall smoothly, jump up at once
        self.levels = np.maximum(heights, self.levels - 20.0 * dt)
        return self.levels

    def _release_when_unused(self):
        from led_mon.equalizer_files import audio
        while time.monotonic() - self.last_used < IDLE_STOP_SECONDS:
            time.sleep(2.0)
        capture, self.capture = self.capture, None
        if capture:
            audio.release(capture)


bars = Bars()


# ---- Drawing ----

def title_strip(text, now, width=COVER):
    """The song name scrolling right to left: FONT_H rows x width columns of 0/1."""
    strip = text_columns(text + "   ")
    w = strip.shape[1]
    if w == 0:
        return np.zeros((5, width), dtype=int)
    offset = int(now * TITLE_SPEED) % w
    tiled = np.hstack([strip] * (2 + width // w))
    return tiled[:, offset:offset + width]


def scene(media, cover, now, title=True, visualizer=True, drift=True, levels=None):
    """The whole picture across both panels: 34 rows x 18 columns, values 0 to 1."""
    canvas = np.zeros((HEIGHT, COVER))
    if cover is not None:
        dy = int(round(np.sin(now * 0.5))) if drift else 0
        top = COVER_TOP + dy
        canvas[top:top + COVER, :] = np.maximum(canvas[top:top + COVER, :], cover * 0.85)
    if title and media.song:
        canvas[0:TITLE_ROWS, :] = title_strip(media.song, now)
    if visualizer and levels is not None:
        for b in range(WIDTH):
            h = int(round(min(BARS_ROWS, levels[b])))
            for k in range(h):
                # Bass in the middle, treble towards the outside
                canvas[HEIGHT - 1 - k, WIDTH - 1 - b] = 0.7
                canvas[HEIGHT - 1 - k, WIDTH + b] = 0.7
    return canvas


def draw_clock(grid, fill_value, now_struct=None):
    from led_mon.drawing import draw_chars_list
    t = now_struct or time.localtime()
    hhmm = list(time.strftime("%H%M", t))
    draw_chars_list(grid, hhmm[:2] + ["horiz_colon"] + hhmm[2:], fill_value, 0)


def draw_now_playing(grid, fill_value, side="left", title=True, visualizer=True, drift=True,
                     when_idle="skip", now=None):
    now = time.monotonic() if now is None else now
    watcher.want(now)
    media, last_cover = watcher.current()
    if not media.playing:
        if when_idle == "clock":
            draw_clock(grid, fill_value)
            return
        if when_idle == "last" and last_cover is not None:
            media = Media(title=media.title, artist=media.artist, cover=media.cover if media.cover is not None else last_cover)
        elif media.cover is None and not media.song:
            # "skip" with this as the only app here, or nothing to keep: show the clock
            draw_clock(grid, fill_value)
            return
    cover = media.cover if media.cover is not None else (last_cover if media.playing else None)
    levels = bars.get(now) if visualizer and media.playing else None
    canvas = scene(media, cover, now, title, visualizer, drift, levels)
    half = canvas[:, :WIDTH] if side == "left" else canvas[:, WIDTH:]
    grid[:, :] = np.maximum(grid, np.rint(half.T * fill_value).astype(int))


# ---- A made-up song for the layout editor's preview, when nothing is playing ----

def demo_cover():
    """A crescent moon over a hill: not a real album."""
    y, x = np.mgrid[0:COVER, 0:COVER].astype(float)
    moon = (np.hypot(x - 6.0, y - 7.0) < 5.5) & ~(np.hypot(x - 8.5, y - 5.5) < 4.5)
    hill = y > 13 + 2.5 * np.cos((x - 12) / 4.0)
    cover = np.where(moon, 0.95, 0.0) + np.where(hill, 0.45, 0.0)
    for yy, xx in ((2, 14), (5, 16), (1, 9), (9, 15)):
        cover[yy, xx] = 1.0
    return np.clip(cover, 0, 1)


DEMO = Media(title="Preview song", artist="White Rabbit", playing=True)


def draw_preview(grid, fill_value, side="left", title=True, visualizer=True, drift=True, when_idle="skip", now=None):
    """The editor preview: what is playing if anything is, otherwise a made-up song with made-up bars."""
    now = time.monotonic() if now is None else now
    watcher.want(now)
    media, _ = watcher.current()
    if media.playing:
        draw_now_playing(grid, fill_value, side, title, visualizer, drift, when_idle, now)
        return
    levels = BARS_ROWS * (0.35 + 0.3 * np.sin(now * 3.0 + np.arange(WIDTH) * 0.9)) * np.linspace(1.0, 0.5, WIDTH)
    canvas = scene(DEMO, _demo_cover, now, title, visualizer, drift, levels)
    half = canvas[:, :WIDTH] if side == "left" else canvas[:, WIDTH:]
    grid[:, :] = np.maximum(grid, np.rint(half.T * fill_value).astype(int))


_demo_cover = demo_cover()
