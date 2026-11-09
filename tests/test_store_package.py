# The Microsoft Store (MSIX) package: building its manifest and logos, and how the app behaves when it
# runs packaged (settings folder, "Start with Windows" as a startup task, updates from the Store).
import os
import sys
import xml.etree.ElementTree as ET

import pytest

from led_mon import platform_utils, startup

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts"))
import build_msix  # noqa: E402

FOUNDATION = "{http://schemas.microsoft.com/appx/manifest/foundation/windows10}"
DESKTOP = "{http://schemas.microsoft.com/appx/manifest/desktop/windows10}"
RESCAP = "{http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities}"
UAP = "{http://schemas.microsoft.com/appx/manifest/uap/windows10}"

STORE_IDENTITY = {"identity_name": "12345kingof9x.WhiteRabbit", "publisher": "CN=ABCDEF12-3456-7890-ABCD-EF1234567890",
                  "publisher_display_name": "kingof9x"}


# Building the package

def test_package_version_has_four_numbers_ending_in_zero():
    assert build_msix.msix_version("1.2.0") == "1.2.0.0"
    assert build_msix.msix_version("v1.2") == "1.2.0.0"
    assert build_msix.msix_version("1") == "1.0.0.0"
    assert build_msix.msix_version("1.2.3-beta") == "1.2.3.0"
    assert build_msix.msix_version("1.2.3.4") == "1.2.3.0"


@pytest.mark.parametrize("bad", ["", "beta", "0.9.0", "1.70000.0"])
def test_package_version_refuses_what_the_store_refuses(bad):
    with pytest.raises(ValueError):
        build_msix.msix_version(bad)


def test_identity_comes_from_the_environment_or_test_stand_ins():
    values = build_msix.identity(environ={})
    assert build_msix.is_test_identity(values)
    values = build_msix.identity(environ={"MSIX_IDENTITY_NAME": STORE_IDENTITY["identity_name"],
                                          "MSIX_PUBLISHER": STORE_IDENTITY["publisher"],
                                          "MSIX_PUBLISHER_DISPLAY_NAME": "kingof9x"})
    assert values == STORE_IDENTITY
    assert not build_msix.is_test_identity(values)


def test_identity_catches_values_pasted_into_the_wrong_box():
    with pytest.raises(ValueError):
        build_msix.identity(publisher="kingof9x", environ={})
    with pytest.raises(ValueError):
        build_msix.identity(identity_name="White Rabbit", environ={})


def manifest(version="1.2.0", values=STORE_IDENTITY):
    return ET.fromstring(build_msix.render_manifest(version, values).encode("utf-8"))


def test_manifest_identity_and_version():
    root = manifest()
    found = root.find(f"{FOUNDATION}Identity")
    assert found.get("Name") == STORE_IDENTITY["identity_name"]
    assert found.get("Publisher") == STORE_IDENTITY["publisher"]
    assert found.get("Version") == "1.2.0.0"
    assert found.get("ProcessorArchitecture") == "x64"
    assert root.find(f"{FOUNDATION}Properties/{FOUNDATION}PublisherDisplayName").text == "kingof9x"


def test_manifest_escapes_values():
    text = build_msix.render_manifest("1.0", dict(STORE_IDENTITY, publisher='CN="A & B"'))
    assert 'Publisher="CN=&quot;A &amp; B&quot;"' in text


def test_manifest_runs_the_exe_as_a_desktop_app_with_its_startup_task():
    root = manifest()
    apps = root.findall(f"{FOUNDATION}Applications/{FOUNDATION}Application")
    assert len(apps) == 1
    assert apps[0].get("Executable") == "WhiteRabbit.exe"
    assert apps[0].get("EntryPoint") == "Windows.FullTrustApplication"
    task = apps[0].find(f"{FOUNDATION}Extensions/{DESKTOP}Extension/{DESKTOP}StartupTask")
    assert task.get("TaskId") == startup.STARTUP_TASK_ID
    capabilities = root.find(f"{FOUNDATION}Capabilities")
    assert capabilities.find(f"{RESCAP}Capability").get("Name") == "runFullTrust"
    devices = {d.get("Name") for d in capabilities.findall(f"{FOUNDATION}DeviceCapability")}
    assert devices == {"location", "microphone"}


def test_layout_has_the_app_manifest_and_every_logo_it_names(tmp_path):
    dist = tmp_path / "dist"
    dist.mkdir()
    (dist / "WhiteRabbit.exe").write_bytes(b"MZ")
    layout = tmp_path / "layout"
    build_msix.lay_out(str(dist), str(layout), "1.2.0", STORE_IDENTITY)
    assert (layout / "WhiteRabbit.exe").read_bytes() == b"MZ"
    text = (layout / "AppxManifest.xml").read_text(encoding="utf-8")
    from PIL import Image
    for name, width, height in build_msix.ASSETS:
        with Image.open(layout / "Assets" / name) as image:
            assert image.size == (width, height), name
    for path in ("Assets\\StoreLogo.png", "Assets\\Square150x150Logo.png", "Assets\\Square44x44Logo.png",
                 "Assets\\Wide310x150Logo.png"):
        assert path in text
        assert (layout / path.replace("\\", "/")).is_file()


