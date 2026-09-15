#!/usr/bin/env python3
"""Verify release metadata bindings without Docker or signature operations."""
import hashlib
import json
import pathlib
import sys


def fail(message):
    raise ValueError(message)


def main():
    if len(sys.argv) != 5:
        print("usage: verify_release_metadata.py MANIFEST PROVENANCE ARCHIVE SBOM", file=sys.stderr)
        return 2
    manifest_path, provenance_path, archive_path, sbom_path = map(pathlib.Path, sys.argv[1:])
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    artifact = manifest.get("artifact", {})
    if manifest.get("schema_version") != 1:
        fail("unsupported manifest schema")
    if artifact.get("filename") != archive_path.name:
        fail("manifest artifact filename mismatch")
    archive_digest = hashlib.sha256(archive_path.read_bytes()).hexdigest()
    archive_size = archive_path.stat().st_size
    if artifact.get("sha256") != archive_digest or artifact.get("size_bytes") != archive_size:
        fail("manifest artifact hash or size mismatch")
    sbom = manifest.get("provenance", {}).get("sbom", {})
    if sbom.get("filename") != sbom_path.name:
        fail("manifest SBOM filename mismatch")
    if sbom.get("sha256") != hashlib.sha256(sbom_path.read_bytes()).hexdigest():
        fail("manifest SBOM hash mismatch")
    binding = manifest.get("source", {}).get("revision_binding")
    if not isinstance(binding, dict) or not all(isinstance(binding.get(k), str) for k in ("commit", "tree", "algorithm")):
        fail("manifest has no current source revision binding")
    if provenance.get("schema_version") != 1 or provenance.get("predicate_type") != "https://slsa.dev/provenance/v1":
        fail("unsupported provenance schema")
    subjects = provenance.get("subject")
    if not isinstance(subjects, list) or not any(s.get("name") == archive_path.name and s.get("digest", {}).get("sha256") == archive_digest for s in subjects):
        fail("provenance subject does not bind archive")
    metadata = provenance.get("metadata", {})
    if metadata.get("manifest_sha256") != hashlib.sha256(manifest_path.read_bytes()).hexdigest():
        fail("provenance manifest hash mismatch")
    if metadata.get("archive_size_bytes") != archive_size or metadata.get("source_revision_binding") != binding:
        fail("provenance source or archive metadata mismatch")
    if metadata.get("image_id") != manifest.get("image", {}).get("image_id"):
        fail("provenance image ID mismatch")
    print("RELEASE_METADATA_OK current-bound artifact=1 sbom=1 provenance=1")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"release metadata: {exc}", file=sys.stderr)
        raise SystemExit(1)
