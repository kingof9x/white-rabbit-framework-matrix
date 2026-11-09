"""Build the Microsoft Store package (WhiteRabbit.msix) from the one-dir build in dist/WhiteRabbit.

    python scripts/build_msix.py --version 1.2.0                 lay out build/msix and pack dist/WhiteRabbit.msix
    python scripts/build_msix.py --version 1.2.0 --layout-only   just lay out build/msix (works on Linux too)

The Store signs the package itself, so this doesn't sign it. The package's identity must match the app's
Partner Center page (Product management > Product identity). It comes from the options below, or the
environment variables MSIX_IDENTITY_NAME, MSIX_PUBLISHER and MSIX_PUBLISHER_DISPLAY_NAME (CI sets them from
the repository's Actions variables). Without them, stand-in values make a package for testing only.
"""

import argparse
import glob
import os
import re
import shutil
import subprocess
import sys
from xml.sax.saxutils import escape

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TEMPLATE = os.path.join(ROOT, "installer", "msix", "AppxManifest.xml")

# Stand-ins for a test package; the Store refuses these, its real values come from Partner Center
TEST_IDENTITY_NAME = "WhiteRabbit.Test"
TEST_PUBLISHER = "CN=White Rabbit Test"
TEST_PUBLISHER_DISPLAY_NAME = "kingof9x"

# (file, width, height): the tile and logo images the manifest names, plus the sizes Windows asks for
# the taskbar, Start and Settings (unplated = shown without a coloured plate behind it)
ASSETS = [
    ("StoreLogo.png", 50, 50),
    ("StoreLogo.scale-200.png", 100, 100),
    ("Square150x150Logo.png", 150, 150),
    ("Square150x150Logo.scale-200.png", 300, 300),
    ("Wide310x150Logo.png", 310, 150),
    ("Wide310x150Logo.scale-200.png", 620, 300),
    ("Square44x44Logo.png", 44, 44),
    ("Square44x44Logo.scale-200.png", 88, 88),
]
for _size in (16, 24, 32, 48, 256):
    ASSETS.append((f"Square44x44Logo.targetsize-{_size}.png", _size, _size))
    ASSETS.append((f"Square44x44Logo.targetsize-{_size}_altform-unplated.png", _size, _size))


def msix_version(version):
    """The package version: four numbers, the last one 0 (the Store keeps that one for itself).

    "1.2.0" -> "1.2.0.0", "v1.2" -> "1.2.0.0", "1.2.0-beta" -> "1.2.0.0".
    """
    numbers = [int(n) for n in re.findall(r"\d+", version or "")][:3]
    if not numbers:
        raise ValueError(f"No version numbers in {version!r}")
    numbers += [0] * (3 - len(numbers))
    if numbers[0] == 0:
        raise ValueError(f"The Store doesn't take a package version starting with 0: {version!r}")
    if any(n > 65535 for n in numbers):
        raise ValueError(f"Package version numbers go up to 65535: {version!r}")
    return ".".join(str(n) for n in numbers + [0])


def identity(identity_name=None, publisher=None, publisher_display_name=None, environ=None):
    """The package identity: the given values, else the MSIX_* environment variables, else test stand-ins."""
    environ = os.environ if environ is None else environ
    values = {
        "identity_name": identity_name or environ.get("MSIX_IDENTITY_NAME", "").strip() or TEST_IDENTITY_NAME,
        "publisher": publisher or environ.get("MSIX_PUBLISHER", "").strip() or TEST_PUBLISHER,
        "publisher_display_name": (publisher_display_name or environ.get("MSIX_PUBLISHER_DISPLAY_NAME", "").strip()
                                   or TEST_PUBLISHER_DISPLAY_NAME),
    }
    if not re.fullmatch(r"[A-Za-z0-9.\-]{3,50}", values["identity_name"]):
        raise ValueError(f"Not a package name (letters, digits, dots and dashes): {values['identity_name']!r}")
    if not values["publisher"].startswith("CN="):
        raise ValueError(f"The publisher should start with CN= (copy it from Partner Center): {values['publisher']!r}")
    return values


def is_test_identity(values):
    return values["identity_name"] == TEST_IDENTITY_NAME or values["publisher"] == TEST_PUBLISHER


