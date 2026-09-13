#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Verify a fidus Debian image release artifact without display access.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
archive=${1:-"$root/fidus-live-debian12.tar.zst"}
checksum=${archive}.sha256
manifest=${FIDUS_RELEASE_MANIFEST:-"$root/fidus-live-debian12.manifest.json"}
image_id_file="$root/fidus-live-debian12.image-id"
[[ -f "$archive" && -f "$checksum" && -f "$manifest" && -f "$image_id_file" ]] || {
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
