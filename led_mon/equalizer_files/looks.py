# The audio visualizer's looks: turning a slice of sound into a 9 x 34 picture for one panel.
#
# Used by the audio visualizer app on the panels, by music mode, and by the previews in the window,
# so they all look the same. Needs only numpy.
#
# Settings (the app's args in config.yaml, and "music" in settings.json, use the same keys):
#   input-mode         playback (what the laptop plays) | microphone
#   input-device       microphone name, blank for the default microphone
#   style              one of STYLES below
#   span-panels        one spectrum across both panels: 18 bars, bass on the left panel
#   peak-dots          a dot above each bar that falls slowly
#   shaded             bars get brighter towards their tips
#   flip               upside down
#   fall-speed         slow | normal | fast: how quickly the bars drop
#   sensitivity        1-10
#   auto-sensitivity   turn the sensitivity up or down to suit how loud the sound is

# Built In Dependencies
import math

# External Dependencies
import numpy as np

WIDTH = 9
HEIGHT = 34
MAX_LEVEL = 34
SAMPLE_RATE = 48000
# Samples analysed per frame (85 ms): enough to tell the lowest bass notes apart
FFT_SIZE = 4096
# Band edges: nine octaves centred on 31.5 Hz ... 8 kHz, like the original equalizer
LOW_HZ = 22.3
HIGH_HZ = 11314.0

# (key, label, description)
STYLES = [
    ("classic", "Classic", "Bars grow up and down from the middle row, like the original equalizer."),
    ("bars", "Spectrum analyzer", "Bars rise from the bottom of the panel: bass on the left, treble on the right."),
    ("centre", "Centre out", "Bass in the middle, treble at the edges, like a mirror. "
                             "Across both panels the bass is where the panels meet."),
    ("dots", "Dots", "One dot where the top of each bar would be."),
    ("waterfall", "Spectrogram", "The spectrum scrolls down the panel like a waterfall; brighter means louder."),
    ("meter", "Level meter", "Loudness meters for the left and right channels, with peak marks. "
                             "Across both panels each panel is one channel."),
    ("wave", "Waveform", "The sound wave itself, like an oscilloscope."),
    ("mirror-wave", "Mirror waveform", "The sound wave mirrored around the middle column."),
    ("double-wave", "Double waveform", "Two waves side by side: the left and the right channel."),
    ("radial", "Radial bars", "Bars that point out from a circle in the middle, one direction per band. "
                              "The left panel is a mirror image of the right one."),
    ("radial-fan", "Radial fan", "Half a circle on each panel that fans out from the keyboard side: bass points up, "
                                 "treble points down. Together the panels look like one circle split by the keyboard."),
    ("radial-facing", "Radial facing", "A circle on each panel with the bass bars pointing towards the keyboard and "
                                       "the treble bars pointing outwards, so the two panels face each other."),
    ("rings", "Rings", "Circles around the middle, bass inside and treble outside; brighter means louder."),
    ("pulse", "Pulse", "A glow in the middle that swells with the beat."),
    ("sparkle", "Sparkle", "LEDs twinkle all over; the louder it gets, the more of them."),
]
STYLE_KEYS = [key for key, _, _ in STYLES]
STYLE_LABELS = {key: label for key, label, _ in STYLES}
STYLE_DESCRIPTIONS = {key: text for key, _, text in STYLES}
# Looks that show bars, where peak dots and shading mean something
BAR_STYLES = ("classic", "bars", "centre", "dots", "waterfall", "meter", "radial", "radial-fan", "radial-facing", "rings")

# Levels (out of 34) the bars drop per second
FALL_SPEEDS = {"slow": 30.0, "normal": 70.0, "fast": 160.0}
FALL_LABELS = {"slow": "Slow", "normal": "Normal", "fast": "Fast"}
PEAK_HOLD_SEC = 0.5

INPUT_MODES = ("playback", "microphone")
INPUT_LABELS = {"playback": "What the laptop is playing", "microphone": "A microphone"}

DEFAULTS = {
    "input-mode": "playback",
    "input-device": "",
    "style": "classic",
    "span-panels": False,
    "peak-dots": False,
    "shaded": False,
    "flip": False,
    "fall-speed": "normal",
    "sensitivity": 5,
    "auto-sensitivity": True,
}

