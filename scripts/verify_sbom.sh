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
# Rebuild to a temporary file and compare package names: generator is deterministic except creation time.
import subprocess, tempfile
with tempfile.NamedTemporaryFile() as f:
    generator = docker.parent / 'scripts' / 'generate_sbom.py'
    subprocess.run([sys.executable, str(generator), '--lock', str(lock), '--dockerfile', str(docker), '--output', f.name], check=True)
    generated=json.loads(pathlib.Path(f.name).read_text())
if {p['name'] for p in generated['packages']} != {p['name'] for p in doc['packages']}: raise SystemExit('SBOM package set is stale')
print(f'SBOM_OK packages={len(doc["packages"])} relationships={len(doc["relationships"])}')
PY
