# Listening to sound for the audio visualizer: what the laptop plays, or a microphone.
#
# Windows: WASAPI through the soundcard package. "playback" records the default output (loopback),
# following it when it changes, e.g. from speakers to headphones. Linux: sounddevice (PortAudio).
#
# One recording is shared by everything in this program that listens to the same thing, so the two
# panels (and the window's preview) see exactly the same sound. acquire() starts or joins one,
# release() leaves it, and the last one to leave stops it.

# Built In Dependencies
import time
import logging
import threading

# External Dependencies
import numpy as np

# Internal Dependencies
from led_mon.platform_utils import IS_WINDOWS
from led_mon.equalizer_files.looks import FFT_SIZE, SAMPLE_RATE

log = logging.getLogger(__name__)

# soundcard initialises COM when it is first imported, and fails ("Error 0x100000001") if that import
# happens on a thread where COM is already set up. So import it here, when the app loads (the plugins
# load on the main thread), rather than inside a recording thread.
sc = None
_soundcard_error = None
if IS_WINDOWS:
    try:
        import soundcard as sc
    except Exception as e:  # e.g. no audio service
        _soundcard_error = e


def _soundcard():
    if sc is None:
        raise RuntimeError(f"Windows audio isn't available: {_soundcard_error}")
    return sc

CHUNK_SIZE = 1024
# Windows: how often to look for a new default output device (or a microphone plugged in)
DEVICE_CHECK_SEC = 3
# Sound older than this counts as silence (WASAPI loopback delivers nothing while nothing plays)
STALE_AUDIO_SEC = 0.25
DEFAULT_MICROPHONE = "Default microphone"


def _com_init_mta():
    # soundcard uses COM; each thread that touches it needs COM initialised
    try:
        import ctypes
        ctypes.windll.ole32.CoInitializeEx(None, 0x0)  # COINIT_MULTITHREADED
    except Exception:
        pass


def list_microphones():
    """Names of the microphones (and other recording inputs) this computer has."""
    names = []
    try:
        if IS_WINDOWS:
            sc = _soundcard()
            _com_init_mta()
            names = [m.name for m in sc.all_microphones(include_loopback=False)]
        else:
            import sounddevice as sd
            names = [d["name"] for d in sd.query_devices()
                     if int(d.get("max_input_channels", 0)) > 0 and "monitor" not in str(d.get("name", "")).lower()]
    except Exception as e:
        log.warning(f"Could not list the microphones: {e}")
    unique = []
    for name in names:
        if name and name not in unique:
            unique.append(name)
    return unique


def match_name(wanted, names):
    """The device in names that wanted means: the same name, else the first that contains it. None if none."""
    if not wanted:
        return None
    for name in names:
        if name == wanted:
            return name
    lower = wanted.lower()
    for name in names:
        if lower in name.lower():
            return name
    return None


