#!/usr/bin/env python3
"""Validate native KiCad corpus provenance fields and fixture hashes."""

from __future__ import annotations

import hashlib
from pathlib import Path
import sys
import tomllib


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "manifest.toml"
REQUIRED = {
    "id", "status", "path", "sha256", "source_url", "source_commit",
    "licence", "produced", "header_version", "generator", "purpose",
    "oracle_commands", "expected_inventory", "differences",
}


def fail(message: str) -> None:
    raise AssertionError(message)


def main() -> int:
    data = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    allowed = set(data["schema"]["allowed_licenses"])
    seen: set[str] = set()

    for fixture in data.get("fixtures", []):
        missing = REQUIRED - fixture.keys()
        if missing:
            fail(f"{fixture.get('id', '<unknown>')}: missing {sorted(missing)}")
        if fixture["status"] != "present":
            fail(f"{fixture['id']}: fixture status must be present")
        if fixture["id"] in seen:
            fail(f"duplicate id: {fixture['id']}")
        seen.add(fixture["id"])
        path = ROOT / fixture["path"]
        if not path.is_file():
            fail(f"{fixture['id']}: missing file {fixture['path']}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != fixture["sha256"]:
            fail(f"{fixture['id']}: sha256 is {digest}, manifest has {fixture['sha256']}")
        if fixture["licence"] not in allowed:
            fail(f"{fixture['id']}: licence {fixture['licence']!r} is not allowed")
        if not fixture["oracle_commands"]:
            fail(f"{fixture['id']}: oracle command is required")

    for slot in data.get("slots", []):
        if set(slot) != {"id", "status", "needed"}:
            fail(f"{slot.get('id', '<unknown>')}: pending slot fields must be id, status, needed")
        if slot["status"] != "pending" or not slot["needed"].strip():
            fail(f"{slot['id']}: pending slot must say what is needed")
        if slot["id"] in seen:
            fail(f"duplicate id: {slot['id']}")
        seen.add(slot["id"])

    if not data.get("fixtures") or not data.get("slots"):
        fail("manifest must contain present fixtures and pending slots")
    print(f"validated {len(data['fixtures'])} fixtures and {len(data['slots'])} pending slots")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (AssertionError, KeyError, tomllib.TOMLDecodeError) as error:
        print(f"manifest validation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
