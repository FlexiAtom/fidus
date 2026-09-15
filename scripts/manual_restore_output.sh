#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Explicit, operator-driven recovery after output mutation uncertainty. This
# never watches a crashed runner and never claims stable output identity.
set -u
set -o pipefail

usage() {
  echo "usage: $0 --allow-output-mutation --output NAME --scale VALUE --transform VALUE [--log PATH]" >&2
}

allow=0
output=
original_scale=
original_transform=
log_path=
while (($#)); do
  case "$1" in
    --allow-output-mutation) allow=1; shift ;;
    --output) (($# >= 2)) || { usage; exit 3; }; output=$2; shift 2 ;;
    --scale) (($# >= 2)) || { usage; exit 3; }; original_scale=$2; shift 2 ;;
    --transform) (($# >= 2)) || { usage; exit 3; }; original_transform=$2; shift 2 ;;
    --log) (($# >= 2)) || { usage; exit 3; }; log_path=$2; shift 2 ;;
    *) usage; exit 3 ;;
  esac
done

[[ "$allow" == 1 && -n "$output" && -n "$original_scale" && -n "$original_transform" ]] || {
  echo 'HarnessError: manual recovery requires explicit authorization, output, scale, and transform' >&2
  exit 3
}
if LC_ALL=C printf '%s\n' "$output$original_scale$original_transform$log_path" | grep -q '[[:cntrl:]]'; then
  echo 'HarnessError: arguments contain control characters' >&2
  exit 3
fi
case "$original_scale" in 1|1.25|1.5|1.75|2) ;; *) echo 'HarnessError: scale is not in the allowlist' >&2; exit 3 ;; esac
case "$original_transform" in normal|90|180|270) ;; *) echo 'HarnessError: transform is not in the allowlist' >&2; exit 3 ;; esac

niri_bin=${FIDUS_NIRI_BIN:-niri}
if [[ -n "$log_path" ]]; then
  : >"$log_path" 2>/dev/null || { echo 'EnvironmentUnavailable: cannot write recovery log' >&2; exit 2; }
  exec > >(tee -a "$log_path") 2>&1
fi
record() { printf '%s\n' "$*"; }
record "MANUAL_RECOVERY version=1 output=$output original_scale=$original_scale original_transform=$original_transform"

read_output() {
  local raw header_count=0 block= line selector in_block=0 matched=0 scale transform
  raw=$("$niri_bin" msg outputs 2>/dev/null) || return 1
  header_count=$(grep -Ec '^Output .* \([^)]*\)$' <<<"$raw")
  [[ "$header_count" -ge 1 ]] || return 1
  block=
  while IFS= read -r line; do
    if [[ "$line" == Output\ * ]]; then
      selector=${line##*\(}; selector=${selector%)}
      in_block=0
      if [[ "$selector" == "$output" ]]; then in_block=1; matched=$((matched + 1)); fi
      continue
    fi
    if [[ "$in_block" == 1 ]]; then block+="$line"$'\n'; fi
  done <<<"$raw"
  [[ "$matched" == 1 && -n "$block" ]] || return 1
  [[ "$(grep -Ec '^  Scale: ' <<<"$block")" == 1 ]] || return 1
  [[ "$(grep -Ec '^  Transform: ' <<<"$block")" == 1 ]] || return 1
  scale=$(sed -n 's/^  Scale: //p' <<<"$block")
  transform=$(sed -n 's/^  Transform: //p' <<<"$block")
  case "$scale" in 1|1.25|1.5|1.75|2) ;; *) return 1 ;; esac
  case "$transform" in
    normal) transform=normal ;;
    '90° counter-clockwise') transform=90 ;;
    '180°') transform=180 ;;
    '270° counter-clockwise') transform=270 ;;
    *) return 1 ;;
  esac
  printf '%s\t%s\n' "$scale" "$transform"
}

before=$(read_output) || {
  record 'RecoveryUnverified: target output is missing, ambiguous, deferred, or malformed'
  exit 2
}
record "before=$before"
scale_rc=0
transform_rc=0
"$niri_bin" msg output "$output" scale "$original_scale" >/dev/null 2>&1 || scale_rc=$?
"$niri_bin" msg output "$output" transform "$original_transform" >/dev/null 2>&1 || transform_rc=$?
after=$(read_output || true)
record "setter_scale_rc=$scale_rc setter_transform_rc=$transform_rc after=${after:-unknown}"
expected="$original_scale"$'\t'"$original_transform"
if [[ "$scale_rc" != 0 || "$transform_rc" != 0 || "$after" != "$expected" ]]; then
  record 'RecoveryUnverified: manual restore was not verified field-by-field'
  exit 4
fi
record 'ManualRecoveryResult recovery=unverified verified_fields=scale,transform note=manual-recovery-never-confirms-identity'
exit 0
