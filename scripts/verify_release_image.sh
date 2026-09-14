#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Verify a fidus Debian image release artifact without display access.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
archive=${1:-"$root/fidus-live-debian12.tar.zst"}
checksum=${archive}.sha256
manifest=${FIDUS_RELEASE_MANIFEST:-"$root/fidus-live-debian12.manifest.json"}
sbom=${FIDUS_RELEASE_SBOM:-"$root/fidus-live.sbom.spdx.json"}
image_id_file="$root/fidus-live-debian12.image-id"
signature=${FIDUS_RELEASE_SIGNATURE:-"${manifest}.asc"}
attestation=${FIDUS_RELEASE_ATTESTATION:-"$root/fidus-live-debian12.provenance.json"}
attestation_signature=${FIDUS_RELEASE_ATTESTATION_SIGNATURE:-"${attestation}.asc"}
signer_fingerprint=${FIDUS_RELEASE_SIGNER_FINGERPRINT:-}
# Source binding is checked before any artifact is trusted.  A dirty checkout,
# unavailable Git, or missing binding is a hard failure (never a best effort).
python3 "$root/scripts/source_revision.py" --verify-manifest "$manifest" >/dev/null || {
  echo "source revision binding missing, stale, or checkout is not clean" >&2
  exit 1
}
[[ -f "$archive" && -f "$checksum" && -f "$manifest" && -f "$image_id_file" && -f "$sbom" ]] || {
  echo "release artifact, checksum, manifest, or image ID is missing" >&2
  exit 2
}
[[ "$(wc -l <"$checksum")" == 1 ]] || { echo "checksum must contain exactly one record" >&2; exit 2; }
[[ "$(awk '{print NF}' "$checksum")" == 2 ]] || { echo "malformed checksum record" >&2; exit 2; }
checksum_name=$(awk '{print $2}' "$checksum")
[[ "$checksum_name" == "$(basename "$archive")" ]] || {
  echo "checksum names a different artifact" >&2
  exit 2
}
sha256sum -c "$checksum"
zstd -t "$archive"
archive_sha256=$(sha256sum "$archive" | awk '{print $1}')
archive_size=$(stat -c '%s' "$archive")
image_id=$(tr -d '\r\n' <"$image_id_file")
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "malformed image ID" >&2; exit 2; }
grep -q '"schema_version": 1' "$manifest" || { echo "unsupported release manifest" >&2; exit 2; }
grep -q '"filename": "'"$(basename "$archive")"'"' "$manifest" || { echo "manifest artifact mismatch" >&2; exit 2; }
grep -q '"sha256": "'"$archive_sha256"'"' "$manifest" || { echo "manifest hash mismatch" >&2; exit 2; }
grep -q '"size_bytes": '"$archive_size"',' "$manifest" || { echo "manifest size mismatch" >&2; exit 2; }
grep -q '"image_id": "'"$image_id"'"' "$manifest" || { echo "manifest image ID mismatch" >&2; exit 2; }
bash "$root/scripts/verify_sbom.sh" "$sbom" "$root/Cargo.lock" "$root/Dockerfile.live-container" >/dev/null
sbom_sha256=$(sha256sum "$sbom" | awk '{print $1}')
grep -q '"sha256": "'"$sbom_sha256"'"' "$manifest" || { echo "manifest SBOM hash mismatch" >&2; exit 2; }
[[ -f "$signature" && -f "$attestation" && -f "$attestation_signature" ]] || {
  echo "detached manifest signature or provenance attestation is missing" >&2
  exit 2
}
command -v gpg >/dev/null 2>&1 || { echo "gpg is required to verify release signatures" >&2; exit 2; }
verify_signed_file() {
  local file=$1 sig=$2 label=$3 status fingerprint
  status=$(mktemp)
  trap 'rm -f "$status"' RETURN
  gpg --batch --status-fd 1 --verify "$sig" "$file" >"$status" 2>/dev/null || {
    echo "$label signature verification failed" >&2
    exit 1
  }
  fingerprint=$(awk '$2 == "VALIDSIG" {print $3; exit}' "$status")
  [[ -n "$fingerprint" ]] || { echo "$label signature has no valid signer" >&2; exit 1; }
  if [[ -n "$signer_fingerprint" && "$fingerprint" != "$signer_fingerprint" ]]; then
    echo "$label signed by unexpected key: $fingerprint" >&2
    exit 1
  fi
}
verify_signed_file "$manifest" "$signature" manifest
verify_signed_file "$attestation" "$attestation_signature" attestation
python3 - "$attestation" "$manifest" "$archive" "$archive_sha256" "$archive_size" "$image_id" <<'PY'
import hashlib, json, pathlib, sys
att, manifest, archive, digest, size, image_id = sys.argv[1:]
try:
    data = json.loads(pathlib.Path(att).read_text())
    if data.get("schema_version") != 1 or data.get("predicate_type") != "https://slsa.dev/provenance/v1":
        raise ValueError("unsupported provenance schema")
    subject = data.get("subject")
    if not isinstance(subject, list) or not any(s.get("name") == pathlib.Path(archive).name and s.get("digest", {}).get("sha256") == digest for s in subject):
        raise ValueError("archive is not a provenance subject")
    if data.get("metadata", {}).get("manifest_sha256") != hashlib.sha256(pathlib.Path(manifest).read_bytes()).hexdigest():
        raise ValueError("manifest binding mismatch")
    manifest_data = json.loads(pathlib.Path(manifest).read_text())
    expected_source = manifest_data.get("source", {}).get("revision_binding")
    if not isinstance(expected_source, dict) or data.get("metadata", {}).get("source_revision_binding") != expected_source:
        raise ValueError("source revision provenance mismatch")
    if data.get("metadata", {}).get("archive_size_bytes") != int(size) or data.get("metadata", {}).get("image_id") != image_id:
        raise ValueError("artifact metadata mismatch")
except (OSError, ValueError, json.JSONDecodeError) as exc:
    print(f"invalid provenance attestation: {exc}", file=sys.stderr)
    raise SystemExit(1)
PY
tag="fidus-release-verify-$$"
cleanup() { docker image rm "$tag" >/dev/null 2>&1 || true; }
trap cleanup EXIT
zstd -dc "$archive" | docker load >/dev/null
actual_id=$(docker image inspect fidus-live:debian12 --format '{{.Id}}')
docker tag fidus-live:debian12 "$tag"
[[ "$actual_id" == "$image_id" ]] || {
  echo "image ID mismatch: expected $image_id, got $actual_id" >&2
  exit 1
}
entrypoint=$(docker image inspect "$tag" --format '{{json .Config.Entrypoint}}')
[[ "$entrypoint" == '["fidus-test"]' ]] || {
  echo "unexpected entrypoint: $entrypoint" >&2
  exit 1
}
printf '%s\n' \
  'FIDUS_RESULT version=1 kind=environment run_id=release-verify status=ready execution_mode=ci backend=none compositor=none output=none scale=not-measured transform=not-measured' \
  'FIDUS_RESULT version=1 kind=summary run_id=release-verify status=ok execution_mode=ci records_total=1 records_ok=1 records_failed=0' |
  docker run --rm --network=none --cap-drop=ALL --security-opt=no-new-privileges \
    --read-only --tmpfs /tmp:rw,noexec,nosuid,size=16m --user "$(id -u):$(id -g)" \
    -i "$tag" parse >/dev/null
echo 'release image: ok'
