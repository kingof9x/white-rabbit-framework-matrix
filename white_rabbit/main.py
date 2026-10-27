# Starting White Rabbit.
#
#   WhiteRabbit.exe                 start the monitor with its tray icon and open the window
#                                   (only opens the window if the monitor is already running)
#   WhiteRabbit.exe --background    just the monitor and tray icon (used when Windows starts)
#   WhiteRabbit.exe --window        just the window; --tab startup|play|firmware picks the tab
#   WhiteRabbit.exe --edit-layout   the window on the Panels tab
#   --diagnose, --list-apps, --no-tray, --config-file ... work as in led_mon

# Built In Dependencies
import sys
import logging
from argparse import ArgumentParser

# Internal Dependencies
from led_mon import platform_utils

log = logging.getLogger(__name__)

# Options that only print something and exit: don't open a window for these
REPORT_OPTIONS = ("--help", "-h", "--list-apps", "-la", "--diagnose")


def window_args(argv):
    parser = ArgumentParser(prog="WhiteRabbit --window")
    parser.add_argument("--window", action="store_true")
    parser.add_argument("--edit-layout", action="store_true")
    parser.add_argument("--tab", choices=("panels", "startup", "play", "firmware"), default=None)
    parser.add_argument("--wait-for-monitor", action="store_true")
    parser.add_argument("--config-file", "-cf", default=None)
    args, _ = parser.parse_known_args(argv)
    return args


def run_monitor(argv):
    """Run the monitor in this process, with White Rabbit's commands on its control channel."""
    from led_mon import led_system_monitor
    from white_rabbit import bridge
    bridge.serve(led_system_monitor.extra_control_handlers)
    sys.argv = [sys.argv[0]] + argv
    led_system_monitor.main(sys.argv)


def run(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    platform_utils.load_env_file()
    platform_utils.setup_logging()
    try:
        if "--window" in argv or "--edit-layout" in argv:
            args = window_args(argv)
            from white_rabbit.window import run_window
            run_window(args, start_tab=args.tab, wait=args.wait_for_monitor)
            return
        if "--background" in argv:
            argv.remove("--background")
            run_monitor(argv)
            return
        if any(option in argv for option in REPORT_OPTIONS) or not platform_utils.IS_WINDOWS:
            # Linux keeps led_mon's behaviour (a console app or systemd service); open the window with --window
            run_monitor(argv)
            return
        from led_mon import control
        if control.request({"cmd": "status"}, timeout=1.5) is not None:
            # Already running: opening the app again shows the window
            platform_utils.launch_self("--window", *argv)
            return
        platform_utils.launch_self("--window", "--wait-for-monitor", *argv)
        run_monitor(argv)
    except Exception:
        log.exception("White Rabbit stopped with an error")
        raise