class Capture:
    """Records sound into a buffer holding the last FFT_SIZE samples (stereo)."""

    def __init__(self, input_mode="playback", input_device="", on_device_change=None):
        self.input_mode = input_mode
        self.input_device = input_device or ""
        self.on_device_change = on_device_change
        self.lock = threading.Lock()
        self.buffer = np.zeros((FFT_SIZE, 2), dtype=np.float32)
        self.updated = 0.0
        self.stopped = threading.Event()
        self.refs = 0
        self.device_name = None
        self.problem = None
        self.generation = 0
        self.current_id = None
        self._stream = None

    # Shared by the recording threads
    def push(self, data):
        data = np.asarray(data, dtype=np.float32)
        if data.ndim == 1:
            data = data[:, None]
        if data.shape[1] == 1:
            data = np.repeat(data, 2, axis=1)
        data = data[-FFT_SIZE:, :2]
        n = len(data)
        if n == 0:
            return
        with self.lock:
            self.buffer = np.concatenate([self.buffer[n:], data])
            self.updated = time.monotonic()

    def snapshot(self):
        """The latest samples, all zero when nothing new has arrived for a moment."""
        with self.lock:
            if time.monotonic() - self.updated > STALE_AUDIO_SEC:
                return np.zeros((FFT_SIZE, 2), dtype=np.float32)
            return self.buffer.copy()

    def start(self):
        if IS_WINDOWS:
            threading.Thread(target=self._supervise, daemon=True, name="audio-visualizer-capture").start()
        else:
            self._start_sounddevice()

    def stop(self):
        self.stopped.set()
        stream, self._stream = self._stream, None
        if stream is not None:
            try:
                stream.stop()
                stream.close()
            except Exception as e:
                log.debug(f"Closing the audio stream: {e}")

    # Windows
    def _resolve_source(self):
        sc = _soundcard()
        if self.input_mode == "playback":
            speaker = sc.default_speaker()
            return sc.get_microphone(speaker.id, include_loopback=True), speaker.name
        if self.input_device:
            mics = sc.all_microphones(include_loopback=False)
            name = match_name(self.input_device, [m.name for m in mics])
            if name is not None:
                mic = next(m for m in mics if m.name == name)
                return mic, mic.name
            if self.problem is None:
                log.warning(f"Microphone '{self.input_device}' was not found, using the default microphone")
            self.problem = f"{self.input_device} wasn't found, so the default microphone is used"
        else:
            self.problem = None
        mic = sc.default_microphone()
        return mic, mic.name

    def _supervise(self):
        _com_init_mta()
        error_logged = False
        while not self.stopped.is_set():
            try:
                source, name = self._resolve_source()
                if source.id != self.current_id:
                    first = self.current_id is None and self.device_name is None
                    self.current_id = source.id
                    self.device_name = name
                    self.generation += 1
                    log.info(f"Audio visualizer listening to {name}")
                    threading.Thread(target=self._record, args=(source, self.generation), daemon=True).start()
                    if not first and self.input_mode == "playback" and self.on_device_change:
                        self.on_device_change(name)
                error_logged = False
            except Exception as e:
                if not error_logged:
                    error_logged = True
                    log.warning(f"Audio visualizer could not open the {self.input_mode} audio device: {e}")
                self.problem = f"Couldn't open the sound device: {e}"
                self.current_id = None
            self.stopped.wait(DEVICE_CHECK_SEC)

    def _record(self, source, generation):
        # Recording can block while nothing plays, so a device change starts a new thread
        # and an outdated one ends when it next wakes up.
        _com_init_mta()
        try:
            with source.recorder(samplerate=SAMPLE_RATE, blocksize=CHUNK_SIZE) as recorder:
                while not self.stopped.is_set() and generation == self.generation:
                    data = recorder.record(numframes=CHUNK_SIZE)
                    if generation != self.generation:
                        break
                    self.push(data)
        except Exception as e:
            log.warning(f"Audio visualizer recording stopped: {e}")
            if generation == self.generation:
                # Let the supervisor open the device again on its next check
                self.current_id = None

    # Linux
    def _start_sounddevice(self):
        import sounddevice as sd
        device = None
        if self.input_mode == "microphone":
            device = match_name(self.input_device, list_microphones()) if self.input_device else None
            if self.input_device and device is None:
                self.problem = f"{self.input_device} wasn't found, so the default microphone is used"
        try:
            info = sd.query_devices(device, "input")
            channels = 2 if int(info.get("max_input_channels", 1)) >= 2 else 1
            self.device_name = info.get("name")
        except Exception:
            channels = 1

        def callback(indata, frames, time_info, status):
            self.push(indata.copy())

        def open_stream(dev, ch):
            stream = sd.InputStream(samplerate=SAMPLE_RATE, channels=ch, blocksize=CHUNK_SIZE,
                                    callback=callback, device=dev)
            stream.start()
            return stream
        try:
            self._stream = open_stream(device, channels)
        except Exception as e:
            if device is None:
                self.problem = f"Couldn't open the sound device: {e}"
                raise
            log.warning(f"Could not open '{device}', using the default input: {e}")
            self._stream = open_stream(None, 1)
        log.info(f"Audio visualizer listening to {self.device_name or 'the default input'}")


_captures = {}
_captures_lock = threading.Lock()


def acquire(input_mode="playback", input_device="", on_device_change=None):
    """Start (or join) the recording of this input. Call release() with it when done."""
    key = (input_mode, (input_device or "") if input_mode == "microphone" else "")
    with _captures_lock:
        capture = _captures.get(key)
        if capture is None:
            capture = Capture(input_mode, key[1], on_device_change)
            capture.start()
            _captures[key] = capture
        capture.refs += 1
        return capture


def release(capture):
    with _captures_lock:
        capture.refs -= 1
        if capture.refs > 0:
            return
        for key, value in list(_captures.items()):
            if value is capture:
                del _captures[key]
    capture.stop()
