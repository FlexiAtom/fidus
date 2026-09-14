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

# Open the lock without following a symlink. The shell's redirection cannot
# express O_NOFOLLOW, so reserve the pathname atomically with mkdir and keep a
# regular lock file inside that private directory. A stale directory is treated
# as busy rather than deleted: guessing stale ownership could steal an active
# session. The directory is removed only after releasing the kernel lock.
if [[ -L "$lock_path" ]]; then
  echo "HarnessError: mutation lock path is a symlink" >&2
  exit 3
fi
if ! mkdir "$lock_path" 2>/dev/null; then
  echo "HarnessError: another mutation runner owns the session lock" >&2
  exit 3
fi
lock_file="$lock_path/lock"
: >"$lock_file" || {
  rmdir "$lock_path" 2>/dev/null || true
  echo "EnvironmentUnavailable: cannot create lock file" >&2
  exit 2
}
exec 9<"$lock_file" || {
  rm -f "$lock_file"
  rmdir "$lock_path" 2>/dev/null || true
  echo "EnvironmentUnavailable: cannot open lock" >&2
  exit 2
}
flock -n 9 || {
  exec 9>&-
  rm -f "$lock_file"
  rmdir "$lock_path" 2>/dev/null || true
  echo "HarnessError: another mutation runner owns the session lock" >&2
  exit 3
}

