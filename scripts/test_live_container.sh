#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Deterministic contract tests for live_container.sh. These do not prove that
# a real Wayland/X11 socket works in a container; that requires a real image.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
uid=$(id -u)
gid=$(id -g)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fake="$tmp/runtime"
cat >"$fake" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >"${FAKE_ARGS_FILE:?}"
exit "${FAKE_RUNTIME_RC:-0}"
EOF
chmod +x "$fake"

expect_rc() {
  local expected=$1; shift
  set +e
  "$@" >/dev/null 2>"$tmp/stderr"
  local actual=$?
  set -e
  [[ "$actual" == "$expected" ]] || {
    echo "expected rc=$expected, got rc=$actual: $*" >&2
    cat "$tmp/stderr" >&2
    exit 1
  }
}

expect_rc 2 "$root/scripts/live_container.sh"
expect_rc 2 "$root/scripts/live_container.sh" --allow-live-container --image local:test
expect_rc 2 "$root/scripts/live_container.sh" --allow-live-container --allow-unpinned-image --image local:test --allow-output-mutation
expect_rc 2 env FIDUS_CONTAINER_RUNTIME="$fake" FAKE_ARGS_FILE="$tmp/args" FAKE_RUNTIME_RC=125 \
  "$root/scripts/live_container.sh" --allow-live-container --allow-unpinned-image --image local:test

FAKE_ARGS_FILE="$tmp/args" FIDUS_CONTAINER_RUNTIME="$fake" \
  "$root/scripts/live_container.sh" --allow-live-container --allow-unpinned-image --image local:test

grep -qx -- '--user' "$tmp/args"
grep -qx -- "$uid:$gid" "$tmp/args"
grep -qx -- '--cap-drop=ALL' "$tmp/args"
grep -qx -- '--network=none' "$tmp/args"
! grep -q '/dev/dri' "$tmp/args"
tail -n 1 "$tmp/args" | grep -qx 'live-calibrate'

# Different identity is explicit test input, not a production default.
FAKE_ARGS_FILE="$tmp/different-args" FIDUS_CONTAINER_RUNTIME="$fake" \
  "$root/scripts/live_container.sh" --allow-live-container --allow-unpinned-image \
  --container-user 65534:65534 --image local:test
 grep -qx -- '65534:65534' "$tmp/different-args"

expect_rc 3 "$root/scripts/live_container.sh" --allow-live-container \
  --allow-unpinned-image --container-user invalid --image local:test

# No display session must be classified before invoking the runtime.
expect_rc 2 env WAYLAND_DISPLAY= XDG_RUNTIME_DIR= DISPLAY= \
  FIDUS_CONTAINER_RUNTIME="$fake" FAKE_ARGS_FILE="$tmp/no-session" \
  "$root/scripts/live_container.sh" --allow-live-container --allow-unpinned-image --image local:test
[[ ! -e "$tmp/no-session" ]]

echo 'live-container contract: ok'
