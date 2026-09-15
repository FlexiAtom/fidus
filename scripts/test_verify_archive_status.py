#!/usr/bin/env python3
import json
import pathlib
import subprocess
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
VERIFY = ROOT / "scripts/verify_archive_status.py"


def run(status, archive):
    return subprocess.run(["python3", str(VERIFY), str(status), str(archive)], capture_output=True, text=True)


def main():
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        archive = root / "sample.tar.zst"
        archive.write_bytes(b"historical archive")
        status = root / "sample.archive-status.json"
        status.write_text(json.dumps({
            "schema_version": 1,
            "artifact": {"filename": archive.name, "sha256": __import__("hashlib").sha256(archive.read_bytes()).hexdigest(), "size_bytes": archive.stat().st_size},
            "status": "historical",
            "current_release_eligible": False,
            "source_binding": {"status": "legacy-unbound", "historical_commit": "old", "historical_tree": "old"},
        }))
        result = run(status, archive)
        assert result.returncode == 0, result.stderr
        data = json.loads(status.read_text())
        data["current_release_eligible"] = True
        status.write_text(json.dumps(data))
        assert run(status, archive).returncode != 0
        data["current_release_eligible"] = False
        data["artifact"]["sha256"] = "0" * 64
        status.write_text(json.dumps(data))
        assert run(status, archive).returncode != 0
    print("ARCHIVE_STATUS_TEST_OK")


if __name__ == "__main__":
    main()