# Loudness (dB) shown as an empty bar and as a full bar. A microphone hears music much more quietly
# than the sound card sends it out.
LEVEL_RANGE_DB = {"playback": (-60.0, 0.0), "microphone": (-80.0, -30.0)}
# dB per sensitivity step away from 5
SENSITIVITY_STEP_DB = 3.0
# Automatic sensitivity: never more than this much louder or quieter, and only turned up for sound
# at least this far above the bottom of the range (so the hiss of a quiet room isn't turned up)
AUTO_LIMIT_DB = 20.0
AUTO_MIN_SIGNAL_DB = 15.0
AUTO_DOWN_DB_PER_SEC = 25.0
AUTO_UP_DB_PER_SEC = 5.0
# Microphones: the bars show how far each band is above the room's background noise, which is the
# quietest each band has been over the last MIC_FLOOR_SECONDS. A microphone's own level varies a lot
# from one laptop or headset to another, so this works whatever the microphone's volume is.
MIC_FLOOR_SECONDS = 10.0
MIC_FLOOR_STEP = 0.1
MIC_FLOOR_PERCENTILE = 30
MIC_MARGIN_DB = 6.0
MIC_MIN_SPAN_DB = 12.0
MIC_FIXED_SPAN_DB = 30.0
MIC_PEAK_FALL_DB_PER_SEC = 1.5
# Bars shorter than this count as nothing; the sum of the bars at or below SILENCE_SUM counts as silence
DEFAULT_NOISE_GATE = {"playback": 1, "microphone": 2}
DEFAULT_SILENCE_SUM = {"playback": 2, "microphone": 8}
# Waveforms are stretched to the recent loudest sample, but no further than to this one (and start here).
# A laptop's microphone can peak at well under 0.001, so it may be stretched much further than playback;
# the room's noise is still never drawn, because silence (see above) draws a flat line.
WAVE_MIN_SCALE = {"playback": 0.002, "microphone": 0.00002}
WAVE_START_SCALE = {"playback": 0.05, "microphone": 0.001}


def _bool(value, default=False):
    if isinstance(value, str):
        text = value.strip().lower()
        if text in ("1", "true", "yes", "on"):
            return True
        if text in ("0", "false", "no", "off", ""):
            return False
        return default
    if value is None:
        return default
    return bool(value)


def normalize_input_mode(value):
    mode = str(value or "playback").strip().lower()
    if mode in ("playback", "monitor", "output"):
        return "playback"
    if mode in ("microphone", "mic", "input", "capture"):
        return "microphone"
    return "playback"


def normalize(raw=None):
    """Settings with every key filled in and every value made valid."""
    raw = dict(raw or {})
    s = dict(DEFAULTS)
    s["input-mode"] = normalize_input_mode(raw.get("input-mode"))
    device = raw.get("input-device")
    s["input-device"] = str(device).strip() if device else ""
    style = str(raw.get("style") or DEFAULTS["style"]).strip().lower()
    s["style"] = style if style in STYLE_KEYS else DEFAULTS["style"]
    for key in ("span-panels", "peak-dots", "shaded", "flip", "auto-sensitivity"):
        s[key] = _bool(raw.get(key), DEFAULTS[key])
    fall = str(raw.get("fall-speed") or DEFAULTS["fall-speed"]).strip().lower()
    s["fall-speed"] = fall if fall in FALL_SPEEDS else DEFAULTS["fall-speed"]
    try:
        s["sensitivity"] = max(1, min(10, int(round(float(raw.get("sensitivity", DEFAULTS["sensitivity"]))))))
    except (TypeError, ValueError):
        s["sensitivity"] = DEFAULTS["sensitivity"]
    # Older settings from the equalizer app
    for key in ("level-gain", "noise-gate-level", "silence-level-sum-threshold"):
        if raw.get(key) is not None:
            s[key] = raw[key]
    return s


def column_bands(style, span, side):
    """(number of bands to measure, the band shown in each of the panel's 9 columns)."""
    if style == "centre":
        if span:
            # Bass where the two panels meet, treble at the outer edges
            columns = [8 - c for c in range(WIDTH)] if side == "left" else list(range(WIDTH))
            return WIDTH, columns
        return 5, [abs(c - 4) for c in range(WIDTH)]
    if span:
        start = 0 if side == "left" else WIDTH
        return 2 * WIDTH, [start + c for c in range(WIDTH)]
    return WIDTH, list(range(WIDTH))


def mono_or_channel(samples, channel):
    """One channel of the samples (or both mixed, for channel None)."""
    samples = np.asarray(samples, dtype=np.float64)
    if samples.ndim == 1:
        return samples
    if samples.shape[1] == 0:
        return np.zeros(samples.shape[0])
    if channel is None:
        return samples.mean(axis=1)
    return samples[:, min(channel, samples.shape[1] - 1)]


