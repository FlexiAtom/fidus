#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Verify a fidus Debian image release artifact without display access.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
archive=${1:-"$root/fidus-live-debian12.tar.zst"}
checksum=${archive}.sha256
[[ -f "$archive" && -f "$checksum" ]] || {
  echo "release artifact or checksum is missing" >&2
  exit 2
}
sha256sum -c "$checksum"
zstd -t "$archive"
tag="fidus-release-verify-$$"
cleanup() { docker image rm "$tag" fidus-live:debian12 >/dev/null 2>&1 || true; }
trap cleanup EXIT
zstd -dc "$archive" | docker load >/dev/null
image_id=$(cat "$root/fidus-live-debian12.image-id")
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
