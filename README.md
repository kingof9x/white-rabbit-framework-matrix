# White Rabbit: Framework 16 LED Matrix app

**One Windows app for everything the Framework Laptop 16 LED Matrix can do.**
It runs in the system tray, turns the LED panels into a system monitor you
design yourself, and lets you play games and patterns, choose the startup
animation and update the firmware, all from the same window.

> **Status: first version, not tested on real hardware yet.** Everything below
> works against simulated panels and passes the automated builds. The testing
> checklist at the bottom tracks what has been confirmed on a real Framework 16.

### ⬇ Downloads

The newest version is on the [Releases](https://github.com/kingof9x/white-rabbit-framework-matrix/releases/latest) page:

- [`WhiteRabbit-setup.exe`](https://github.com/kingof9x/white-rabbit-framework-matrix/releases/latest/download/WhiteRabbit-setup.exe): installer (starts with Windows, Start menu shortcut)
- [`WhiteRabbit.exe`](https://github.com/kingof9x/white-rabbit-framework-matrix/releases/latest/download/WhiteRabbit.exe): portable app, nothing is installed
- [`ledmatrix.uf2`](https://github.com/kingof9x/white-rabbit-framework-matrix/releases/latest/download/ledmatrix.uf2): LED Matrix firmware with the startup animation setting and the extra Game of Life patterns (the app can flash it for you)

These links work once the first release has been published.

## Why one app?

Until now there were two separate apps for the LED Matrix:

| | What it does |
|---|---|
| [**led_mon**](https://github.com/kingof9x/led-matrix) | System monitor: CPU, GPU, memory, battery, disk, network, FPS, clock, equalizer and more on the panels, with a tray icon and a layout editor. |
| [**LED Matrix Control** (`ledmatrixgui.exe`)](https://github.com/kingof9x/inputmodule-rs) | Control app: patterns, games, startup animation, firmware version and firmware updates. |

On Windows only one program at a time can talk to an LED panel, so to change
the startup animation or play a game you had to quit the monitor, open the
control app, then switch back. White Rabbit owns the panels and shares them:
when you start a game or a pattern, the monitor steps aside on that panel only,
and comes back when you press *Stop* or *Back to system monitor*.

## Features

### Panels tab: the system monitor (from led_mon)

- **Layout editor** with a live preview: drag apps onto the left and right
  panel, top or bottom half, and set up several pages that take turns.
- **Half-panel apps:** CPU cores, CPU graph, GPU graph, memory + battery,
  disk, network, disk + network, temperatures, fans.
- **Whole-panel apps:** clock, weather, audio visualizer, FPS counter for
  games, picture.
- Brightness follows the laptop screen, or set it yourself.
- The panels turn off when you close the lid, when the screen turns off, or
  when the laptop goes to sleep, and come back when you open it. Turn this
  off with *Turn off when lid closes* in the tray menu.
- Runs in the tray and starts with Windows. Keyboard shortcuts
  (`Ctrl`+`Alt`+`Shift` + `I` / `N` / `F` / `U` / `M`) show the app names, skip to
  the next app, freeze or unfreeze app switching, and turn music mode on or
  off. Each one can be turned off on the Keyboard shortcuts tab.

![The Panels tab with the layout editor](docs/screenshots/white-rabbit-panels.png)

![Some of the monitor apps as they look on a panel](docs/screenshots/panel-apps.png)

*Some of the monitor apps, drawn by the app's own preview code.*

### Startup animation tab

Pick one animation to always show, a random one out of a set you choose, a
random one out of all of them, or none. The choice is saved on the panel itself,
so it works even when the app isn't running. Needs the White Rabbit firmware.

![The Startup animation tab](docs/screenshots/white-rabbit-startup.png)

### Games & patterns tab

Game of Life with 10 extra patterns (R-pentomino, Acorn, spaceships and more),
Snake and Ledris, the firmware's built-in patterns, text, a clock, a countdown
and a random equalizer.

![The Games & patterns tab](docs/screenshots/white-rabbit-games.png)

![The starting position of each extra Game of Life pattern](docs/screenshots/game-of-life-patterns.png)

*Where each extra Game of Life pattern starts on the 9 x 34 matrix.*

### Audio tab

An audio visualizer that moves with what the laptop is playing, or with what a
microphone hears (pick any microphone Windows has).

- **Music mode** shows the visualizer on both panels until you turn it off.
  Turn it on here, with *Music mode* in the tray menu, or with
  `Ctrl`+`Alt`+`Shift`+`M`. It uses the look last picked on this tab.
- **15 looks:** classic, spectrum analyzer, centre out, dots, spectrogram,
  level meter, waveform, mirror waveform, double waveform, radial bars,
  radial fan, radial facing, rings, pulse and sparkle. On the radial looks the
  left panel is a mirror image of the right one.
- **Options:** one 18-bar spectrum across both panels, peak dots, shading,
  upside down, falling speed, sensitivity, and automatic sensitivity.
- A live preview of both panels and a sound check, which show whether the
  microphone (or the music) is being heard.

The same looks and options are on the *Audio visualizer* app in the layout
editor, so it can also take turns with the other apps.
### Math tab

Graphs and simulations drawn in greyscale on the panels. With both panels
ticked they work as one 18 x 34 picture. A preview on the right shows the same
thing, even with no panels connected.

- **Graphs**: 20 ready-made ones (sine, tangent, bell curve, damped wave...),
  *Random graphs* to cycle through them, or type your own formula such as
  `sin(x) * x`. Put `t` in a formula to make it move, like `sin(x - t)`.
- **Game of Life and other Life-like rules**: Conway's Life, HighLife, Seeds,
  Day & Night, Maze and more, or type your own birth/survival numbers. Start
  from random cells, a math graph, or a classic pattern. It starts over when
  it dies out or settles.
- **Traffic jam** (Biham-Middleton-Levine model): two kinds of cars taking
  turns. Busy roads jam for good.
- **Gas particles** (Hardy-Pomeau-Pazzis lattice gas): particles bouncing off
  each other and the walls, starting at random, packed in a corner, or along a
  math graph.
- **One-line automata**: elementary rules like Rule 30 and Rule 90, and
  3-colour totalistic codes like 777, scrolling up one generation per row.
- **Random grey noise**, and a speed slider for all of them.

![The Math tab](docs/screenshots/white-rabbit-math.png)

### Firmware tab

Shows which firmware each panel has, puts a panel into bootloader mode, and
flashes the bundled firmware: White Rabbit's (startup animation setting, extra
patterns) or Framework's official 0.2.0, or any `.uf2` file you choose.

![The Firmware tab](docs/screenshots/white-rabbit-firmware.png)

### Keyboard shortcuts tab

Lists the monitor's keyboard shortcuts and what each one does, with a
checkbox to turn each shortcut on or off. A shortcut that's off is left free
for other programs, and the change works straight away without restarting.

*Screenshots taken on Linux with simulated panels, so the window looks a little
different from Windows.*

## Install and use

1. Download [`WhiteRabbit-setup.exe`](https://github.com/kingof9x/white-rabbit-framework-matrix/releases/latest/download/WhiteRabbit-setup.exe)
   (or the portable `WhiteRabbit.exe`) and run it. If Windows shows *"Windows
   protected your PC"*, click *More info* > *Run anyway* (the app isn't code-signed).
2. Quit the old apps first if you use them: led_mon (tray icon > *Quit*) and
   `ledmatrixgui.exe`. They would be holding the panels.
3. The panels show the system monitor, an LED icon appears in the tray, and
   the White Rabbit window opens. Opening the app again from the Start menu,
   or *Open White Rabbit...* in the tray menu, brings the window back.
   To keep the tray icon out of the hidden-icons arrow, turn on *White Rabbit* in
   *Settings > Personalization > Taskbar > Other system tray icons* (Windows 11),
   or drag the icon from the arrow onto the taskbar.
4. **Coming from led_mon?** Your layout, brightness and weather API key are
   copied from `%APPDATA%\led_mon` the first time White Rabbit starts.
5. The checkboxes at the bottom of the window choose which panels the
   Startup animation, Games & patterns and Firmware controls act on. *Back to
   system monitor* hands borrowed panels back to the monitor.
6. **Firmware:** the startup animation and the extra patterns need the White
   Rabbit firmware. On the Firmware tab, tick one panel, pick *White Rabbit
   firmware*, click *Flash*, then do the same for the other panel.

Something not working? Right-click the tray icon > *Open log*, or run
`WhiteRabbit.exe --diagnose` in a command prompt to see which panels the app finds.

### Good to know

- **Temperatures and fans**: on the Panels tab, click the Temperatures or Fans app on a panel. Its
  settings (on the right) choose which sensor each of the three numbers shows and Celsius or Fahrenheit,
  plus, for all panels, where readings come from (*Read from*, saved as `sensor_source` in `settings.json`)
  and how often (*Read every*, `sensor_refresh_seconds`):
  - *Automatic* (default): tries each of the ones below in that order and uses the first that gives readings.
  - *[Framework Control](https://github.com/ozturkkl/framework-control)*: its service reads them with
    system rights, so nothing has to be open and White Rabbit needs no admin prompt. On a Framework 16
    it gives the CPU, APU, ambient and charger temperatures and both fans.
  - *[Libre Hardware Monitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases)*:
    White Rabbit reads its sensors through Windows (WMI) while it is open and running as admin.
    Which sensors show depends on the laptop.
  - *framework_tool*: Framework's `framework_tool.exe`
    ([framework-system releases](https://github.com/FrameworkComputer/framework-system/releases)), next to
    `WhiteRabbit.exe` or on PATH. Only works when White Rabbit itself runs as admin.

  The Panels tab shows which source the readings come from right now, and `WhiteRabbit.exe --diagnose`
  lists what each source gives.
- **FPS counter** uses Intel's PresentMon, which the installer includes. Windows
  only lets programs read frame timing when the account is in the *Performance
  Log Users* group. One-time setup in *Terminal (Admin)*:
  `Add-LocalGroupMember -SID S-1-5-32-559 -Member "$env:USERDOMAIN\$env:USERNAME"`,
  then sign out and back in.
  In the FPS counter's settings, *Game to watch* starts on *Automatic*: it shows the
  full-screen game in front and keeps showing it while you switch to other windows. To
  watch one game all the time, open the list and pick it by its window title; apps
  drawing frames right now are at the top with their FPS.
  The graph under the number is GPU use over the last 8 seconds. On a Framework 16
  with the graphics module, set *GPU for the graph* to the Radeon RX 7700S, since
  games run on it.
- **[Libre Hardware Monitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/releases)** has no installer, so the
  tray looks anywhere in your Desktop, Downloads and Documents (three folders
  deep), in Program Files, `C:\`, and next to `WhiteRabbit.exe`. If it isn't in any of those, the tray item says *Find Libre
  Hardware Monitor...*: click it and pick `LibreHardwareMonitor.exe` once, and it
  is remembered. It asks for admin rights each time it starts (that's normal for it).
  The old Open Hardware Monitor isn't supported: it unpacks the WinRing0 driver, which
  has a known security hole, and Windows Defender blocks it and can flag apps that start it.
- **Weather** needs an API key in `%APPDATA%\WhiteRabbit\.env` (see [`.env-example`](.env-example)).
- **Where things live:** settings and layout in `%APPDATA%\WhiteRabbit`, the
  log in `%LOCALAPPDATA%\WhiteRabbit\WhiteRabbit.log`.
- **Command line:** `WhiteRabbit.exe --background` starts only the monitor and
  tray icon (that's what runs at login); `--window` only the window;
  `--window --tab firmware` opens it on a tab.

## Testing checklist

Tested on a real Framework Laptop 16 with two LED Matrix modules on Windows 11.
Confirmed items are removed from this list (kept in step with the Project Workbench). Tick items off (edit this file, or mention them
in an issue) as they are confirmed. Items marked *(confirmed in the old app)*
worked in led_mon or LED Matrix Control, but need checking again here.

**Firmware tab**
- [ ] The bundled White Rabbit firmware includes Patterns 2 to 5 (builds made after 2026-10-28 06:00 UTC)

## Making a release

In GitHub go to *Actions > Windows build > Run workflow*, type a version such
as `1.0.0`, and run it. When it finishes, the release `v1.0.0` appears on the
Releases page with `WhiteRabbit-setup.exe`, `WhiteRabbit.exe` and
`ledmatrix.uf2` attached. Leaving the version empty makes a test build (the
files are under the run's *Artifacts*) without a release.

The firmware is built in [kingof9x/inputmodule-rs](https://github.com/kingof9x/inputmodule-rs):
each White Rabbit build bundles the newest firmware from its Releases page.

## Run or build it yourself

Windows, Python 3.12:

```powershell
git clone https://github.com/kingof9x/white-rabbit-framework-matrix
cd white-rabbit-framework-matrix
py -3.12 -m venv .venv
.venv\Scripts\activate
python -m pip install -r requirements.txt pytest
python -m pytest
python main.py                    # monitor + tray + window
python main.py --window           # just the window
python -m PyInstaller white_rabbit.spec --noconfirm    # dist\WhiteRabbit\WhiteRabbit.exe
```

On Linux the monitor runs in the console as before (`python main.py`), and
`python main.py --window` opens the window next to it.

## Where it comes from

White Rabbit combines two open source projects, both MIT licensed (see
[LICENSE](LICENSE) and [LICENSES/](LICENSES)):

- [kingof9x/led-matrix](https://github.com/kingof9x/led-matrix), a Windows
  fork of [MidnightJava/led-matrix](https://github.com/MidnightJava/led-matrix),
  itself based on [LED System Monitor](https://code.karsttech.com/jeremy/FW_LED_System_Monitor.git).
  It is the `led_mon` folder here.
- [kingof9x/inputmodule-rs](https://github.com/kingof9x/inputmodule-rs), a fork
  of [Framework's inputmodule-rs](https://github.com/FrameworkComputer/inputmodule-rs).
  Its Python package is the `inputmodule` folder here; the LED Matrix firmware
  itself is still built there.

Thanks to everyone who worked on those.

The Math tab's ideas come from mariobx's
[Framework-LED-Matrix](https://github.com/mariobx/Framework-LED-Matrix). That
project is GPL-3.0 licensed, so no code was copied: the Math tab is written
from scratch from the textbook definitions of each graph and simulation.

## Project status

**Status:** Waiting on developer  
**Waiting on:** Developer: run the testing checklist  
**Notes:** One tray app combining led_mon and the LED Matrix controls. Not released yet. Newest additions: Keyboard shortcuts tab, Open Hardware Monitor finder fixes.  
**Last updated:** 2026-11-02

