"""The installed Windows app: WhiteRabbit.exe is the Rust monitor, WhiteRabbitWindow.exe (this Python app) the window."""

import os
import sys

from led_mon import platform_utils


def _frozen_at(monkeypatch, tmp_path, exe_name, with_monitor=True):
    monkeypatch.setattr(platform_utils, "IS_WINDOWS", True)
    monkeypatch.setattr(platform_utils, "is_frozen", lambda: True)
    exe = tmp_path / exe_name
    exe.write_bytes(b"")
    if with_monitor:
        (tmp_path / "WhiteRabbit.exe").write_bytes(b"")
    monkeypatch.setattr(sys, "executable", str(exe))
    return tmp_path


def test_window_exe_finds_the_rust_monitor(monkeypatch, tmp_path):
    folder = _frozen_at(monkeypatch, tmp_path, "WhiteRabbitWindow.exe")
    assert platform_utils.rust_monitor_exe() == os.path.join(str(folder), "WhiteRabbit.exe")


def test_portable_python_exe_runs_its_own_monitor(monkeypatch, tmp_path):
    # The portable exe is itself WhiteRabbit.exe: it must not start itself as "the Rust monitor"
    _frozen_at(monkeypatch, tmp_path, "WhiteRabbit.exe")
    assert platform_utils.rust_monitor_exe() is None


def test_no_rust_monitor_next_to_it(monkeypatch, tmp_path):
    _frozen_at(monkeypatch, tmp_path, "WhiteRabbitWindow.exe", with_monitor=False)
    assert platform_utils.rust_monitor_exe() is None


def test_from_source_uses_python_monitor(monkeypatch):
    monkeypatch.setattr(platform_utils, "is_frozen", lambda: False)
    assert platform_utils.rust_monitor_exe() is None


def test_start_monitor_runs_the_rust_monitor(monkeypatch, tmp_path):
    folder = _frozen_at(monkeypatch, tmp_path, "WhiteRabbitWindow.exe")
    started = []

    class FakePopen:
        def __init__(self, command, env=None, **kwargs):
            started.append((command, env))

    import subprocess
    monkeypatch.setattr(subprocess, "Popen", FakePopen)
    monkeypatch.setattr(platform_utils, "subprocess_flags", lambda: 0)
    platform_utils.launch_monitor("my.yaml")
    command, env = started[0]
    assert command == [os.path.join(str(folder), "WhiteRabbit.exe"), "--background"]
    assert env["CONFIG_FILE"] == "my.yaml"


def test_plain_start_of_the_window_exe_hands_over(monkeypatch, tmp_path):
    folder = _frozen_at(monkeypatch, tmp_path, "WhiteRabbitWindow.exe")
    started = []
    import subprocess
    monkeypatch.setattr(subprocess, "Popen", lambda command, **kw: started.append(command))
    monkeypatch.setattr(platform_utils, "subprocess_flags", lambda: 0)
    monkeypatch.setattr(platform_utils, "setup_logging", lambda: None)
    from white_rabbit import main
    monkeypatch.setattr(main, "run_monitor", lambda argv: started.append("python monitor"))
    main.run([])
    main.run(["--background"])
    rust = os.path.join(str(folder), "WhiteRabbit.exe")
    assert started == [[rust], [rust, "--background"]]
