#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Validate the machine-readable P5 evidence boundary without upgrading pending evidence.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
file="$root/docs/measurements/p5-f1-f42-matrix.tsv"
[[ -f "$file" ]] || { echo "matrix missing: $file" >&2; exit 2; }

awk -F '\t' '
NR == 1 {
  if ($1 != "id" || $2 != "status" || $3 != "evidence_kind" || $4 != "command" || $5 != "spec_ref") exit 2
  next
}
{
  if (NF != 5 || $1 !~ /^[0-9]+$/ || $1 != NR-1) exit 3
  if ($2 != "pass" && $2 != "real-pending" && $2 != "blocked") exit 4
  if ($3 != "fake" && $3 != "protocol" && $3 != "host" && $3 != "compositor") exit 5
  if ($2 == "real-pending" && $3 != "host" && $3 != "compositor") exit 6
  if ($2 == "pass" && ($4 == "manual-authorized-real-desktop" || $3 == "host" || $3 == "compositor")) exit 7
  count++
}
END { if (count != 42) exit 8 }
' "$file" || {
  echo "invalid P5 F1-F42 matrix (expected IDs 1..42 and explicit evidence boundaries)" >&2
  exit 1
}

pending=$(awk -F '\t' 'NR > 1 && $2 == "real-pending" { n++ } END { print n+0 }' "$file")
passed=$(awk -F '\t' 'NR > 1 && $2 == "pass" { n++ } END { print n+0 }' "$file")
printf 'P5_MATRIX version=1 pass=%s real_pending=%s blocked=0\n' "$passed" "$pending"
