#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Read-only evidence probe for P5. It never changes output configuration.
set -euo pipefail

command -v niri >/dev/null || {
  echo "EnvironmentUnavailable: niri is not installed" >&2
  exit 2
}

help=$(niri msg output --help)
for action in scale transform; do
  grep -Eq "^[[:space:]]+$action[[:space:]]" <<<"$help" || {
    echo "EnvironmentUnavailable: niri lacks output $action control" >&2
    exit 2
  }
done

outputs=$(niri msg outputs)
grep -q '^Output ' <<<"$outputs" || {
  echo "EnvironmentUnavailable: no output record" >&2
  exit 2
}
grep -q '^[[:space:]]*Scale:' <<<"$outputs" || {
  echo "EnvironmentUnavailable: output scale is unreadable" >&2
  exit 2
}
grep -q '^[[:space:]]*Transform:' <<<"$outputs" || {
  echo "EnvironmentUnavailable: output transform is unreadable" >&2
  exit 2
}

printf '%s\n' "$outputs"
printf '%s\n' 'P5 read-only output probe: control=scale,transform state=readable'
