#!/usr/bin/env python3
"""Compute and verify a release's source revision binding.

The digest is deliberately independent of Git's platform-dependent checkout
metadata: it is SHA-256 over sorted ``path NUL blob-sha256 NUL`` records.
Release outputs are excluded because signing updates them after the source
snapshot is taken.  A dirty or untracked checkout is never an acceptable
source of release provenance.
"""
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
EXCLUDED = {
    "fidus-live-debian12.tar.zst", "fidus-live-debian12.tar.zst.sha256",
    "fidus-live-debian12.image-id", "fidus-live-debian12.manifest.json",
    "fidus-live-debian12.manifest.json.asc", "fidus-live-debian12.provenance.json",
    "fidus-live-debian12.provenance.json.asc", "fidus-live.sbom.spdx.json",
}

def git(*args: str) -> bytes:
    return subprocess.check_output(["git", *args], cwd=ROOT)

def snapshot() -> dict:
    status_lines = git("status", "--porcelain=v1", "--untracked-files=all").decode().splitlines()
    # Release metadata is intentionally rewritten by sign_release; it is not
    # source. Any other modified/untracked path fails closed.
    source_changes = []
    for line in status_lines:
        path = line[3:].split(" -> ", 1)[-1]
        if path not in EXCLUDED:
            source_changes.append(path)
    if source_changes:
        raise RuntimeError("source checkout is dirty or contains untracked files: " + ", ".join(source_changes))
    head = git("rev-parse", "HEAD").decode().strip()
    paths = [p.decode() for p in git("ls-files", "-z").split(b"\0") if p]
    records = []
    for name in sorted(p for p in paths if p not in EXCLUDED):
        data = (ROOT / name).read_bytes()
        blob = hashlib.sha256(data).hexdigest()
        records.append((name, blob))
    h = hashlib.sha256()
    for name, blob in records:
        h.update(name.encode("utf-8")); h.update(b"\0")
        h.update(blob.encode("ascii")); h.update(b"\0")
    return {"commit": head, "tree": h.hexdigest(), "file_count": len(records), "algorithm": "sha256(path\\0blob-sha256\\0)"}

def main() -> int:
    try:
        current = snapshot()
        if len(sys.argv) == 2 and sys.argv[1] == "--json":
            print(json.dumps(current, sort_keys=True))
            return 0
        if len(sys.argv) == 3 and sys.argv[1] == "--verify-manifest":
            data = json.loads(pathlib.Path(sys.argv[2]).read_text())
            binding = data.get("source", {}).get("revision_binding")
            if not isinstance(binding, dict):
                raise RuntimeError("manifest has no source revision binding")
            for key in ("commit", "tree", "algorithm"):
                if binding.get(key) != current[key]:
                    raise RuntimeError(f"source {key} mismatch")
            print("source revision binding: ok")
            return 0
        print(json.dumps(current, indent=2, sort_keys=True))
        return 0
    except (OSError, subprocess.CalledProcessError, RuntimeError, json.JSONDecodeError) as exc:
        print(f"source revision binding: {exc}", file=sys.stderr)
        return 1

if __name__ == "__main__":
    raise SystemExit(main())
