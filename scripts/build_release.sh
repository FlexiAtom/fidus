#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Build a release image only from a clean, source-bound checkout.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/../fidus-release-rebuild"}
mkdir -p "$out"
out=$(cd "$out" && pwd)
[[ "$out" != "$root" ]] || { echo "output directory must be outside source tree" >&2; exit 2; }
for tool in docker zstd sha256sum stat python3; do
  command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 2; }
done
# Capture the binding before Docker sees the context. Never build from a dirty
# tree: a successful image must not be attributed to an unreviewed checkout.
binding=$(python3 "$root/scripts/source_revision.py" --json)
commit=$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["commit"])' "$binding")
tag="fidus-live:rebuild-${commit:0:12}"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/fidus-release.XXXXXX")
cleanup() { rm -rf "$tmp"; docker image rm "$tag" >/dev/null 2>&1 || true; }
trap cleanup EXIT

build_network=${FIDUS_DOCKER_BUILD_NETWORK:-host}
[[ "$build_network" == host || "$build_network" == none ]] || {
  echo "FIDUS_DOCKER_BUILD_NETWORK must be host or none" >&2
  exit 2
}
# Build network access is explicit and scoped to image construction; runtime
# verification remains network-isolated.
docker build --network="$build_network" --pull -f "$root/Dockerfile.live-container" -t "$tag" "$root"
docker save "$tag" | zstd -T0 -q -o "$tmp/fidus-live-debian12.tar.zst"
image_id=$(docker image inspect "$tag" --format '{{.Id}}')
python3 "$root/scripts/generate_sbom.py" --lock "$root/Cargo.lock" --dockerfile "$root/Dockerfile.live-container" --output "$tmp/fidus-live.sbom.spdx.json"
sbom_sha256=$(sha256sum "$tmp/fidus-live.sbom.spdx.json" | awk '{print $1}')
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]] || { echo "malformed image ID" >&2; exit 1; }
archive_sha256=$(sha256sum "$tmp/fidus-live-debian12.tar.zst" | awk '{print $1}')
archive_size=$(stat -c '%s' "$tmp/fidus-live-debian12.tar.zst")
python3 - "$tmp/fidus-live-debian12.manifest.json" "$binding" "$archive_sha256" "$archive_size" "$image_id" "$sbom_sha256" "$tag" <<'PY'
import json, pathlib, sys
out, binding, digest, size, image_id, sbom_digest, image_ref = sys.argv[1:]
# This is a new manifest in an isolated output directory; the historical
# manifest in the repository is never edited or retroactively rebound.
pathlib.Path(out).write_text(json.dumps({
  "schema_version": 1,
  "artifact": {"filename": "fidus-live-debian12.tar.zst", "size_bytes": int(size), "sha256": digest, "compression": "zstd"},
  "image": {"reference": image_ref, "image_id": image_id, "entrypoint": ["fidus-test"]},
  "source": {"revision_binding": json.loads(binding)},
  "provenance": {"signing_required": True, "sbom": {"filename": "fidus-live.sbom.spdx.json", "sha256": sbom_digest}}
}, indent=2) + "\n")
PY
printf '%s  %s\n' "$archive_sha256" "fidus-live-debian12.tar.zst" > "$tmp/fidus-live-debian12.tar.zst.sha256"
cp "$tmp"/fidus-live-debian12.tar.zst* "$out/"
cp "$tmp/fidus-live.sbom.spdx.json" "$out/"
printf '%s\n' "$image_id" > "$out/fidus-live-debian12.image-id"
cp "$tmp/fidus-live-debian12.manifest.json" "$out/"
printf 'rebuilt output=%s source=%s image=%s\n' "$out" "$commit" "$image_id"
