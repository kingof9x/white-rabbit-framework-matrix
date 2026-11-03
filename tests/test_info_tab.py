# The Info tab: version numbers, asking GitHub for the newest release, downloading the installer,
# and the monitor's quit command used before installing.
import io

from white_rabbit import info_tab


class FakeReply:
    def __init__(self, data=None, body=b"", headers=None, status=200):
        self.data = data
        self.body = body
        self.headers = headers or {}
        self.status = status

    def raise_for_status(self):
        if self.status != 200:
            raise RuntimeError(f"HTTP {self.status}")

    def json(self):
        return self.data

    def iter_content(self, chunk_size):
        stream = io.BytesIO(self.body)
        while chunk := stream.read(chunk_size):
            yield chunk

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def test_version_numbers_ignore_the_v_and_missing_parts():
    assert info_tab.version_numbers("v1") == info_tab.version_numbers("1.0.0")
    assert info_tab.version_numbers("1.10") > info_tab.version_numbers("1.9")


def test_is_newer():
    assert info_tab.is_newer("1.1", "1")
    assert info_tab.is_newer("2", "1.9.9")
    assert not info_tab.is_newer("1", "1.0.0")
    assert not info_tab.is_newer("1", "1.2")
    # Running a copy with no version: always offer the newest
    assert info_tab.is_newer("1", None)


def test_app_version_from_source_is_pyprojects():
    import tomllib, os
    root = os.path.dirname(os.path.dirname(info_tab.__file__))
    with open(os.path.join(root, "pyproject.toml"), "rb") as f:
        expected = tomllib.load(f)["project"]["version"]
    assert info_tab.app_version() == expected


def test_app_version_prefers_the_builds_version_file(tmp_path, monkeypatch):
    (tmp_path / "VERSION").write_text("1.2\n")
    monkeypatch.setattr(info_tab, "__file__", str(tmp_path / "info_tab.py"))
    assert info_tab.app_version() == "1.2"


def test_latest_release_finds_the_installer():
    calls = []

    def get(url, **kwargs):
        calls.append(url)
        return FakeReply({"tag_name": "v2", "html_url": "https://example/v2", "assets": [
            {"name": "WhiteRabbit.exe", "browser_download_url": "https://example/WhiteRabbit.exe"},
            {"name": "WhiteRabbit-setup.exe", "browser_download_url": "https://example/WhiteRabbit-setup.exe"},
        ]})
    release = info_tab.latest_release(get)
    assert calls == [info_tab.LATEST_RELEASE_API]
    assert release == {"version": "2", "page": "https://example/v2",
                       "setup_url": "https://example/WhiteRabbit-setup.exe"}


def test_latest_release_without_an_installer():
    release = info_tab.latest_release(lambda url, **kw: FakeReply({"tag_name": "v3", "assets": []}))
    assert release["setup_url"] is None
    assert release["page"] == info_tab.RELEASES_URL


def test_download_saves_the_installer_and_reports_progress():
    body = b"x" * (600 * 1024)
    seen = []
    path = info_tab.download("https://example/setup", seen.append,
                             lambda url, **kw: FakeReply(body=body, headers={"Content-Length": str(len(body))}))
    assert path.endswith("WhiteRabbit-setup.exe")
    with open(path, "rb") as f:
        assert f.read() == body
    assert seen[-1] == 1.0


def test_links_point_at_the_right_pages():
    assert info_tab.ISSUES_URL == "https://github.com/kingof9x/white-rabbit-framework-matrix/issues"
    assert info_tab.REPO_URL == "https://github.com/kingof9x/white-rabbit-framework-matrix"
    urls = [url for _, url, _ in info_tab.CREDITS]
    assert "https://github.com/MidnightJava/led-matrix" in urls
    assert "https://github.com/FrameworkComputer/inputmodule-rs" in urls


def test_tray_quit_from_elsewhere():
    from led_mon import tray
    assert tray.quit_from_elsewhere() is False
    quits = []
    tray._quit_from_elsewhere = lambda: quits.append(True)
    try:
        assert tray.quit_from_elsewhere() is True
    finally:
        tray._quit_from_elsewhere = None
    assert quits == [True]


def test_kofi_link():
    assert info_tab.KOFI_URL == "https://ko-fi.com/kingof9x"


def test_versions_typed_in_different_ways_match():
    # The workflow can be given "1.1" or "v1.1.0"; the release tag and the build's VERSION must still agree
    assert not info_tab.is_newer("1.1", "1.1.0")
    assert not info_tab.is_newer("v1.1.0", "1.1")
    assert info_tab.is_newer("1.1", "1")       # the first release was tagged v1
    assert not info_tab.is_newer("1", "1.1")
    assert info_tab.is_newer("1.10", "1.9")


def test_window_title_shows_the_version(monkeypatch):
    from white_rabbit import window
    monkeypatch.setattr(info_tab, "app_version", lambda: "1.1")
    assert window.window_title() == "White Rabbit 1.1"
    monkeypatch.setattr(info_tab, "app_version", lambda: None)
    assert window.window_title() == "White Rabbit"


def test_installer_is_started_with_a_command_cmd_understands(monkeypatch):
    # Windows said "cannot find '\\'" because the quotes reached cmd as \" (Python's list quoting)
    import subprocess
    path = r"C:\Users\Some One\AppData\Local\Temp\WhiteRabbit-update-x\WhiteRabbit-setup.exe"
    started = []
    monkeypatch.setattr(subprocess, "Popen", lambda args, **kw: started.append(args))
    info_tab.start_installer_after_exit(path)
    assert started == [f'cmd /c ping -n 3 127.0.0.1 >nul & start "" "{path}"']
    assert '\\"' not in started[0]


def test_installer_really_starts_on_windows(tmp_path):
    # Run the real command (as the update does) with a stand-in "installer" in a folder with a space,
    # and check it actually ran
    import subprocess
    import sys
    import time
    import pytest
    if sys.platform != "win32":
        pytest.skip("cmd only exists on Windows")
    folder = tmp_path / "White Rabbit update"
    folder.mkdir()
    marker = folder / "ran.txt"
    installer = folder / "WhiteRabbit-setup.cmd"
    installer.write_text(f'@echo ran> "{marker}"\r\n@exit\r\n')
    subprocess.Popen(info_tab.installer_command(str(installer)), close_fds=True)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and not marker.exists():
        time.sleep(0.5)
    assert marker.exists(), "the stand-in installer never started"
