#!/usr/bin/env python3
import hashlib
import json
import pathlib
import subprocess
import tempfile

VERIFY = pathlib.Path(__file__).with_name("verify_release_metadata.py")


def main():
    with tempfile.TemporaryDirectory() as d:
        root = pathlib.Path(d)
        archive = root / "image.tar.zst"; archive.write_bytes(b"archive")
        sbom = root / "image.sbom.json"; sbom.write_text('{"spdxVersion":"SPDX-2.3"}\n')
        manifest = root / "manifest.json"
        binding = {"commit": "a" * 40, "tree": "b" * 64, "algorithm": "sha256(path\\0blob-sha256\\0)"}
        manifest_data = {"schema_version": 1, "artifact": {"filename": archive.name, "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(), "size_bytes": archive.stat().st_size}, "image": {"image_id": "sha256:" + "c" * 64}, "source": {"revision_binding": binding}, "provenance": {"sbom": {"filename": sbom.name, "sha256": hashlib.sha256(sbom.read_bytes()).hexdigest()}}}
        manifest.write_text(json.dumps(manifest_data, indent=2) + "\n")
        provenance = root / "provenance.json"
        provenance.write_text(json.dumps({"schema_version": 1, "predicate_type": "https://slsa.dev/provenance/v1", "subject": [{"name": archive.name, "digest": {"sha256": manifest_data["artifact"]["sha256"]}}], "metadata": {"manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(), "archive_size_bytes": archive.stat().st_size, "image_id": manifest_data["image"]["image_id"], "source_revision_binding": binding}}))
        args = ["python3", str(VERIFY), str(manifest), str(provenance), str(archive), str(sbom)]
        assert subprocess.run(args, capture_output=True).returncode == 0
        bad = json.loads(provenance.read_text()); bad["metadata"]["archive_size_bytes"] += 1; provenance.write_text(json.dumps(bad))
        assert subprocess.run(args, capture_output=True).returncode != 0
    print("RELEASE_METADATA_TEST_OK")


if __name__ == "__main__":
    main()
