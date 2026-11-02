"""The README part of the virus scan workflow (no network)."""

import os
import sys
from datetime import datetime, timezone

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts"))

import virus_scan  # noqa: E402

SHA = "ab" * 32


def report(**categories):
    results = {}
    for category, engines in categories.items():
        for engine in engines:
            results[engine] = {"category": category}
    stats = {c: len(e) for c, e in categories.items()}
    stats["type-unsupported"] = 5  # engines that couldn't scan aren't counted
    return {"last_analysis_stats": stats, "last_analysis_results": results}


def test_summarise_counts_flagging_engines():
    r = virus_scan.summarise(SHA, report(malicious=["Zeta", "Alpha"], suspicious=["Mid"],
                                         undetected=["U1", "U2"], harmless=["H"]))
    assert r["flagged"] == 3
    assert r["total"] == 6
    assert r["engines"] == ["Alpha", "Mid", "Zeta"]
    assert r["link"] == f"https://www.virustotal.com/gui/file/{SHA}"


def section():
    results = {
        "WhiteRabbit-setup.exe": virus_scan.summarise(SHA, report(undetected=["A", "B"])),
        "WhiteRabbit.exe": virus_scan.summarise(SHA, report(malicious=["Bad"], undetected=["A"])),
    }
    when = datetime(2026, 11, 2, tzinfo=timezone.utc)
    return virus_scan.render("v1.1.0", "https://example.com/r", results, when)


def test_render_shows_each_file():
    text = section()
    assert "2026-11-02" in text
    assert "| `WhiteRabbit-setup.exe` | ✅ 0 of 2 flagged it |" in text
    assert "⚠️ 1 of 2 flagged it (Bad)" in text
    assert text.startswith(virus_scan.START) and text.endswith(virus_scan.END)


def test_update_readme_adds_then_replaces():
    readme = "# Title\n\n### Downloads\n\n- a\n\n## Why one app?\n\nText\n"
    first = virus_scan.update_readme(readme, section())
    assert first.index(virus_scan.START) < first.index("## Why one app?")
    newer = section().replace("v1.1.0", "v1.2.0")
    second = virus_scan.update_readme(first, newer)
    assert second.count(virus_scan.START) == 1
    assert "v1.2.0" in second and "v1.1.0" not in second
    assert second.endswith("Text\n")


def test_real_readme_has_the_markers():
    path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "README.md")
    text = open(path, encoding="utf-8").read()
    assert text.count(virus_scan.START) == 1 and text.count(virus_scan.END) == 1