class Analyzer:
    """Measures how loud each frequency band is, as a bar height from 0 to 34."""

    def __init__(self, n_bands, input_mode="playback", sensitivity=5, auto=True, level_gain=1.0,
                 noise_gate=None, sample_rate=SAMPLE_RATE, fft_size=FFT_SIZE):
        self.n_bands = n_bands
        self.fft_size = fft_size
        self.window = np.hanning(fft_size)
        self.window_rms = float(np.sqrt(np.mean(self.window ** 2)))
        freqs = np.fft.rfftfreq(fft_size, 1.0 / sample_rate)
        edges = np.geomspace(LOW_HZ, HIGH_HZ, n_bands + 1)
        self.bins = []
        for low, high in zip(edges[:-1], edges[1:]):
            idx = np.nonzero((freqs >= low) & (freqs < high))[0]
            if len(idx) == 0:
                # A band narrower than the FFT's resolution: use the nearest frequency
                idx = np.array([int(np.argmin(np.abs(freqs - math.sqrt(low * high))))])
            self.bins.append(idx)
        self.bottom_db, self.top_db = LEVEL_RANGE_DB.get(input_mode, LEVEL_RANGE_DB["playback"])
        try:
            gain_db = 20.0 * math.log10(max(0.01, float(level_gain)))
        except (TypeError, ValueError):
            gain_db = 0.0
        self.offset_db = (int(sensitivity) - 5) * SENSITIVITY_STEP_DB + gain_db
        self.auto = bool(auto)
        self.auto_db = 0.0
        self.microphone = input_mode == "microphone"
        self.sensitivity = int(sensitivity)
        self.gain_db = gain_db
        self.history = []
        self.history_wait = 0.0
        self.peak_above = MIC_MIN_SPAN_DB
        if noise_gate is None:
            noise_gate = DEFAULT_NOISE_GATE.get(input_mode, 1)
        self.noise_gate = max(0, min(MAX_LEVEL, int(noise_gate)))

    def band_db(self, samples):
        """Loudness of each band in dB (0 dB: a full-scale sine wave)."""
        x = mono_or_channel(samples, None)[-self.fft_size:]
        if len(x) < self.fft_size:
            x = np.concatenate([np.zeros(self.fft_size - len(x)), x])
        x = x - x.mean()
        power = np.abs(np.fft.rfft(x * self.window)) ** 2
        rms = np.array([math.sqrt(2.0 * power[idx].sum()) for idx in self.bins]) / (self.fft_size * self.window_rms)
        return 20.0 * np.log10(rms + 1e-10)

    def microphone_levels(self, samples, dt):
        db = self.band_db(samples) + self.gain_db
        if not self.history:
            self.history = [db]
        self.history_wait += dt
        if self.history_wait >= MIC_FLOOR_STEP:
            self.history_wait = 0.0
            self.history.append(db)
            del self.history[:-int(MIC_FLOOR_SECONDS / MIC_FLOOR_STEP)]
        # A low percentile rather than the minimum: a band of noise flickers by several dB from frame to frame
        floor = np.percentile(self.history, MIC_FLOOR_PERCENTILE, axis=0)
        above = db - floor - MIC_MARGIN_DB
        if self.auto:
            # The loudest recent sound fills the panel, but background noise is never stretched to fill it
            self.peak_above = max(float(above.max()), self.peak_above - MIC_PEAK_FALL_DB_PER_SEC * dt,
                                  MIC_MIN_SPAN_DB)
            span = self.peak_above * 1.1
        else:
            span = MIC_FIXED_SPAN_DB
        # Each sensitivity step above 5 makes the bars about 15% taller
        span *= 0.87 ** (self.sensitivity - 5)
        levels = np.clip(above / span * MAX_LEVEL, 0, MAX_LEVEL)
        levels[levels < self.noise_gate] = 0
        return levels

    def levels(self, samples, dt=0.03):
        if self.microphone:
            return self.microphone_levels(samples, dt)
        db = self.band_db(samples) + self.offset_db
        if self.auto:
            loudest = float(db.max())
            # Gain that puts the loudest band just under the top
            target = max(-AUTO_LIMIT_DB, min(AUTO_LIMIT_DB, (self.top_db - 4.0) - loudest))
            if loudest < self.bottom_db + AUTO_MIN_SIGNAL_DB:
                # Silence or the hiss of a quiet room: don't turn it up
                target = min(target, self.auto_db)
            if target < self.auto_db:
                self.auto_db = max(target, self.auto_db - AUTO_DOWN_DB_PER_SEC * dt)
            else:
                self.auto_db = min(target, self.auto_db + AUTO_UP_DB_PER_SEC * dt)
            db = db + self.auto_db
        levels = (db - self.bottom_db) / (self.top_db - self.bottom_db) * MAX_LEVEL
        levels = np.clip(levels, 0, MAX_LEVEL)
        levels[levels < self.noise_gate] = 0
        return levels


