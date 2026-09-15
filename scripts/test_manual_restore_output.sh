#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fake="$tmp/fake-niri.sh"
state="$tmp/state"
log="$tmp/recovery.log"
printf '2\t180\n' >"$state"
cat >"$fake" <<'EOF'
#!/usr/bin/env bash
set -u
state=${FIDUS_FAKE_STATE:?}
if [[ "$*" == "msg outputs" ]]; then
  mode=${FIDUS_FAKE_OUTPUT_MODE:-normal}
  if [[ "$mode" == deferred ]]; then
    printf 'Output "missing" is not connected.\nThe change will apply when it is connected.\n'; exit 0
  fi
  if [[ "$mode" == malformed ]]; then
    printf 'Output "Panel" (eDP-1)\n  Scale: invalid\n'; exit 0
  fi
  IFS=$'\t' read -r scale transform <"$state"
  case "$transform" in normal) display=normal ;; 90) display='90° counter-clockwise' ;; 180) display='180°' ;; 270) display='270° counter-clockwise' ;; esac
  printf 'Output "Panel" (eDP-1)\n  Scale: %s\n  Transform: %s\n' "$scale" "$display"; exit 0
fi
if [[ "$1" == msg && "$2" == output ]]; then
  IFS=$'\t' read -r scale transform <"$state"
  case "$4" in scale) scale=$5 ;; transform) transform=$5 ;; *) exit 2 ;; esac
  printf '%s\t%s\n' "$scale" "$transform" >"$state"
  [[ "${FIDUS_FAKE_OUTPUT_MODE:-normal}" == fail-scale && "$4" == scale ]] && exit 1
  exit 0
fi
exit 2
EOF
chmod +x "$fake"
export FIDUS_NIRI_BIN="$fake" FIDUS_FAKE_STATE="$state" XDG_RUNTIME_DIR="$tmp/runtime"
mkdir "$XDG_RUNTIME_DIR"

expect_rc() { expected=$1; shift; set +e; "$@" >/dev/null 2>&1; actual=$?; set -e; [[ "$actual" == "$expected" ]]; }
expect_rc 3 "$root/scripts/manual_restore_output.sh" --output eDP-1 --scale 1 --transform normal
expect_rc 0 "$root/scripts/manual_restore_output.sh" --allow-output-mutation --output eDP-1 --scale 1 --transform normal --log "$log"
[[ "$(<"$state")" == $'1\tnormal' ]]
grep -q 'recovery=unverified' "$log"
printf '2\t180\n' >"$state"
export FIDUS_FAKE_OUTPUT_MODE=fail-scale
expect_rc 4 "$root/scripts/manual_restore_output.sh" --allow-output-mutation --output eDP-1 --scale 1 --transform normal
[[ "$(<"$state")" == $'1\tnormal' ]]
rm -f "$log"
printf '2\t180\n' >"$state"
export FIDUS_FAKE_OUTPUT_MODE=deferred
expect_rc 2 "$root/scripts/manual_restore_output.sh" --allow-output-mutation --output missing --scale 1 --transform normal
printf 'MANUAL_RESTORE_TEST_OK\n'
