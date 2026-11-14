# The Rust monitor's copy of the pixel patterns (rust/wr-core/src/patterns_data.rs) matches
# led_mon/patterns.py and the plugins. After changing a pattern, run `python scripts/export_patterns.py`.
import os
import sys

import pytest

pytest.importorskip("numpy")

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts"))
import export_patterns  # noqa: E402


def test_rust_patterns_are_up_to_date():
    try:
        tables = export_patterns.load_tables()
    except SystemExit as e:
        pytest.skip(str(e))
    with open(export_patterns.OUT_FILE, encoding="utf-8") as f:
        saved = f.read()
    assert saved == export_patterns.rust_source(tables), "run python scripts/export_patterns.py"
