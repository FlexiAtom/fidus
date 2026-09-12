#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
# Regression tests for the legacy CLI and fidus-test bridge.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/target/debug/fidus-test"
legacy="$root/target/debug/fidus-live-calibrate"
[[ -x "$bin" && -x "$legacy" ]] || { echo 'build debug binaries first' >&2; exit 2; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat >"$tmp/child" <<'EOF'
#!/usr/bin/env bash
set -u
printf '%s\n' \
  'FIDUS_RESULT version=1 kind=environment run_id=compat status=ready execution_mode=live-host backend=none compositor=none output=none scale=not-measured transform=not-measured' \
  'FIDUS_RESULT version=1 kind=calibration run_id=compat status=ok execution_mode=live-host backend=none method=crosshair rms_residual_px=0 verification_max_err_px=0 consistency_max_err_px=0' \
  'FIDUS_RESULT version=1 kind=lifecycle run_id=compat status=ok execution_mode=live-host teardown=confirmed recovery=not_requested' \
  'FIDUS_RESULT version=1 kind=summary run_id=compat status=ok execution_mode=live-host records_total=3 records_ok=3 records_failed=0'
exit "${CHILD_RC:-0}"
EOF
chmod +x "$tmp/child"
FIDUS_LIVE_CALIBRATE_BIN="$tmp/child" CHILD_RC=0 "$bin" live-calibrate >/dev/null
set +e
FIDUS_LIVE_CALIBRATE_BIN="$tmp/child" CHILD_RC=1 "$bin" live-calibrate >/dev/null
rc=$?
set -e
[[ "$rc" == 1 ]] || { echo "bridge did not preserve exit 1: $rc" >&2; exit 1; }
set +e
WAYLAND_DISPLAY= DISPLAY= "$legacy" >/dev/null 2>&1
rc=$?
set -e
[[ "$rc" == 2 ]] || { echo "legacy init exit changed: $rc" >&2; exit 1; }
echo 'CLI compatibility: ok'
