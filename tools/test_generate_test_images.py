#!/usr/bin/env python3
"""No-display regression test for deterministic image fixtures."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import generate_test_images as generator  # noqa: E402


def main() -> None:
    manifest_path = ROOT / "tests/fixtures/images/manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["generator"] == "tools/generate_test_images.py"
    assert [item["name"] for item in manifest["images"]] == ["fractal", "texture", "gradient", "periodic"]
    for item in manifest["images"]:
        data = generator.render(item["name"])
        path = ROOT / item["path"]
        assert path.read_bytes() == data, f"fixture differs from generator: {path}"
        digest = hashlib.sha256(data).hexdigest()
        assert digest == item["sha256"], f"manifest hash mismatch: {path}"
        assert item["width"] == generator.WIDTH and item["height"] == generator.HEIGHT
    print(f"deterministic image fixtures: {len(manifest['images'])} verified")


if __name__ == "__main__":
    main()
