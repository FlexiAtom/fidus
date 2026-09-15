#!/usr/bin/env python3
"""Verify a checked-in historical archive status annotation."""
import hashlib
import json
import pathlib
import sys


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("usage: verify_archive_status.py STATUS [ARCHIVE]", file=sys.stderr)
        return 2
    status_path = pathlib.Path(sys.argv[1])
    data = json.loads(status_path.read_text(encoding="utf-8"))
    artifact = data.get("artifact", {})
    archive = pathlib.Path(sys.argv[2]) if len(sys.argv) == 3 else status_path.parent / artifact.get("filename", "")
    if data.get("schema_version") != 1 or data.get("status") != "historical":
        raise ValueError("status annotation is not historical schema 1")
    if data.get("current_release_eligible") is not False:
        raise ValueError("historical annotation must not be release eligible")
    if data.get("source_binding", {}).get("status") != "legacy-unbound":
        raise ValueError("historical annotation must be legacy-unbound")
    name = artifact.get("filename")
    if not isinstance(name, str) or pathlib.Path(name).name != name or not name:
        raise ValueError("invalid artifact filename")
    if archive.name != name:
        raise ValueError("archive filename does not match annotation")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    size = archive.stat().st_size
    if digest != artifact.get("sha256") or size != artifact.get("size_bytes"):
        raise ValueError("historical annotation does not match archive")
    print("ARCHIVE_STATUS_OK status=historical current_release_eligible=false")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"archive status: {exc}", file=sys.stderr)
        raise SystemExit(1)
