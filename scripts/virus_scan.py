"""Scan a GitHub release's Windows files with VirusTotal and write the results into README.md.

Run by the "Virus scan" workflow (.github/workflows/virus-scan.yml). Needs:
  VIRUSTOTAL_API_KEY  a VirusTotal API key (free account)
  GITHUB_TOKEN        to read the release
  GITHUB_REPOSITORY   owner/name
  RELEASE_TAG         optional, e.g. v1.1.0; empty means the latest release
"""

from __future__ import annotations

import hashlib
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

FILES = ["WhiteRabbit-setup.exe", "WhiteRabbit.exe"]
VT = "https://www.virustotal.com/api/v3"
START = "<!-- virus-scan:start -->"
END = "<!-- virus-scan:end -->"
# Engines that couldn't scan the file don't count towards the total
COUNTED = ("malicious", "suspicious", "undetected", "harmless")
# The free API allows 4 requests a minute
WAIT = 30


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def summarise(sha: str, attributes: dict) -> dict:
    """Pick what the README needs out of a VirusTotal file report."""
    stats = attributes.get("last_analysis_stats", {})
    results = attributes.get("last_analysis_results", {})
    flagged = sorted(
        name for name, r in results.items() if r.get("category") in ("malicious", "suspicious")
    )
    return {
        "flagged": stats.get("malicious", 0) + stats.get("suspicious", 0),
        "total": sum(stats.get(k, 0) for k in COUNTED),
        "engines": flagged,
        "link": f"https://www.virustotal.com/gui/file/{sha}",
    }


def render(tag: str, release_url: str, results: dict[str, dict], when: datetime) -> str:
    """The README section between the markers (markers included)."""
    lines = [
        START,
        "### 🛡 Virus scan",
        "",
        f"[{tag}]({release_url}) was checked with [VirusTotal](https://www.virustotal.com) on "
        f"{when:%Y-%m-%d}. VirusTotal runs each file past about 70 antivirus programs.",
        "",
        "| File | Result | Full report |",
        "| --- | --- | --- |",
    ]
    for name, r in results.items():
        if r["flagged"] == 0:
            result = f"✅ 0 of {r['total']} flagged it"
        else:
            result = f"⚠️ {r['flagged']} of {r['total']} flagged it ({', '.join(r['engines'])})"
        lines.append(f"| `{name}` | {result} | [VirusTotal report]({r['link']}) |")
    lines += [
        "",
        "White Rabbit isn't code-signed yet and is built with PyInstaller, which some antivirus",
        "programs flag on sight, so a few warnings from lesser-known scanners are expected (false",
        "positives). Windows Defender (listed as *Microsoft*) is the one most people will meet.",
        END,
    ]
    return "\n".join(lines)


def update_readme(readme: str, section: str) -> str:
    """Replace the scan section, or add it after the Downloads list the first time."""
    pattern = re.compile(re.escape(START) + r".*?" + re.escape(END), re.S)
    if pattern.search(readme):
        return pattern.sub(lambda _: section, readme, count=1)
    anchor = "\n## Why one app?"
    if anchor in readme:
        return readme.replace(anchor, "\n" + section + "\n" + anchor, 1)
    return readme.rstrip("\n") + "\n\n" + section + "\n"


# ---- network parts (run in the workflow only) ----


def vt_get(session, url: str, tries: int = 10) -> dict:
    """GET from VirusTotal, waiting and retrying when it answers with an error.

    Right after an upload VirusTotal can answer "not found" for a while, and the free
    API answers "quota exceeded" if asked more than 4 times a minute.
    """
    for attempt in range(1, tries + 1):
        response = session.get(url)
        body = response.json() if response.content else {}
        if response.status_code == 200 and "data" in body:
            return body["data"]
        error = body.get("error", {})
        code = error.get("code", response.status_code)
        print(f"  VirusTotal answered {code}: {error.get('message', '')} (try {attempt} of {tries})")
        if code in ("WrongCredentialsError", "AuthenticationRequiredError"):
            sys.exit("VirusTotal didn't accept the API key. Check the VIRUSTOTAL_API_KEY secret.")
        time.sleep(WAIT * 2)
    sys.exit(f"VirusTotal kept answering with errors for {url}, try again later")


def scan(session, path: Path) -> dict:
    sha = sha256(path)
    print(f"{path.name}: sha256 {sha}")
    # Always upload, so the result is fresh even if VirusTotal saw this file before.
    # The upload URL works for any size (the plain endpoint stops at 32 MB).
    response = session.get(f"{VT}/files/upload_url")
    if response.status_code == 401:
        sys.exit("VirusTotal didn't accept the API key. Check the VIRUSTOTAL_API_KEY secret.")
    response.raise_for_status()
    upload_url = response.json()["data"]
    with path.open("rb") as f:
        response = session.post(upload_url, files={"file": (path.name, f)}, timeout=600)
    response.raise_for_status()
    analysis = response.json()["data"]["id"]
    print(f"{path.name}: uploaded, waiting for the scan to finish")
    for _ in range(60):  # up to 30 minutes
        time.sleep(WAIT)
        status = vt_get(session, f"{VT}/analyses/{analysis}")["attributes"]["status"]
        print(f"  scan status: {status}")
        if status == "completed":
            break
    else:
        sys.exit(f"{path.name}: VirusTotal didn't finish scanning in time, try again later")
    time.sleep(WAIT)
    result = summarise(sha, vt_get(session, f"{VT}/files/{sha}")["attributes"])
    print(f"{path.name}: {result['flagged']} of {result['total']} flagged {result['engines']}")
    return result


def main() -> None:
    import requests

    key = os.environ.get("VIRUSTOTAL_API_KEY", "").strip()
    if not key:
        sys.exit(
            "The VIRUSTOTAL_API_KEY secret is missing. Add it under Settings > Secrets and "
            "variables > Actions (see 'Virus scan' in the README)."
        )
    repo = os.environ["GITHUB_REPOSITORY"]
    tag = os.environ.get("RELEASE_TAG", "").strip()
    gh = requests.Session()
    gh.headers.update({
        "Authorization": f"Bearer {os.environ['GITHUB_TOKEN']}",
        "Accept": "application/vnd.github+json",
    })
    url = f"https://api.github.com/repos/{repo}/releases/" + (f"tags/{tag}" if tag else "latest")
    response = gh.get(url)
    if response.status_code == 404:
        sys.exit(f"No release found ({'tag ' + tag if tag else 'latest'}). Publish a release first.")
    response.raise_for_status()
    release = response.json()
    print(f"Scanning release {release['tag_name']}")

    assets = {a["name"]: a for a in release["assets"]}
    missing = [n for n in FILES if n not in assets]
    if missing:
        sys.exit(f"Release {release['tag_name']} has no {', '.join(missing)}")

    vt = requests.Session()
    vt.headers.update({"x-apikey": key})
    results = {}
    folder = Path("scan-downloads")
    folder.mkdir(exist_ok=True)
    for name in FILES:
        path = folder / name
        with gh.get(
            assets[name]["url"], headers={"Accept": "application/octet-stream"}, stream=True
        ) as r:
            r.raise_for_status()
            with path.open("wb") as f:
                for block in r.iter_content(1 << 20):
                    f.write(block)
        results[name] = scan(vt, path)

    section = render(release["tag_name"], release["html_url"], results, datetime.now(timezone.utc))
    readme = Path("README.md")
    readme.write_text(update_readme(readme.read_text(encoding="utf-8"), section), encoding="utf-8")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(section.replace(START, "").replace(END, "") + "\n")


if __name__ == "__main__":
    main()