snapshot=$(read_output) || {
  echo "EnvironmentUnavailable: cannot read exactly one connected output" >&2
  flock -u 9
  exec 9>&-
  rm -f "$lock_file" 2>/dev/null || true
  rmdir "$lock_path" 2>/dev/null || true
  exit 2
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
child_sid=
child_cleanup_verified=0
child_escape_detected=0
lock_cleanup_failed=0
interrupted=0
timeout_hit=0
external_change=0
applied_state=
signal_handler() {
  interrupted=1
  # Cleanup owns restoration and is deliberately non-reentrant.  A signal
  # during apply/read-back only marks the run; the next checkpoint restores.
  if [[ -n "$child_pgid" ]]; then kill -TERM -- "-$child_pgid" 2>/dev/null || true; fi
  [[ "$state" == Running || "$state" == AppliedAndReadBack ]] && state=RestoreRequested
}
trap signal_handler INT TERM
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

session_processes() {
  [[ -n "$child_sid" ]] || return 0
  # A process can change its process group, so PGID-only cleanup is not a
  # proof.  The setsid-created session ID is the stronger boundary we can
  # inspect portably with procps.  If a child calls setsid itself, it escapes
  # this boundary; we report that limitation instead of claiming cleanup.
  ps -eo pid=,sid=,stat= 2>/dev/null |
    awk -v sid="$child_sid" '$2 == sid && $3 !~ /^Z/ { print $1 }'
}

child_session_alive() {
  [[ -n "$child_pid" ]] && [[ -n "$(session_processes)" ]]
}

stop_child_group() {
  [[ -n "$child_pid" ]] || return 0
  if child_alive || child_session_alive; then
    kill -TERM -- "-$child_pgid" 2>/dev/null || true
    # Also signal session members whose process groups changed. This is best
    # effort: an escaping process is not reachable through the session ID and
    # therefore must make recovery unverified rather than being misreported.
    while read -r pid; do kill -TERM "$pid" 2>/dev/null || true; done < <(session_processes)
    local deadline=$((SECONDS + timeout_seconds))
    while child_alive || child_session_alive; do
      ((SECONDS >= deadline)) && break
      sleep 0.1
    done
    if child_alive || child_session_alive; then
      timeout_hit=1
      kill -KILL -- "-$child_pgid" 2>/dev/null || true
      while read -r pid; do kill -KILL "$pid" 2>/dev/null || true; done < <(session_processes)
    fi
  fi
  wait "$child_pid" 2>/dev/null || true
  if [[ -n "$(session_processes)" ]]; then
    child_escape_detected=1
  else
    child_cleanup_verified=1
  fi
  child_pid=
  child_pgid=
}

capture_applied_state() {
  local observed
  observed=$(read_output 2>/dev/null || true)
  if [[ -n "$observed" ]]; then
    applied_state="$observed"
  else
    # Unknown state must never be treated as the requested state: restoring
    # across an unreadable observation could overwrite an external mutation.
    applied_state=__unknown__
  fi
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
    teardown=unknown
    # This runner has only NameOnly output identity.  Even a fully reaped
    # child cannot prove that the restored output is the same object.
    # Escaped sessions, cleanup failures, or external changes are explicitly
    # unverified rather than being turned into a successful-looking result.
    # NameOnly cannot prove stable output identity, so teardown never becomes
    # confirmed here. Keep the explicit unknown result even after read-back.
    teardown=unknown
    # Release the kernel lock before removing its private directory.  Failure
    # to remove either path is not harmless: it leaves ownership ambiguous and
    # must be visible in the diagnostic (the stale directory remains fail-closed).
    flock -u 9 2>/dev/null || lock_cleanup_failed=1
    exec 9>&-
    rm -f "$lock_file" 2>/dev/null || lock_cleanup_failed=1
    rmdir "$lock_path" 2>/dev/null || lock_cleanup_failed=1
    [[ "$lock_cleanup_failed" == 0 ]] || teardown=unknown
    if ! printf 'FIDUS_RESULT version=1 kind=lifecycle run_id=p5-output status=unverified execution_mode=live-host teardown=%s recovery=unverified\n' \
      "$teardown"; then
      echo 'HarnessError: cannot write lifecycle result' >&2
    fi
    if ! printf 'FIDUS_RESULT version=1 kind=summary run_id=p5-output status=failed execution_mode=live-host records_total=1 records_ok=0 records_failed=1\n'; then
      echo 'HarnessError: cannot write summary result' >&2
    fi
    printf 'P5_DIAGNOSTIC timeout=%s external_change=%s lock_cleanup_failed=%s\n' \
      "$timeout_hit" "$external_change" "$lock_cleanup_failed" >&2
  else
    # Still release the lock on the unusual re-entry path.
    flock -u 9 2>/dev/null || lock_cleanup_failed=1
    exec 9>&-
    rm -f "$lock_file" 2>/dev/null || lock_cleanup_failed=1
    rmdir "$lock_path" 2>/dev/null || lock_cleanup_failed=1
  fi
  rm -f "$child_out" "$child_err" 2>/dev/null || true
}
trap signal_handler INT TERM
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
# Record the state each setter is expected to have produced before checking its
# status.  A compositor may apply a setter and then return an error; leaving
# this empty would make cleanup overwrite an unclassified state.
applied_state="$requested_scale"$'\t'"$original_transform"
"$niri_bin" msg output "$output" scale "$requested_scale" >/dev/null 2>&1 || {
  capture_applied_state
  echo "EnvironmentUnavailable: scale command failed" >&2
  finish 2
}
applied_state="$requested_scale"$'\t'"$original_transform"
"$niri_bin" msg output "$output" transform "$requested_transform" >/dev/null 2>&1 || {
  capture_applied_state
  echo "EnvironmentUnavailable: transform command failed" >&2
  finish 2
}
applied_state="$requested_scale"$'\t'"$requested_transform"
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
child_sid=$(ps -o sid= -p "$child_pid" | tr -d ' ')
if [[ -z "$child_pgid" || ! "$child_pgid" =~ ^[0-9]+$ || -z "$child_sid" || ! "$child_sid" =~ ^[0-9]+$ ]]; then
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
  # Keep the session identifiers until EXIT cleanup verifies that no descendant
  # remains. Clearing them here would make a detached descendant invisible and
  # would falsely turn teardown into a successful-looking result.
fi
finish "$child_status"

if [[ "$restore_verified" != 1 ]]; then
  echo "RecoveryUnverified: output fields were not read back to their original values" >&2
  exit 4
fi
# NameOnly is insufficient identity evidence, even when fields match.
echo "FIDUS_RESULT version=1 kind=lifecycle run_id=p5-output status=unverified execution_mode=live-host teardown=unknown recovery=unverified"
exit "$child_status"