def render_manifest(version, values, template=TEMPLATE):
    with open(template, encoding="utf-8") as f:
        text = f.read()
    fields = dict(values, version=msix_version(version))
    for key, value in fields.items():
        text = text.replace("{" + key + "}", escape(value, {'"': "&quot;"}))
    left = re.findall(r'="\{[a-z_]+\}"|>\{[a-z_]+\}<', text)
    if left:
        raise ValueError(f"Manifest placeholders not filled in: {left}")
    return text


def make_assets(folder):
    """Draw the logos from the same picture as the exe and tray icon (two lit panels), on transparent."""
    sys.path.insert(0, ROOT)
    from PIL import Image
    from led_mon.tray import make_icon_image
    os.makedirs(folder, exist_ok=True)
    for name, width, height in ASSETS:
        side = min(width, height)
        # Tiles get some space around the picture; small taskbar icons use the whole square
        rabbit = side if side <= 48 else int(side * 0.66)
        icon = make_icon_image(rabbit)
        image = Image.new("RGBA", (width, height), (0, 0, 0, 0))
        image.paste(icon, ((width - rabbit) // 2, (height - rabbit) // 2), icon)
        image.save(os.path.join(folder, name))


def lay_out(dist_dir, layout_dir, version, values):
    """Copy the one-dir build, the manifest and the logos into layout_dir (the package's contents)."""
    if not os.path.isfile(os.path.join(dist_dir, "WhiteRabbit.exe")) and sys.platform == "win32":
        raise FileNotFoundError(f"No WhiteRabbit.exe in {dist_dir}: build the one-dir app first")
    if os.path.exists(layout_dir):
        shutil.rmtree(layout_dir)
    if os.path.isdir(dist_dir):
        shutil.copytree(dist_dir, layout_dir)
    else:
        os.makedirs(layout_dir)
    with open(os.path.join(layout_dir, "AppxManifest.xml"), "w", encoding="utf-8") as f:
        f.write(render_manifest(version, values))
    make_assets(os.path.join(layout_dir, "Assets"))


def windows_sdk_tool(name):
    """makeappx.exe / makepri.exe from the newest installed Windows 10/11 SDK."""
    base = os.path.join(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"), "Windows Kits", "10", "bin")
    found = glob.glob(os.path.join(base, "10.*", "x64", name))
    if not found:
        raise FileNotFoundError(f"{name} not found under {base}: install the Windows SDK")
    return sorted(found, key=lambda p: [int(n) for n in re.findall(r"\d+", os.path.basename(os.path.dirname(os.path.dirname(p))))])[-1]


def run(command):
    print(">", " ".join(command), flush=True)
    subprocess.run(command, check=True)


def pack(layout_dir, output):
    """Index the logos (resources.pri, so Windows picks the right size) and pack the folder into an .msix."""
    makepri = windows_sdk_tool("makepri.exe")
    config = os.path.join(os.path.dirname(layout_dir), "priconfig.xml")
    run([makepri, "createconfig", "/cf", config, "/dq", "en-US", "/pv", "10.0.0", "/o"])
    run([makepri, "new", "/pr", layout_dir, "/cf", config, "/mn", os.path.join(layout_dir, "AppxManifest.xml"),
         "/of", os.path.join(layout_dir, "resources.pri"), "/o"])
    if os.path.exists(output):
        os.remove(output)
    run([windows_sdk_tool("makeappx.exe"), "pack", "/d", layout_dir, "/p", output, "/o"])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--version", required=True, help="the app version, e.g. 1.2.0")
    parser.add_argument("--dist", default=os.path.join(ROOT, "dist", "WhiteRabbit"), help="the one-dir build")
    parser.add_argument("--layout", default=os.path.join(ROOT, "build", "msix", "layout"))
    parser.add_argument("--output", default=os.path.join(ROOT, "dist", "WhiteRabbit.msix"))
    parser.add_argument("--identity-name")
    parser.add_argument("--publisher")
    parser.add_argument("--publisher-display-name")
    parser.add_argument("--layout-only", action="store_true", help="don't pack (no Windows SDK needed)")
    args = parser.parse_args(argv)

    values = identity(args.identity_name, args.publisher, args.publisher_display_name)
    if is_test_identity(values):
        print("Using stand-in package identity values: this package is for testing, not for the Store.")
    print(f"Package {values['identity_name']} {msix_version(args.version)} by {values['publisher']}")
    lay_out(args.dist, args.layout, args.version, values)
    if not args.layout_only:
        pack(args.layout, args.output)
        print(f"Made {args.output}")


if __name__ == "__main__":
    main()
