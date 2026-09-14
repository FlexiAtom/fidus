#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Verify an offline declaration-only SPDX SBOM; never performs package lookup.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
sbom=${1:-"$root/fidus-live.sbom.spdx.json"}
lock=${2:-"$root/Cargo.lock"}
dockerfile=${3:-"$root/Dockerfile.live-container"}
[[ -f "$sbom" && -f "$lock" && -f "$dockerfile" ]] || { echo 'SBOM, Cargo.lock, or Dockerfile missing' >&2; exit 2; }
python3 - "$sbom" "$lock" "$dockerfile" <<'PY'
import json, pathlib, sys
sbom, lock, docker = map(pathlib.Path, sys.argv[1:])
doc=json.loads(sbom.read_text())
if doc.get('spdxVersion') != 'SPDX-2.3' or doc.get('SPDXID') != 'SPDXRef-DOCUMENT': raise SystemExit('unsupported SPDX document')
ids={p.get('SPDXID') for p in doc.get('packages', [])}
if None in ids or len(ids) != len(doc['packages']): raise SystemExit('duplicate or missing package SPDXID')
for r in doc.get('relationships', []):
    if r.get('spdxElementId') not in ids | {'SPDXRef-DOCUMENT'} or r.get('relatedSpdxElement') not in ids: raise SystemExit('relationship references unknown package')
if 'NOASSERTION' not in sbom.read_text(): raise SystemExit('SBOM must state unresolved offline metadata explicitly')
# Rebuild and compare all reproducible package metadata. Creation time is
# intentionally variable, but names alone are too weak: a stale version or
# source can otherwise pass verification while describing another dependency.
import subprocess, tempfile
with tempfile.NamedTemporaryFile() as f:
    generator = docker.parent / 'scripts' / 'generate_sbom.py'
    subprocess.run([sys.executable, str(generator), '--lock', str(lock), '--dockerfile', str(docker), '--output', f.name], check=True)
    generated=json.loads(pathlib.Path(f.name).read_text())
def package_signature(p):
    checksums=tuple(sorted((c.get('algorithm'), c.get('checksumValue')) for c in p.get('checksums', [])))
    return (p.get('SPDXID'), p.get('name'), p.get('versionInfo'), p.get('downloadLocation'), checksums)
actual={package_signature(p) for p in doc['packages']}
expected={package_signature(p) for p in generated['packages']}
if actual != expected:
    raise SystemExit('SBOM package metadata is stale (name/version/source/checksum mismatch)')
actual_rel={(r.get('spdxElementId'), r.get('relationshipType'), r.get('relatedSpdxElement')) for r in doc['relationships']}
expected_rel={(r.get('spdxElementId'), r.get('relationshipType'), r.get('relatedSpdxElement')) for r in generated['relationships']}
if actual_rel != expected_rel:
    raise SystemExit('SBOM dependency relationships are stale')
print(f'SBOM_OK packages={len(doc["packages"])} relationships={len(doc["relationships"])}')
PY
