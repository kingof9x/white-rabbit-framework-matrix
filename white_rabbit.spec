# -*- mode: python ; coding: utf-8 -*-
# PyInstaller spec for Linux and Windows.
#
#   pyinstaller white_rabbit.spec                              one-dir build: dist/WhiteRabbit/
#   WHITE_RABBIT_ONEFILE=1 pyinstaller white_rabbit.spec       one-file build: dist/WhiteRabbit(.exe)  (portable exe on Windows)

import os
import sys
from PyInstaller.utils.hooks import collect_data_files

IS_WINDOWS = sys.platform == 'win32'
ONEFILE = os.environ.get('WHITE_RABBIT_ONEFILE', '') not in ('', '0')

datas = [
    (os.path.join('led_mon', 'plugins'), os.path.join('led_mon', 'plugins')),
    (os.path.join('led_mon', 'snapshot_files'), os.path.join('led_mon', 'snapshot_files')),
    (os.path.join('led_mon', 'config.yaml'), 'led_mon'),
    (os.path.join('led_mon', 'equalizer_files'), os.path.join('led_mon', 'equalizer_files')),
    # Icon and sample images of the LED Matrix controls, and the firmware the release build downloads into res/releases
    ('res', 'res'),
]

hiddenimports = [
    'yaml', 'requests', 'zoneinfo', 'iplocate', 'dotenv',
    'scipy', 'scipy._distributor_init', 'scipy._lib.messagestream', 'scipy.special._ufuncs',
    'scipy.linalg._flapack', 'scipy.signal',
    # Plugins are loaded from files at runtime, so PyInstaller can't see what they import
    'led_mon.patterns', 'led_mon.drawing', 'led_mon.shared_state', 'led_mon.framework_tool',
    'led_mon.equalizer_files.visualize',
    # The layout editor is imported only when started with --edit-layout
    'led_mon.rendering', 'led_mon.control', 'led_mon.editor', 'led_mon.editor.window', 'led_mon.editor.preview',
    'led_mon.editor.layout', 'led_mon.editor.catalog', 'led_mon.hotkeys', 'led_mon.gpu', 'tkinter', 'tkinter.ttk', 'tkinter.messagebox',
    # Imported when the tray starts
    'led_mon.tray', 'led_mon.brightness_popup', 'led_mon.app_settings', 'led_mon.external_apps', 'led_mon.startup',
    # Imported only with --diagnose
    'led_mon.diagnose',
    # The White Rabbit window and the LED Matrix controls
    'white_rabbit', 'white_rabbit.main', 'white_rabbit.window', 'white_rabbit.tabs', 'white_rabbit.bridge',
    'white_rabbit.panels', 'white_rabbit.firmware', 'inputmodule', 'inputmodule.gui', 'inputmodule.gui.ledmatrix',
    'inputmodule.gui.pygames.snake', 'inputmodule.gui.pygames.ledris', 'inputmodule.inputmodule.ledmatrix',
    'inputmodule.uf2conv', 'inputmodule.font', 'pygame', 'tkinter.filedialog',
]

icon = None
version_info = None
if IS_WINDOWS:
    hiddenimports += [
        'soundcard', 'pystray', 'pystray._win32', 'PIL', 'PIL.ImageTk', 'wmi', 'win32com', 'win32com.client', 'pythoncom',
        'serial.tools.list_ports_windows', 'tzdata', 'win32pdh',
    ]
    # soundcard reads its C header at runtime; zoneinfo needs the tzdata database on Windows
    datas += collect_data_files('soundcard') + collect_data_files('tzdata')
    # Build the exe icon from the same drawing as the tray icon
    sys.path.insert(0, os.path.abspath('.'))
    from led_mon.tray import make_icon_image
    os.makedirs('build', exist_ok=True)
    icon = os.path.join('build', 'WhiteRabbit.ico')
    make_icon_image(256).save(icon, sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    # Version details shown in the exe's Properties; an exe without them looks more suspicious to scanners
    import tomllib
    from PyInstaller.utils.win32.versioninfo import (VSVersionInfo, FixedFileInfo, StringFileInfo, StringTable,
                                                     StringStruct, VarFileInfo, VarStruct)
    # A release build passes its version in WHITE_RABBIT_VERSION; otherwise use the one in pyproject.toml
    app_version = os.environ.get('WHITE_RABBIT_VERSION', '').strip()
    if not app_version:
        with open('pyproject.toml', 'rb') as f:
            app_version = tomllib.load(f)['project']['version']
    import re
    version_numbers = [int(n) for n in re.findall(r'\d+', app_version)]
    version_tuple = tuple((version_numbers + [0] * 4)[:4])
    version_info = VSVersionInfo(
        ffi=FixedFileInfo(filevers=version_tuple, prodvers=version_tuple),
        kids=[
            StringFileInfo([StringTable('040904B0', [
                StringStruct('CompanyName', 'kingof9x'),
                StringStruct('FileDescription', 'White Rabbit: LED Matrix app for the Framework 16'),
                StringStruct('FileVersion', app_version),
                StringStruct('InternalName', 'WhiteRabbit'),
                StringStruct('LegalCopyright', 'MIT License'),
                StringStruct('OriginalFilename', 'WhiteRabbit.exe'),
                StringStruct('ProductName', 'White Rabbit'),
                StringStruct('ProductVersion', app_version),
            ])]),
            VarFileInfo([VarStruct('Translation', [1033, 1200])]),
        ],
    )
else:
    hiddenimports += ['pynput', 'sounddevice', 'pulsectl']

# Windows uses registered hotkeys; keep pynput's keyboard hook out of the exe (antivirus flags it)
excludes = ['pynput'] if IS_WINDOWS else []

a = Analysis(
    ['main.py'],
    pathex=[],
    binaries=[],
    datas=datas,
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=excludes,
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe_options = dict(
    name='WhiteRabbit',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    # No UPX: antivirus scanners treat UPX-packed executables as suspicious
    upx=False,
    # Windows runs windowless with a tray icon; Linux stays a console app (systemd service)
    console=not IS_WINDOWS,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon=icon,
    version=version_info,
)

if ONEFILE:
    exe = EXE(
        pyz,
        a.scripts,
        a.binaries,
        a.datas,
        [],
        upx_exclude=[],
        runtime_tmpdir=None,
        **exe_options,
    )
else:
    exe = EXE(
        pyz,
        a.scripts,
        [],
        exclude_binaries=True,
        **exe_options,
    )
    coll = COLLECT(
        exe,
        a.binaries,
        a.datas,
        strip=False,
        # No UPX: antivirus scanners treat UPX-packed executables as suspicious
        upx=False,
        upx_exclude=[],
        name='WhiteRabbit',
    )
