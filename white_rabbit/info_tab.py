# The Info tab: who made White Rabbit, the projects it comes from, checking for a newer version,
# and reporting a problem.
#
# Checking for updates asks GitHub for the newest release. On Windows the app can download that
# release's installer, close itself and start the installer, which keeps the user's settings.

# Built In Dependencies
import os
import re
import sys
import logging
import tempfile
import webbrowser
import tkinter as tk
from tkinter import ttk, messagebox, font as tkfont

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

HINT = "#555555"
LINK = "#1a5fb4"

# Tips for the developer
KOFI_URL = "https://ko-fi.com/kingof9x"
REPO = "kingof9x/white-rabbit-framework-matrix"
REPO_URL = f"https://github.com/{REPO}"
ISSUES_URL = f"{REPO_URL}/issues"
RELEASES_URL = f"{REPO_URL}/releases/latest"
LATEST_RELEASE_API = f"https://api.github.com/repos/{REPO}/releases/latest"
# The installer's name is the same in every release (see .github/workflows/windows.yml)
SETUP_ASSET = "WhiteRabbit-setup.exe"

# The projects White Rabbit is based on or inspired by: (name, url, what came from it)
CREDITS = [
    ("led-matrix by MidnightJava", "https://github.com/MidnightJava/led-matrix",
     "The system monitor: apps on the panels, plugins and the layout file."),
    ("LED System Monitor by Jeremy", "https://code.karsttech.com/jeremy/FW_LED_System_Monitor",
     "The original system monitor that led-matrix grew from."),
    ("inputmodule-rs by Framework", "https://github.com/FrameworkComputer/inputmodule-rs",
     "The LED Matrix firmware and the LED Matrix Control app: patterns, games and firmware updates."),
    ("Framework-LED-Matrix by mariobx", "https://github.com/mariobx/Framework-LED-Matrix",
     "The ideas for the Math tab (written from scratch, no code copied)."),
    ("pixeled by patate (slowpaste)", "https://github.com/slowpaste/pixeled",
     "The ideas for the Effects tab, the sand battery and now playing apps (written from scratch)."),
]


# Versions

def app_version():
    """This copy's version: from the VERSION file the build adds, or pyproject.toml when run from source."""
    here = os.path.dirname(os.path.abspath(__file__))
    try:
        with open(os.path.join(here, "VERSION")) as f:
            version = f.read().strip()
            if version:
                return version
    except OSError:
        pass
    try:
        import tomllib
        with open(os.path.join(os.path.dirname(here), "pyproject.toml"), "rb") as f:
            return tomllib.load(f)["project"]["version"]
    except Exception:
        return None


def version_numbers(text):
    """'v1.2' -> (1, 2, 0, 0), so '1' and '1.0.0' count as the same version."""
    numbers = [int(n) for n in re.findall(r"\d+", text or "")]
    return tuple((numbers + [0] * 4)[:4])


def is_newer(latest, current):
    if not current:
        return True
    return version_numbers(latest) > version_numbers(current)


def latest_release(get=None):
    """The newest GitHub release: {'version', 'page', 'setup_url'} (setup_url is None if it has no installer)."""
    if get is None:
        import requests
        get = requests.get
    reply = get(LATEST_RELEASE_API, timeout=15, headers={"Accept": "application/vnd.github+json"})
    reply.raise_for_status()
    release = reply.json()
    setup_url = next((asset.get("browser_download_url") for asset in release.get("assets", [])
                      if asset.get("name") == SETUP_ASSET), None)
    return {"version": release.get("tag_name", "").lstrip("v"),
            "page": release.get("html_url") or RELEASES_URL,
            "setup_url": setup_url}


def download(url, progress=None, get=None):
    """Download the installer into a temporary folder and return its path. progress(fraction or None)."""
    if get is None:
        import requests
        get = requests.get
    folder = tempfile.mkdtemp(prefix="WhiteRabbit-update-")
    path = os.path.join(folder, SETUP_ASSET)
    with get(url, stream=True, timeout=30) as reply:
        reply.raise_for_status()
        total = int(reply.headers.get("Content-Length") or 0)
        done = 0
        with open(path, "wb") as f:
            for chunk in reply.iter_content(chunk_size=256 * 1024):
                f.write(chunk)
                done += len(chunk)
                if progress is not None:
                    progress(done / total if total else None)
    return path


def can_install_updates():
    """Only the Windows exe can install the new version itself; elsewhere the release page opens."""
    return platform_utils.IS_WINDOWS and platform_utils.is_frozen()


def start_installer_after_exit(path):
    """Start the installer a couple of seconds from now, once White Rabbit has closed.

    The installer refuses to start while White Rabbit is running, so it waits in a hidden cmd.
    """
    import subprocess
    subprocess.Popen(installer_command(path), creationflags=platform_utils.subprocess_flags(),
                     close_fds=True)


def installer_command(path):
    """The cmd line that waits a moment and then starts the installer at path.

    It is one string on purpose: given a list, Python adds backslashes before the inner quotes,
    which cmd doesn't understand, so start got a broken path ("Windows cannot find '\\\\'").
    """
    return f'cmd /c ping -n 3 127.0.0.1 >nul & start "" "{path}"'


# The tab

