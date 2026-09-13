#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Explicit live-host output mutation runner. This is intentionally separate
# from fidus-test and is never used by default test paths.
set -u
set -o pipefail

usage() {
  echo "usage: $0 --allow-output-mutation --output NAME [--scale VALUE] [--transform VALUE] -- command args..." >&2
}

allow=0
output=
requested_scale=
requested_transform=
timeout_seconds=${FIDUS_CHILD_TIMEOUT_SECONDS:-30}
command_args=()
while (($#)); do
  case "$1" in
    --allow-output-mutation) allow=1; shift ;;
    --output) (($# >= 2)) || { usage; exit 3; }; output=$2; shift 2 ;;
    --scale) (($# >= 2)) || { usage; exit 3; }; requested_scale=$2; shift 2 ;;
    --transform) (($# >= 2)) || { usage; exit 3; }; requested_transform=$2; shift 2 ;;
    --timeout-seconds) (($# >= 2)) || { usage; exit 3; }; timeout_seconds=$2; shift 2 ;;
    --) shift; command_args=("$@"); break ;;
    *) usage; exit 3 ;;
  esac
done

[[ "$allow" == 1 && -n "$output" && -n "$requested_scale" && -n "$requested_transform" && ${#command_args[@]} -gt 0 ]] || {
  echo "HarnessError: mutation requires explicit authorization, output, scale, transform, and command" >&2
  exit 3
}

# Parameter validation happens before lock acquisition or compositor access.
if LC_ALL=C printf '%s' "$output" | grep -q '[[:cntrl:]]'; then
  echo "HarnessError: output contains control characters" >&2
  exit 3
fi
case "$requested_scale" in
  1|1.25|1.5|1.75|2) ;;
  *) echo "HarnessError: scale is not in the allowlist" >&2; exit 3 ;;
esac
case "$requested_transform" in
  normal|90|180|270) ;;
  *) echo "HarnessError: transform is not in the allowlist" >&2; exit 3 ;;
esac
[[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] || {
  echo "HarnessError: timeout must be a positive integer" >&2
  exit 3
}

niri_bin=${FIDUS_NIRI_BIN:-niri}
lock_root=${XDG_RUNTIME_DIR:-/tmp}
lock_path="$lock_root/fidus-output-mutation.lock"
mkdir -p "$lock_root" 2>/dev/null || {
  echo "EnvironmentUnavailable: cannot access session lock directory" >&2
  exit 2
}

# Read exactly one connected output block. Niri can return rc=0 for a deferred
# disconnected output, so output text and read-back are both required.
read_output() {
  local raw header_count=0 block scale transform
  raw=$("$niri_bin" msg outputs 2>/dev/null) || return 1
  header_count=$(grep -Ec '^Output .* \([^)]*\)$' <<<"$raw")
  [[ "$header_count" -ge 1 ]] || return 1
  block=
  local in_block=0 matched=0 line selector
  while IFS= read -r line; do
    if [[ "$line" == Output\ * ]]; then
      selector=${line##*\(}
      selector=${selector%\)}
      in_block=0
      if [[ "$selector" == "$output" ]]; then
        in_block=1
        matched=$((matched + 1))
      fi
      continue
    fi
    if [[ "$in_block" == 1 ]]; then
      block+="$line"$'\n'
    fi
  done <<<"$raw"
  [[ "$matched" == 1 && -n "$block" ]] || return 1
  [[ "$(grep -c . <<<"$block")" -gt 0 ]] || return 1
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

# The kernel-held descriptor, not metadata, is the ownership proof.
[[ ! -L "$lock_path" ]] || {
  echo "HarnessError: mutation lock path is a symlink" >&2
  exit 3
}
exec 9>"$lock_path" || { echo "EnvironmentUnavailable: cannot open lock" >&2; exit 2; }
flock -n 9 || { echo "HarnessError: another mutation runner owns the session lock" >&2; exit 3; }

snapshot=$(read_output) || {
  echo "EnvironmentUnavailable: cannot read exactly one connected output" >&2
  flock -u 9; exec 9>&-; exit 2
}
# Install the EXIT cleanup immediately after snapshot. Any later setup failure
# can follow a partial apply or leave a changed output, so it must not bypass
# restoration merely because child bookkeeping was not initialized yet.
state=ReadOriginal
restore_verified=0
restore_attempted=0
summary_emitted=0
child_status=0
child_pid=
child_pgid=
interrupted=0
timeout_hit=0
external_change=0
applied_state=
trap 'interrupted=1; trap - INT TERM; if [[ -n "$child_pgid" ]]; then kill -TERM -- "-$child_pgid" 2>/dev/null || true; fi; [[ "$state" == Running ]] && state=RestoreRequested' INT TERM
trap cleanup EXIT
original_scale=${snapshot%%$'\t'*}
original_transform=${snapshot##*$'\t'}
child_out="$(mktemp "${TMPDIR:-/tmp}/fidus-output-child.XXXXXX")" || {
  echo "EnvironmentUnavailable: cannot create child output file" >&2
  exit 2
}
child_err="$(mktemp "${TMPDIR:-/tmp}/fidus-output-child.XXXXXX")" || {
  rm -f "$child_out"
  echo "EnvironmentUnavailable: cannot create child error file" >&2
  exit 2
}

child_alive() {
  [[ -n "$child_pid" ]] || return 1
  local stat
  stat=$(ps -o stat= -p "$child_pid" 2>/dev/null | tr -d ' ')
  [[ -n "$stat" && "$stat" != Z* ]]
}

stop_child_group() {
  [[ -n "$child_pid" ]] || return 0
  if child_alive; then
    kill -TERM -- "-$child_pgid" 2>/dev/null || kill -TERM "$child_pid" 2>/dev/null || true
    local deadline=$((SECONDS + timeout_seconds))
    while child_alive && ((SECONDS < deadline)); do
      sleep 0.1
    done
    if child_alive; then
      timeout_hit=1
      kill -KILL -- "-$child_pgid" 2>/dev/null || kill -KILL "$child_pid" 2>/dev/null || true
    fi
  fi
  wait "$child_pid" 2>/dev/null || true
  child_pid=
  child_pgid=
}

restore_output() {
  restore_attempted=1
  local current expected actual
  if [[ -n "$applied_state" ]]; then
    current=$(read_output 2>/dev/null || true)
    if [[ "$current" != "$applied_state" ]]; then
      external_change=1
      return 0
    fi
  fi
  "$niri_bin" msg output "$output" scale "$original_scale" >/dev/null 2>&1 || true
  "$niri_bin" msg output "$output" transform "$original_transform" >/dev/null 2>&1 || true
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    current=$(read_output 2>/dev/null || true)
    expected="$original_scale"$'\t'"$original_transform"
    if [[ "$current" == "$expected" ]]; then
      restore_verified=1
      break
    fi
    sleep 0.1
done
}

cleanup() {
  if [[ -n "$child_pid" ]]; then
    stop_child_group
  fi
  # A setter may partially apply before returning non-zero. After snapshot,
  # every exit must attempt restore; otherwise a failed command can leave the
  # user's output changed. A failed read-back remains unverified.
  if [[ "$restore_attempted" == 0 ]]; then
    state=RestoreRequested
    restore_output
  fi
  if [[ "$summary_emitted" == 0 && "$restore_attempted" == 1 ]]; then
    summary_emitted=1
    printf 'FIDUS_RESULT version=1 kind=lifecycle run_id=p5-output status=unverified execution_mode=live-host teardown=%s recovery=unverified\n' \
      "$([[ "$timeout_hit" == 1 ]] && echo unverified || echo confirmed)"
    printf 'FIDUS_RESULT version=1 kind=summary run_id=p5-output status=failed execution_mode=live-host records_total=1 records_ok=0 records_failed=1\n'
    printf 'P5_DIAGNOSTIC timeout=%s external_change=%s\n' "$timeout_hit" "$external_change" >&2
  fi
  rm -f "$child_out" "$child_err"
  flock -u 9 2>/dev/null || true
  exec 9>&-
}
trap 'interrupted=1; trap - INT TERM; if [[ -n "$child_pgid" ]]; then kill -TERM -- "-$child_pgid" 2>/dev/null || true; fi; [[ "$state" == Running ]] && state=RestoreRequested' INT TERM
trap cleanup EXIT

finish() {
  local requested_status=$1
  state=RestoreRequested
  restore_output
  # Cleanup is the sole summary owner. Niri currently exposes only a name
  # selector, so matching fields cannot prove stable identity; cleanup emits
  # unverified and the recovery code dominates any child result.
  exit 4
}

# Apply each field separately: a failed second command does not skip recovery.
if [[ "$interrupted" == 1 ]]; then
  finish 130
fi
"$niri_bin" msg output "$output" scale "$requested_scale" >/dev/null 2>&1 || {
  echo "EnvironmentUnavailable: scale command failed" >&2
  finish 2
}
"$niri_bin" msg output "$output" transform "$requested_transform" >/dev/null 2>&1 || {
  echo "EnvironmentUnavailable: transform command failed" >&2
  finish 2
}
state=AppliedAndReadBack
applied=$(read_output 2>/dev/null || true)
expected_applied="$requested_scale"$'\t'"$requested_transform"
[[ "$applied" == "$expected_applied" ]] || {
  echo "EnvironmentUnavailable: apply read-back mismatch" >&2
  finish 2
}
applied_state="$applied"
state=Running

# setsid creates a fresh session; PGID is read from the child, never guessed
# from the shell job PID. The child command is still caller-controlled.
setsid -- "${command_args[@]}" >"$child_out" 2>"$child_err" &
child_pid=$!
child_pgid=$(ps -o pgid= -p "$child_pid" | tr -d ' ')
if [[ -z "$child_pgid" || ! "$child_pgid" =~ ^[0-9]+$ ]]; then
  echo "HarnessError: cannot determine child process group" >&2
  finish 3
fi
child_deadline=$((SECONDS + timeout_seconds))
while child_alive && ((SECONDS < child_deadline)); do
  sleep 0.1
done
if child_alive; then
  timeout_hit=1
  stop_child_group
  child_status=124
else
  wait "$child_pid" 2>/dev/null; child_status=$?
  child_pid=
  child_pgid=
fi
finish "$child_status"

if [[ "$restore_verified" != 1 ]]; then
  echo "RecoveryUnverified: output fields were not read back to their original values" >&2
  exit 4
fi
# NameOnly is insufficient identity evidence, even when fields match.
echo "FIDUS_RESULT version=1 kind=lifecycle run_id=p5-output status=unverified execution_mode=live-host teardown=unknown recovery=unverified"
exit "$child_status"
