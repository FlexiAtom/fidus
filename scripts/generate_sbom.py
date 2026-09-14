#!/usr/bin/env python3
"""Generate a declaration-only SPDX 2.3 SBOM without network access.

The lockfile is authoritative for Rust packages. Debian entries are the
runtime package names declared in Dockerfile.live-container; apt metadata is
not available offline, so their versions and hashes are deliberately
NOASSERTION rather than invented.
"""
import argparse
import hashlib
import json
import re
from datetime import datetime, timezone
from pathlib import Path
import os


def lock_packages(text):
    result = []
    for block in re.split(r"\n(?=\[\[package\]\])", text):
        if not block.lstrip().startswith("[[package]]"):
            continue
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        version = re.search(r'^version = "([^"]+)"', block, re.M)
        if not name or not version:
            continue
        source = re.search(r'^source = "([^"]+)"', block, re.M)
        checksum = re.search(r'^checksum = "([^"]+)"', block, re.M)
        deps = re.search(r'^dependencies = \[\n(.*?)^\]', block, re.M | re.S)
        dep_names = re.findall(r'^\s*"([^"]+)', deps.group(1), re.M) if deps else []
        result.append({"name": name.group(1), "version": version.group(1),
                       "source": source.group(1) if source else None,
                       "checksum": checksum.group(1) if checksum else None,
                       "deps": dep_names})
    return result


def runtime_packages(text):
    # Keep this intentionally narrow: only the final runtime stage is SBOM'd.
    stage = text.split("FROM ${DEBIAN_IMAGE}@${DEBIAN_IMAGE_DIGEST}", 1)[-1]
    start = stage.find("apt-get install --yes --no-install-recommends")
    end = stage.find("&& rm -rf /var/lib/apt/lists", start)
    if start < 0 or end < 0:
        raise SystemExit("runtime apt declaration not found")
    packages = []
    for line in stage[start:end].splitlines()[1:]:

        token = line.strip().rstrip('\\')
        if re.fullmatch(r'[a-z0-9][a-z0-9+.-]*', token):
            packages.append(token)
    return packages
    return re.findall(r"(?m)^\s+([a-z0-9][a-z0-9+.-]*)(?:\\s+\\\\)?\\s*$", match.group(1))


def sid(name, version):
    safe = re.sub(r"[^A-Za-z0-9.-]", "-", f"{name}-{version}")
    return "SPDXRef-Package-" + safe


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lock", default="Cargo.lock")
    ap.add_argument("--dockerfile", default="Dockerfile.live-container")
    ap.add_argument("--output", required=True)
    args = ap.parse_args()
    lock_path, docker_path = Path(args.lock), Path(args.dockerfile)
    lock = lock_path.read_text(encoding="utf-8")
    docker = docker_path.read_text(encoding="utf-8")
    rust = lock_packages(lock)
    if not rust:
        raise SystemExit("Cargo.lock contains no packages")
    deb = runtime_packages(docker)
    lock_hash = hashlib.sha256(lock.encode()).hexdigest()
    packages = []
    relationships = []
    # Reproducibility: callers may pin the timestamp; otherwise the generated
    # document records the actual generation time and verification compares only
    # the dependency/package set, not this intentionally variable metadata.
    created = os.environ.get("SOURCE_DATE_EPOCH")
    if created is not None:
        created = datetime.fromtimestamp(int(created), timezone.utc).isoformat().replace("+00:00", "Z")
    else:
        created = datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")
    root_id = "SPDXRef-Application-fidus-live"
    packages.append({"SPDXID": root_id, "name": "fidus-live", "versionInfo": "0.1.0-beta.1",
                     "downloadLocation": "NOASSERTION", "filesAnalyzed": False,
                     "licenseConcluded": "Apache-2.0", "licenseDeclared": "Apache-2.0"})
    by_name = {}
    for p in rust:
        pid = sid(p["name"], p["version"])
        by_name.setdefault(p["name"], []).append(pid)
        item = {"SPDXID": pid, "name": p["name"], "versionInfo": p["version"],
                "downloadLocation": p["source"] or "NOASSERTION", "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION"}
        if p["checksum"]:
            item["checksums"] = [{"algorithm": "SHA256", "checksumValue": p["checksum"]}]
        packages.append(item)
        relationships.append({"spdxElementId": root_id, "relationshipType": "DEPENDS_ON", "relatedSpdxElement": pid})
    for p in rust:
        parent = sid(p["name"], p["version"])
        for dep in p["deps"]:
            candidates = by_name.get(dep.split(" ", 1)[0], [])
            # A lock dependency can have a version qualifier. Resolve only an
            # unambiguous name; otherwise retain an explicit unresolved edge.
            if len(candidates) == 1:
                relationships.append({"spdxElementId": parent, "relationshipType": "DEPENDS_ON", "relatedSpdxElement": candidates[0]})
    for name in deb:
        pid = sid("debian-" + name, "NOASSERTION")
        packages.append({"SPDXID": pid, "name": name, "versionInfo": "NOASSERTION",
                         "downloadLocation": "NOASSERTION", "filesAnalyzed": False,
                         "licenseConcluded": "NOASSERTION", "licenseDeclared": "NOASSERTION",
                         "annotations": [{"annotationType": "OTHER", "annotator": "Tool: fidus-sbom",
                                          "annotationDate": created,
                                          "comment": "Runtime package declared by Dockerfile.live-container; version not resolved offline."}]})
        relationships.append({"spdxElementId": root_id, "relationshipType": "DEPENDS_ON", "relatedSpdxElement": pid})
    doc = {"spdxVersion": "SPDX-2.3", "dataLicense": "CC0-1.0",
           "SPDXID": "SPDXRef-DOCUMENT", "name": "fidus-live-sbom",
           "documentNamespace": "https://fidus.invalid/sbom/" + lock_hash,
           "creationInfo": {"created": created, "creators": ["Tool: fidus-sbom (offline)"]},
           "packages": packages, "relationships": relationships,
           "comment": "Generated from Cargo.lock and the final runtime apt declaration. No network lookup or unobserved version is implied."}
    out = Path(args.output)
    out.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
