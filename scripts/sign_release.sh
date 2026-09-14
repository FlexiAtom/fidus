#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Create a locally verifiable release manifest signature and SLSA provenance.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
archive=${1:-"$root/fidus-live-debian12.tar.zst"}
manifest=${FIDUS_RELEASE_MANIFEST:-"$root/fidus-live-debian12.manifest.json"}
key=${FIDUS_RELEASE_SIGNING_KEY:-}
signature="${manifest}.asc"
attestation=${FIDUS_RELEASE_ATTESTATION:-"$root/fidus-live-debian12.provenance.json"}
attestation_signature="${attestation}.asc"
[[ -n "$key" ]] || { echo 'set FIDUS_RELEASE_SIGNING_KEY to a GPG key fingerprint or email' >&2; exit 2; }
for tool in gpg sha256sum stat python3; do command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }; done
[[ -f "$archive" && -f "$manifest" ]] || { echo 'archive or manifest is missing' >&2; exit 2; }
# Refuse provenance from a dirty checkout: the binding must describe exactly the
# source used by the build, not a later or fabricated claim.
source_binding=$(python3 "$root/scripts/source_revision.py" --json)
archive_sha256=$(sha256sum "$archive" | awk '{print $1}')
archive_size=$(stat -c '%s' "$archive")
# Signing may add provenance fields, but it must never retag a different archive
# or silently replace an existing source claim. The artifact bytes and source
# binding must have been established by the build step first.
python3 - "$manifest" "$archive_sha256" "$archive_size" "$source_binding" <<'PY'
import json, pathlib, sys
manifest, digest, size, binding = sys.argv[1:]
data = json.loads(pathlib.Path(manifest).read_text())
artifact = data.get("artifact", {})
if artifact.get("sha256") != digest or int(artifact.get("size_bytes", -1)) != int(size):
    raise SystemExit("manifest artifact does not match archive; refusing to sign")
existing = data.get("source", {}).get("revision_binding")
if not isinstance(existing, dict):
    raise SystemExit("manifest has no build-time source binding; refusing to sign historical artifact")
if existing != json.loads(binding):
    raise SystemExit("manifest source binding differs from current clean source; refusing to rebind")
PY
key_fingerprint=$(gpg --batch --with-colons --list-keys "$key" | awk -F: '$1 == "fpr" {print $10; exit}')
[[ "$key_fingerprint" =~ ^[0-9A-Fa-f]{40}$ ]] || { echo 'signing key must resolve to an OpenPGP fingerprint' >&2; exit 2; }
# Keep the manifest's provenance pointers explicit, then sign that exact byte sequence.
python3 - "$manifest" "$signature" "$attestation" "$key_fingerprint" "$source_binding" <<'PY'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1]); data = json.loads(p.read_text())
prov = data.setdefault("provenance", {})
prov.update({"signature": pathlib.Path(sys.argv[2]).name,
             "attestation": pathlib.Path(sys.argv[3]).name,
             "signer_fingerprint": sys.argv[4]})
binding = json.loads(sys.argv[5])
data.setdefault("source", {})["revision_binding"] = binding
# Canonical JSON keeps the signed bytes deterministic and prevents stale fields.
p.write_text(json.dumps(data, indent=2, sort_keys=False) + "\n")
PY
manifest_sha256=$(sha256sum "$manifest" | awk '{print $1}')
python3 - "$attestation" "$archive" "$archive_sha256" "$archive_size" "$manifest_sha256" "$key_fingerprint" "$manifest" "$source_binding" <<'PY'
import json, pathlib, sys
out, archive, digest, size, manifest, fingerprint, manifest_path, source_binding = sys.argv[1:]
data = {
  "schema_version": 1,
  "predicate_type": "https://slsa.dev/provenance/v1",
  "subject": [{"name": pathlib.Path(archive).name, "digest": {"sha256": digest}}],
  "predicate": {"buildDefinition": {"buildType": "https://github.com/flexiatom/fidus/release"}},
  "metadata": {"manifest_sha256": manifest, "archive_size_bytes": int(size), "image_id": json.loads(pathlib.Path(manifest_path).read_text())["image"]["image_id"], "signer_fingerprint": fingerprint, "source_revision_binding": json.loads(source_binding)}
}
pathlib.Path(out).write_text(json.dumps(data, indent=2) + "\n")
PY
gpg --batch --yes --local-user "$key_fingerprint" --armor --detach-sign --output "$signature" "$manifest"
gpg --batch --yes --local-user "$key_fingerprint" --armor --detach-sign --output "$attestation_signature" "$attestation"
printf 'signed manifest=%s attestation=%s signer=%s\n' "$signature" "$attestation_signature" "$key_fingerprint"