class InfoTab(ttk.Frame):
    def __init__(self, master, window):
        super().__init__(master, padding=6)
        self.window = window
        self.release = None
        self.version = app_version()

        about = self.section("White Rabbit")
        version_text = f"Version {self.version}" if self.version else "Version unknown (running from source)"
        ttk.Label(about, text=version_text).pack(anchor="w")
        ttk.Label(about, text="One app for the Framework 16 LED Matrix panels: the system monitor, games, patterns, "
                  "startup animations and firmware updates.", foreground=HINT, wraplength=760,
                  justify="left").pack(anchor="w", pady=(2, 6))
        buttons = ttk.Frame(about)
        buttons.pack(anchor="w")
        ttk.Button(buttons, text="White Rabbit on GitHub", command=lambda: self.open(REPO_URL)).pack(side="left")
        ttk.Button(buttons, text="Report a bug or send feedback",
                   command=lambda: self.open(ISSUES_URL)).pack(side="left", padx=8)

        tips = self.section("Enjoying White Rabbit?")
        ttk.Label(tips, text="White Rabbit is free. If you like it, you can leave a tip on Ko-fi. Thank you!",
                  wraplength=760, justify="left").pack(anchor="w", pady=(0, 6))
        ttk.Button(tips, text="Leave a tip on Ko-fi", command=lambda: self.open(KOFI_URL)).pack(anchor="w")

        updates = self.section("Updates")
        row = ttk.Frame(updates)
        row.pack(anchor="w", fill="x")
        self.check_button = ttk.Button(row, text="Check for updates", command=self.check)
        self.check_button.pack(side="left")
        self.install_button = ttk.Button(row, text="Download and install", command=self.install)
        self.page_button = ttk.Button(row, text="Open the download page", command=self.open_release_page)
        self.update_status = ttk.Label(updates, text="", foreground=HINT, wraplength=760, justify="left")
        self.update_status.pack(anchor="w", pady=(6, 0))

        credits = self.section("Based on")
        ttk.Label(credits, text="White Rabbit is built on these open source projects. Thanks to everyone who "
                  "worked on them.", foreground=HINT, wraplength=760, justify="left").grid(
            row=0, column=0, columnspan=2, sticky="w", pady=(0, 6))
        self.link_font = tkfont.nametofont("TkDefaultFont").copy()
        self.link_font.configure(underline=True)
        link_font = self.link_font
        for i, (name, url, what) in enumerate(CREDITS, start=1):
            link = tk.Label(credits, text=name, fg=LINK, cursor="hand2", font=link_font)
            link.grid(row=i, column=0, sticky="w", padx=(0, 16), pady=2)
            link.bind("<Button-1>", lambda event, url=url: self.open(url))
            ttk.Label(credits, text=what, foreground=HINT).grid(row=i, column=1, sticky="w", pady=2)

    def section(self, title):
        frame = ttk.LabelFrame(self, text=title, padding=8)
        frame.pack(fill="x", padx=12, pady=6)
        return frame

    def open(self, url):
        try:
            webbrowser.open(url)
        except Exception as e:
            log.warning(f"Could not open {url}: {e}")

    def open_release_page(self):
        self.open(self.release["page"] if self.release else RELEASES_URL)

    def show(self, text):
        self.update_status.configure(text=text)

    def show_buttons(self, install=False, page=False):
        self.install_button.pack_forget()
        self.page_button.pack_forget()
        if install:
            self.install_button.pack(side="left", padx=(8, 0))
        if page:
            self.page_button.pack(side="left", padx=(8, 0))

    # Checking
    def check(self):
        self.check_button.state(["disabled"])
        self.show_buttons()
        self.show("Asking GitHub for the newest version...")
        self.window.in_background(latest_release, self.checked, self.check_failed)

    def check_failed(self, error):
        self.check_button.state(["!disabled"])
        log.warning(f"Update check failed: {error}")
        self.show("Couldn't reach GitHub to check for updates. Check the internet connection and try again.")
        self.show_buttons(page=True)

    def checked(self, release):
        self.check_button.state(["!disabled"])
        self.release = release
        latest = release["version"] or "unknown"
        if not is_newer(release["version"], self.version):
            self.show(f"You have the newest version ({self.version}).")
            return
        if can_install_updates() and release["setup_url"]:
            self.show(f"Version {latest} is available (you have {self.version}). "
                      "\"Download and install\" gets the installer, closes White Rabbit and starts the installer. "
                      "Your settings are kept.")
            self.show_buttons(install=True, page=True)
        else:
            self.show(f"Version {latest} is available (you have {self.version or 'an unknown version'}).")
            self.show_buttons(page=True)

    # Installing
    def install(self):
        if not self.release or not self.release["setup_url"]:
            return
        self.install_button.state(["disabled"])
        self.check_button.state(["disabled"])
        self.show(f"Downloading version {self.release['version']}...")

        def progress(fraction):
            if fraction is not None:
                self.window.call_soon(lambda: self.show(
                    f"Downloading version {self.release['version']}... {int(fraction * 100)}%"))

        self.window.in_background(lambda: download(self.release["setup_url"], progress),
                                  self.downloaded, self.download_failed)

    def download_failed(self, error):
        self.install_button.state(["!disabled"])
        self.check_button.state(["!disabled"])
        log.warning(f"Update download failed: {error}")
        self.show("The download didn't work. Try again, or get it from the download page.")

    def downloaded(self, path):
        self.install_button.state(["!disabled"])
        self.check_button.state(["!disabled"])
        if not messagebox.askokcancel(
                "Install the update",
                f"Version {self.release['version']} is downloaded.\n\nWhite Rabbit will close (the panels too) and "
                "the installer will start. Your settings are kept.", parent=self):
            self.show("Downloaded, not installed yet. Click \"Download and install\" when you're ready.")
            return
        self.show("Closing White Rabbit and starting the installer...")
        self.window.quit_for_update(lambda: start_installer_after_exit(path))