# Running packaged

@pytest.fixture
def packaged(monkeypatch, tmp_path):
    monkeypatch.setattr(platform_utils, "IS_WINDOWS", True)
    monkeypatch.setattr(platform_utils, "_package_family", "kingof9x.WhiteRabbit_abc123")
    monkeypatch.setenv("APPDATA", str(tmp_path / "Roaming"))
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "Local"))
    monkeypatch.delenv("CONFIG_FILE", raising=False)
    return tmp_path


def test_not_packaged_off_windows():
    assert platform_utils.package_family_name() is None
    assert not platform_utils.is_store_package()


def test_packaged_settings_and_log_live_in_the_package_data_folder(packaged):
    expected = os.path.join(str(packaged), "Local", "Packages", "kingof9x.WhiteRabbit_abc123", "LocalState",
                            "WhiteRabbit")
    assert platform_utils.user_config_dir() == expected
    assert platform_utils.user_log_dir() == expected
    assert platform_utils.resolve_config_file() == os.path.join(expected, "config.yaml")


def test_packaged_app_copies_the_installer_versions_settings_once(packaged):
    old = packaged / "Roaming" / "WhiteRabbit"
    old.mkdir(parents=True)
    (old / "settings.json").write_text('{"music": true}')
    (old / "config-local.yaml").write_text("duration: 3\n")
    (old / "WhiteRabbit.log").write_text("old log")
    assert sorted(platform_utils.import_old_settings()) == ["config-local.yaml", "settings.json"]
    new = platform_utils.user_config_dir()
    assert open(os.path.join(new, "settings.json")).read() == '{"music": true}'
    assert not os.path.exists(os.path.join(new, "WhiteRabbit.log"))
    (old / "settings.json").write_text('{"music": false}')
    assert platform_utils.import_old_settings() == []


def test_packaged_app_falls_back_to_led_mon_settings(packaged):
    old = packaged / "Roaming" / "led_mon"
    old.mkdir(parents=True)
    (old / ".env").write_text("KEY=1\n")
    assert platform_utils.import_old_settings() == [".env"]


class FakeTask:
    def __init__(self, state=startup.TASK_DISABLED, enable_gives=startup.TASK_ENABLED):
        self.current = state
        self.enable_gives = enable_gives

    def state(self):
        return self.current

    def enable(self):
        self.current = self.enable_gives
        return self.current

    def disable(self):
        self.current = startup.TASK_DISABLED
        return self.current


def test_startup_task_on_and_off():
    task = FakeTask()
    assert startup.is_enabled(task=task) is False
    startup.set_enabled(True, task=task)
    assert startup.is_enabled(task=task) is True
    startup.set_enabled(False, task=task)
    assert startup.is_enabled(task=task) is False
    assert startup.is_enabled(task=FakeTask(startup.TASK_ENABLED_BY_POLICY)) is True


@pytest.mark.parametrize("blocked", [startup.TASK_DISABLED_BY_USER, startup.TASK_DISABLED_BY_POLICY])
def test_startup_task_turned_off_by_the_user_says_where_to_turn_it_on(blocked):
    task = FakeTask(blocked, enable_gives=blocked)
    with pytest.raises(startup.StartupBlocked):
        startup.set_enabled(True, task=task)
    assert startup.is_enabled(task=task) is False


def test_packaged_app_uses_the_startup_task_not_the_run_key(packaged, monkeypatch):
    assert isinstance(startup._store_task(None), startup.StoreStartupTask)
    monkeypatch.setattr(platform_utils, "_package_family", None)
    assert startup._store_task(None) is None


def test_started_at_login_by_the_store_startup_task_stays_in_the_tray(monkeypatch):
    from white_rabbit import main
    calls = []
    monkeypatch.setattr(main.platform_utils, "IS_WINDOWS", True)
    monkeypatch.setattr(main.platform_utils, "load_env_file", lambda: None)
    monkeypatch.setattr(main.platform_utils, "setup_logging", lambda: None)
    monkeypatch.setattr(main.platform_utils, "launch_self", lambda *a: calls.append(("launch",) + a))
    monkeypatch.setattr(main, "run_monitor", lambda argv: calls.append(("monitor",) + tuple(argv)))
    monkeypatch.setattr(startup, "started_by_startup_task", lambda: True)
    main.run([])
    assert calls == [("monitor",)]


def test_unpackaged_app_is_never_started_by_a_startup_task():
    assert startup.started_by_startup_task() is False


def test_store_app_gets_updates_from_the_store(packaged, monkeypatch):
    from white_rabbit import info_tab
    monkeypatch.setattr(platform_utils, "is_frozen", lambda: True)
    assert info_tab.store_updates()
    assert not info_tab.can_install_updates()
    monkeypatch.setattr(platform_utils, "_package_family", None)
    assert not info_tab.store_updates()
    assert info_tab.can_install_updates()