class Visualizer:
    """One panel's audio visualizer: give it the latest sound, get back the next picture."""

    def __init__(self, settings=None, side="left", seed=None):
        self.settings = s = normalize(settings)
        self.side = "right" if side == "right" else "left"
        self.style = s["style"]
        self.span = s["span-panels"]
        self.n_bands, self.columns = column_bands(self.style, self.span, self.side)
        noise_gate = s.get("noise-gate-level")
        self.analyzer = Analyzer(self.n_bands, s["input-mode"], s["sensitivity"], s["auto-sensitivity"],
                                 level_gain=s.get("level-gain", 1.0),
                                 noise_gate=None if noise_gate is None else noise_gate)
        try:
            self.silence_sum = int(s.get("silence-level-sum-threshold", DEFAULT_SILENCE_SUM[s["input-mode"]]))
        except (TypeError, ValueError):
            self.silence_sum = DEFAULT_SILENCE_SUM[s["input-mode"]]
        self.fall = FALL_SPEEDS[s["fall-speed"]]
        # Across both panels the spectrum is one sound (both channels mixed); otherwise the left panel
        # shows the left channel and the right panel the right one
        self.channel = None if self.span else (0 if self.side == "left" else 1)
        self.shown = np.zeros(self.n_bands)
        self.peaks = np.zeros(self.n_bands)
        self.peak_time = np.zeros(self.n_bands)
        self.trail = np.zeros((WIDTH, HEIGHT))
        self.trail_wait = 0.0
        self.sparkle = np.zeros((WIDTH, HEIGHT))
        self.wave_min_scale = WAVE_MIN_SCALE.get(s["input-mode"], WAVE_MIN_SCALE["playback"])
        self.wave_scale = WAVE_START_SCALE.get(s["input-mode"], WAVE_START_SCALE["playback"])
        self.pulse = 0.0
        self.clock = 0.0
        self.rng = np.random.default_rng(seed)
        self.loudness = 0
        self.samples = np.zeros((FFT_SIZE, 2))
        self.meter = np.zeros(2)
        self.meter_auto = 0.0
        self.meter_peak = np.zeros(2)
        self.meter_peak_time = np.zeros(2)

    def is_silent(self):
        """True when the last frame heard (next to) nothing."""
        return self.loudness <= self.silence_sum

    def frame(self, samples, dt=0.03):
        """The next picture: a 9 x 34 array of brightness from 0.0 to 1.0."""
        dt = max(0.001, float(dt))
        self.clock += dt
        self.samples = samples
        chunk = mono_or_channel(samples, self.channel)
        levels = self.analyzer.levels(chunk, dt)
        self.loudness = int(levels.sum())
        # Bars jump up at once and fall at the chosen speed
        self.shown = np.maximum(levels, self.shown - self.fall * dt)
        rising = self.shown >= self.peaks
        self.peaks = np.where(rising, self.shown, self.peaks)
        self.peak_time = np.where(rising, self.clock, self.peak_time)
        falling = (self.clock - self.peak_time) > PEAK_HOLD_SEC
        self.peaks = np.where(falling, np.maximum(self.shown, self.peaks - 0.5 * self.fall * dt), self.peaks)

        draw = getattr(self, "_draw_" + self.style.replace("-", "_"))
        grid = draw(chunk, dt)
        if self.settings["flip"]:
            grid = grid[:, ::-1]
        return np.clip(grid, 0.0, 1.0)

    # Bar heights for the panel's 9 columns
    def _column_levels(self, values):
        return [float(values[b]) for b in self.columns]

    def _shade(self, length, distance):
        if not self.settings["shaded"] or length <= 0:
            return 1.0
        return 0.3 + 0.7 * min(1.0, (distance + 1) / length)

    def _draw_bars(self, chunk, dt, from_bottom=True):
        grid = np.zeros((WIDTH, HEIGHT))
        peaks = self._column_levels(self.peaks)
        for col, value in enumerate(self._column_levels(self.shown)):
            length = int(round(value))
            for i in range(length):
                grid[col, HEIGHT - 1 - i] = self._shade(length, i)
            if self.settings["peak-dots"]:
                peak = int(round(peaks[col]))
                if peak > 0:
                    grid[col, HEIGHT - min(HEIGHT, peak + (1 if peak > length else 0))] = 1.0
        return grid

    def _draw_centre(self, chunk, dt):
        return self._draw_bars(chunk, dt)

    def _draw_classic(self, chunk, dt):
        grid = np.zeros((WIDTH, HEIGHT))
        middle = HEIGHT // 2
        peaks = self._column_levels(self.peaks)
        for col, value in enumerate(self._column_levels(self.shown)):
            length = int(round(value))
            above = length // 2
            below = length - above
            half = max(1, below)
            for i in range(below):
                grid[col, middle - 1 - i] = self._shade(half, i)
            for i in range(above):
                grid[col, middle + i] = self._shade(half, i)
            if self.settings["peak-dots"]:
                peak = int(round(peaks[col]))
                if peak > 1:
                    reach = (peak + 1) // 2 + (1 if peak > length else 0)
                    grid[col, max(0, middle - reach)] = 1.0
                    grid[col, min(HEIGHT - 1, middle + reach - 1)] = 1.0
        return grid

    def _draw_dots(self, chunk, dt):
        grid = np.zeros((WIDTH, HEIGHT))
        peaks = self._column_levels(self.peaks)
        for col, value in enumerate(self._column_levels(self.shown)):
            length = int(round(value))
            if length > 0:
                grid[col, HEIGHT - length] = 1.0
                if length > 1:
                    grid[col, HEIGHT - length + 1] = 0.35
            if self.settings["peak-dots"]:
                peak = int(round(peaks[col]))
                if peak > length + 1:
                    grid[col, HEIGHT - peak] = 0.6
        return grid

    def _draw_waterfall(self, chunk, dt):
        # A new row at the top about 20 times a second; older rows move down and fade
        self.trail_wait += dt
        if self.trail_wait >= 0.05:
            self.trail_wait = 0.0
            self.trail[:, 1:] = self.trail[:, :-1] * 0.97
            row = np.array(self._column_levels(self.shown)) / MAX_LEVEL
            if self.settings["shaded"]:
                row = row ** 1.6
            self.trail[:, 0] = np.where(row > 0.04, np.maximum(row, 0.08), 0.0)
        grid = self.trail.copy()
        if self.settings["peak-dots"]:
            # The loudest band in the newest row stands out
            if grid[:, 0].max() > 0:
                grid[int(np.argmax(grid[:, 0])), 0] = 1.0
        return grid

    def _draw_wave(self, chunk, dt):
        grid = np.zeros((WIDTH, HEIGHT))
        recent = np.asarray(chunk[-1024:], dtype=np.float64)
        if len(recent) < HEIGHT:
            return grid
        recent = recent - recent.mean()
        peak = float(np.abs(recent).max())
        # Follow the volume so quiet and loud songs both fill the panel
        self._update_wave_scale(chunk, dt)
        if self.loudness <= self.silence_sum:
            peak = 0.0
        points = recent[np.linspace(0, len(recent) - 1, HEIGHT).astype(int)] / self.wave_scale
        cols = np.clip(np.round(4 + points * 4), 0, WIDTH - 1).astype(int)
        if peak == 0.0:
            cols[:] = 4
        previous = cols[0]
        for row, col in enumerate(cols):
            low, high = sorted((previous, col))
            grid[low:high + 1, row] = 0.5
            grid[col, row] = 1.0
            previous = col
        if self.settings["shaded"]:
            grid[4, :] = np.maximum(grid[4, :], 0.15)
        return grid

    def _channel_db(self, channel):
        x = mono_or_channel(self.samples, channel)[-2048:]
        rms = float(np.sqrt(np.mean(np.square(x)))) if len(x) else 0.0
        return 20.0 * math.log10(rms) if rms > 0 else -120.0

    def _draw_meter(self, chunk, dt):
        # Loudness of each channel: -60 dB empty, 0 dB full (moved by the sensitivity like the bars)
        bottom, top = self.analyzer.bottom_db, self.analyzer.top_db
        channels = [0 if self.side == "left" else 1] if self.span else [0, 1]
        loud = [self._channel_db(channel) + self.analyzer.offset_db for channel in channels]
        if self.analyzer.auto:
            # Like the bars' automatic sensitivity: the loudest moments reach about 85% of the meter
            target = max(-AUTO_LIMIT_DB, min(AUTO_LIMIT_DB, (top - 0.15 * (top - bottom)) - max(loud)))
            if max(loud) < bottom + AUTO_MIN_SIGNAL_DB:
                target = min(target, self.meter_auto)
            rate = AUTO_DOWN_DB_PER_SEC if target < self.meter_auto else AUTO_UP_DB_PER_SEC
            step = rate * dt
            self.meter_auto = max(self.meter_auto - step, min(self.meter_auto + step, target))
            loud = [db + self.meter_auto for db in loud]
        for i, db in enumerate(loud):
            value = max(0.0, min(1.0, (db - bottom) / (top - bottom))) * HEIGHT
            if self.analyzer.microphone:
                # A microphone is measured against the room's noise, like the bars
                value = min(1.0, 1.4 * float(np.sqrt(np.mean((self.shown / MAX_LEVEL) ** 2)))) * HEIGHT
            if self.loudness <= self.silence_sum:
                value = 0.0
            self.meter[i] = max(value, self.meter[i] - self.fall * dt)
            if self.meter[i] >= self.meter_peak[i]:
                self.meter_peak[i], self.meter_peak_time[i] = self.meter[i], self.clock
            elif self.clock - self.meter_peak_time[i] > PEAK_HOLD_SEC * 2:
                self.meter_peak[i] = max(self.meter[i], self.meter_peak[i] - 0.4 * self.fall * dt)
        grid = np.zeros((WIDTH, HEIGHT))
        spans = [range(1, 8)] if self.span else [range(1, 4), range(5, 8)]
        for i, cols in enumerate(spans):
            length = int(round(self.meter[i]))
            for row in range(length):
                # Segments, like a hardware meter
                if row % 3 == 2:
                    continue
                shade = self._shade(length, row) if self.settings["shaded"] else (1.0 if row >= HEIGHT * 0.8 else 0.7)
                for col in cols:
                    grid[col, HEIGHT - 1 - row] = shade
            peak = int(round(self.meter_peak[i]))
            if peak > 0:
                for col in cols:
                    grid[col, HEIGHT - peak] = 1.0
        # Scale marks at the edges
        grid[0, HEIGHT - 1::-6] = np.maximum(grid[0, HEIGHT - 1::-6], 0.12)
        grid[WIDTH - 1, HEIGHT - 1::-6] = np.maximum(grid[WIDTH - 1, HEIGHT - 1::-6], 0.12)
        return grid

    def _wave_points(self, channel, count=HEIGHT):
        """count points of the recent sound wave, scaled to about -1 ... 1."""
        recent = mono_or_channel(self.samples, channel)[-1024:]
        recent = recent - recent.mean()
        peak = float(np.abs(recent).max()) if len(recent) else 0.0
        if self.loudness <= self.silence_sum or peak == 0.0:
            return np.zeros(count)
        return recent[np.linspace(0, len(recent) - 1, count).astype(int)] / max(self.wave_scale, 1e-6)

    def _update_wave_scale(self, chunk, dt):
        recent = np.asarray(chunk[-1024:], dtype=np.float64)
        peak = float(np.abs(recent - recent.mean()).max()) if len(recent) else 0.0
        self.wave_scale = max(peak, self.wave_scale * (1.0 - 0.8 * dt), self.wave_min_scale)

    def _draw_mirror_wave(self, chunk, dt):
        self._update_wave_scale(chunk, dt)
        grid = np.zeros((WIDTH, HEIGHT))
        points = np.abs(self._wave_points(self.channel))
        for row, amplitude in enumerate(points):
            reach = int(round(min(1.0, amplitude) * 4))
            for d in range(reach + 1):
                value = 1.0 if d == reach else (0.35 if self.settings["shaded"] else 0.6)
                grid[4 - d, row] = max(grid[4 - d, row], value)
                grid[4 + d, row] = max(grid[4 + d, row], value)
        return grid

    def _draw_double_wave(self, chunk, dt):
        self._update_wave_scale(chunk, dt)
        grid = np.zeros((WIDTH, HEIGHT))
        for channel, centre in ((0, 2), (1, 6)):
            points = self._wave_points(channel)
            cols = np.clip(np.round(centre + points * 2), centre - 2, centre + 2).astype(int)
            previous = cols[0]
            for row, col in enumerate(cols):
                low, high = sorted((previous, col))
                grid[low:high + 1, row] = np.maximum(grid[low:high + 1, row], 0.5)
                grid[col, row] = 1.0
                previous = col
        if self.settings["shaded"]:
            grid[4, :] = np.maximum(grid[4, :], 0.12)
        return grid

    # Positions on the panel measured so that a circle fills it: x and y both from -1 to 1
    def _polar(self):
        x = (np.arange(WIDTH, dtype=float)[:, None] - 4.0) / 4.5
        y = (np.arange(HEIGHT, dtype=float)[None, :] - 16.5) / 17.0
        return np.sqrt(x ** 2 + y ** 2), np.arctan2(y, x)

    def _draw_radial(self, chunk, dt):
        radius, angle = self._polar()
        # Bands go round the circle and back (mirrored), so it looks the same on both sides
        n = self.n_bands
        position = (angle / (2 * np.pi)) % 1.0
        folded = 1.0 - np.abs(2.0 * position - 1.0)
        band = np.clip((folded * n).astype(int), 0, n - 1)
        levels = self.shown / MAX_LEVEL
        inner = 0.18
        reach = inner + levels[band] * (1.0 - inner) * 1.15
        grid = np.where((radius >= inner) & (radius <= reach), 1.0, 0.0)
        if self.settings["shaded"]:
            grid = grid * np.clip(0.3 + 0.7 * (radius - inner) / np.maximum(reach - inner, 1e-6), 0, 1)
        if self.settings["peak-dots"]:
            peak_reach = inner + (self.peaks / MAX_LEVEL)[band] * (1.0 - inner) * 1.15
            grid = np.maximum(grid, np.where(np.abs(radius - peak_reach) < 0.05, 0.8, 0.0))
        # The circle in the middle glows with the bass
        bass = float(levels[0]) if n else 0.0
        grid = np.maximum(grid, np.where(np.abs(radius - inner * 0.6) < 0.08, 0.25 + 0.75 * bass, 0.0))
        if self.side == "left":
            # The two panels mirror each other: bass points outwards on both
            grid = grid[::-1, :]
        return grid

    def _radial_from(self, radius, band, inner, stretch, glow):
        """Bars out from inner to their level along each direction; band says which bar each LED is on."""
        levels = self.shown / MAX_LEVEL
        reach = inner + levels[band] * (1.0 - inner) * stretch
        grid = np.where((radius >= inner) & (radius <= reach), 1.0, 0.0)
        if self.settings["shaded"]:
            grid = grid * np.clip(0.3 + 0.7 * (radius - inner) / np.maximum(reach - inner, 1e-6), 0, 1)
        if self.settings["peak-dots"]:
            peak_reach = inner + (self.peaks / MAX_LEVEL)[band] * (1.0 - inner) * stretch
            grid = np.maximum(grid, np.where(np.abs(radius - peak_reach) < 0.05, 0.8, 0.0))
        # The middle glows with the bass
        bass = float(levels[0]) if self.n_bands else 0.0
        return np.maximum(grid, np.where(glow, 0.25 + 0.75 * bass, 0.0))

    def _keyboard_side(self, grid):
        # Drawn for the right panel, whose keyboard side is column 0; the left panel is its mirror image
        return grid[::-1, :] if self.side == "left" else grid

    def _draw_radial_fan(self, chunk, dt):
        # Half a circle centred just past the keyboard-side edge, reaching the outer edge
        x = (np.arange(WIDTH, dtype=float)[:, None] + 0.5) / 9.0
        y = (np.arange(HEIGHT, dtype=float)[None, :] - 16.5) / 17.0
        radius = np.sqrt(x ** 2 + y ** 2)
        # 0 = straight up, 1 = straight down
        direction = np.arctan2(x, -y) / np.pi
        band = np.clip((direction * self.n_bands).astype(int), 0, self.n_bands - 1)
        inner = 0.15
        return self._keyboard_side(self._radial_from(radius, band, inner, 1.1, radius < inner * 0.9))

    def _draw_radial_facing(self, chunk, dt):
        radius, angle = self._polar()
        # 0 = towards the keyboard (bass), 1 = outwards (treble), the same above and below
        direction = 1.0 - np.abs(angle) / np.pi
        band = np.clip((direction * self.n_bands).astype(int), 0, self.n_bands - 1)
        inner = 0.18
        return self._keyboard_side(self._radial_from(radius, band, inner, 1.15,
                                                     np.abs(radius - inner * 0.6) < 0.08))

    def _draw_rings(self, chunk, dt):
        radius, _ = self._polar()
        n = self.n_bands
        ring = np.clip((radius / 1.05 * n).astype(int), 0, n)
        levels = np.append(self.shown / MAX_LEVEL, 0.0)
        grid = levels[ring]
        if not self.settings["shaded"]:
            # Thin bright rings with gaps between them
            edge = (radius / 1.05 * n) % 1.0
            grid = np.where(edge < 0.6, grid, grid * 0.15)
        if self.settings["peak-dots"]:
            peaks = np.append(self.peaks / MAX_LEVEL, 0.0)[ring]
            grid = np.maximum(grid, np.where(peaks > 0.85, 1.0, 0.0) * (grid > 0))
        return np.where(grid > 0.05, grid, 0.0)

    def _draw_pulse(self, chunk, dt):
        bass = float(np.mean(self.shown[:max(1, self.n_bands // 3)])) / MAX_LEVEL
        overall = float(np.mean(self.shown)) / MAX_LEVEL
        target = min(1.0, 0.65 * bass + 0.5 * overall)
        # Swell at once, shrink smoothly
        self.pulse = max(target, self.pulse - (self.fall / MAX_LEVEL) * 0.5 * dt)
        x = (np.arange(WIDTH, dtype=float)[:, None] - 4.0) / 4.5
        y = (np.arange(HEIGHT, dtype=float)[None, :] - 16.5) / 17.0
        radius = np.sqrt(x ** 2 + y ** 2)
        size = 0.08 + 1.1 * self.pulse
        glow = np.clip(1.0 - radius / max(size, 1e-6), 0.0, 1.0)
        if self.settings["shaded"]:
            glow = glow ** 0.5
        grid = np.where(glow > 0, 0.15 + 0.85 * glow, 0.0) * (0.4 + 0.6 * self.pulse)
        if self.settings["peak-dots"] and self.pulse > 0.05:
            # A thin ring at the edge of the glow
            grid = np.maximum(grid, np.where(np.abs(radius - size) < 0.08, 1.0, 0.0))
        return grid if self.pulse > 0.02 else np.zeros((WIDTH, HEIGHT))

    def _draw_sparkle(self, chunk, dt):
        self.sparkle *= max(0.0, 1.0 - 4.0 * dt)
        overall = float(np.mean(self.shown)) / MAX_LEVEL
        count = int(round(overall * 40 * dt / 0.03))
        if count:
            cols = self.rng.integers(0, WIDTH, count)
            if self.settings["shaded"]:
                # Louder sparkles reach higher up the panel
                reach = max(1, int(HEIGHT * min(1.0, overall * 1.5)))
                rows = HEIGHT - 1 - self.rng.integers(0, reach, count)
            else:
                rows = self.rng.integers(0, HEIGHT, count)
            self.sparkle[cols, rows] = 1.0
        grid = self.sparkle.copy()
        if self.settings["peak-dots"]:
            for col, value in enumerate(self._column_levels(self.peaks)):
                peak = int(round(value))
                if peak > 0:
                    grid[col, HEIGHT - peak] = 1.0
        return grid


def to_leds(grid, foreground_value):
    """Brightness 0.0-1.0 to LED values, keeping faint LEDs just visible."""
    values = np.rint(np.asarray(grid, dtype=float) * max(1, int(foreground_value))).astype(int)
    values = np.where(np.asarray(grid) > 0.02, np.maximum(values, 1), 0)
    return np.clip(values, 0, 255)


def demo_samples(t, seconds=FFT_SIZE / SAMPLE_RATE, sample_rate=SAMPLE_RATE):
    """Made-up music for previews: a beat, a bass line and a few notes, as stereo samples ending at time t."""
    n = int(seconds * sample_rate)
    times = t - seconds + np.arange(n) / sample_rate
    beat = np.exp(-((times * 2.0) % 1.0) * 6.0)
    bass = 0.5 * beat * np.sin(2 * np.pi * (55 + 10 * math.sin(t * 0.7)) * times)
    notes = sum(0.12 * (0.6 + 0.4 * math.sin(t * k)) * np.sin(2 * np.pi * f * times)
                for k, f in ((1.3, 220), (0.9, 440), (1.7, 880), (2.3, 1760), (0.5, 3520)))
    hats = 0.03 * np.exp(-((times * 4.0) % 1.0) * 30.0) * np.sin(2 * np.pi * 7000 * times)
    left = bass + notes + hats
    right = bass + 0.8 * notes + 1.2 * hats
    return np.stack([left, right], axis=1)
