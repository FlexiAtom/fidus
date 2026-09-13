#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
set -u
set -o pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
runner="$root/scripts/output_mutation_runner.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fake="$tmp/fake-niri.sh"
state="$tmp/state"
cat >"$state" <<'EOF'
1	normal
EOF
cat >"$fake" <<'EOF'
#!/usr/bin/env bash
set -u
state=${FIDUS_FAKE_STATE:?}
if [[ "$*" == "msg outputs" ]]; then
  if [[ "${FIDUS_FAKE_OUTPUT_MODE:-normal}" == malformed ]]; then
    printf 'Output "Panel" (eDP-1)\n  Scale: 1.25\n'
    exit 0
  fi
  if [[ "${FIDUS_FAKE_OUTPUT_MODE:-normal}" == deferred ]]; then
    printf 'Output "missing" is not connected.\nThe change will apply when it is connected.\n'
    exit 0
  fi
  IFS=$'\t' read -r scale transform <"$state"
  case "$transform" in
    normal) display=normal ;;
    90) display='90° counter-clockwise' ;;
    180) display='180°' ;;
    270) display='270° counter-clockwise' ;;
  esac
  printf 'Output "Panel" (eDP-1)\n  Scale: %s\n  Transform: %s\n' "$scale" "$display"
  exit 0
fi
if [[ "$1" == msg && "$2" == output ]]; then
  IFS=$'\t' read -r scale transform <"$state"
  case "$4" in
    scale) scale=$5 ;;
    transform) transform=$5 ;;
    *) exit 2 ;;
  esac
  printf '%s\t%s\n' "$scale" "$transform" >"$state"
  exit 0
fi
exit 2
EOF
chmod +x "$fake"
export FIDUS_NIRI_BIN="$fake"
export FIDUS_FAKE_STATE="$state"
export XDG_RUNTIME_DIR="$tmp/runtime"
mkdir "$XDG_RUNTIME_DIR"

failures=0
expect_rc() {
  expected=$1; shift
  set +e
  "$@" >/dev/null 2>&1
  actual=$?
  set -e
  [[ "$actual" == "$expected" ]] || {
    echo "expected rc=$expected got rc=$actual: $*" >&2
    "$@" >&2 || true
    failures=$((failures + 1))
  }
}
set -e
expect_rc 3 "$runner" --output eDP-1 --scale 1.25 --transform 90 -- true
runner_output=$("$runner" --allow-output-mutation --output eDP-1 --scale 1.25 --transform 90 -- sh -c 'sleep 0.2' 2>/dev/null || true)
[[ "$(grep -c '^FIDUS_RESULT ' <<<"$runner_output")" == 2 ]] || { echo 'expected lifecycle plus summary' >&2; failures=$((failures + 1)); }
[[ "$(grep -c 'kind=lifecycle' <<<"$runner_output")" == 1 ]] || { echo 'expected one lifecycle record' >&2; failures=$((failures + 1)); }
[[ "$(grep -c 'kind=summary' <<<"$runner_output")" == 1 ]] || { echo 'expected one summary record' >&2; failures=$((failures + 1)); }
[[ "$(grep -c 'recovery=unverified' <<<"$runner_output")" == 1 ]] || { echo 'expected unverified recovery' >&2; failures=$((failures + 1)); }
[[ "$(<"$state")" == $'1\tnormal' ]] || { echo 'state was not restored' >&2; failures=$((failures + 1)); }
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
expect_rc 4 "$runner" --allow-output-mutation --output eDP-1 --scale 1.5 --transform 180 -- sh -c 'sleep 0.2; exit 7'
[[ "$(<"$state")" == $'1\tnormal' ]] || { echo 'state was not restored after child failure' >&2; failures=$((failures + 1)); }
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
expect_rc 4 "$runner" --allow-output-mutation --timeout-seconds 1 --output eDP-1 --scale 1.5 --transform 180 -- sh -c 'sleep 5'
[[ "$(<"$state")" == $'1\tnormal' ]] || { echo 'state was not restored after timeout' >&2; failures=$((failures + 1)); }
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
expect_rc 4 "$runner" --allow-output-mutation --output eDP-1 --scale 1.5 --transform 180 -- sh -c 'sleep 0.2; printf "2\\t180\\n" > "$FIDUS_FAKE_STATE"'
[[ "$(<"$state")" == $'2\t180' ]] || { echo 'external state was overwritten' >&2; failures=$((failures + 1)); }
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
ln -s "$tmp/elsewhere" "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
expect_rc 3 "$runner" --allow-output-mutation --output eDP-1 --scale 1.25 --transform 90 -- true
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
mkdir "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
exec 8<"$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
flock -n 8
expect_rc 3 "$runner" --allow-output-mutation --output eDP-1 --scale 1.25 --transform 90 -- sh -c 'sleep 0.2'
flock -u 8
exec 8>&-
rm -rf "$XDG_RUNTIME_DIR/fidus-output-mutation.lock"
export FIDUS_FAKE_OUTPUT_MODE=malformed
expect_rc 2 "$runner" --allow-output-mutation --output eDP-1 --scale 1.25 --transform 90 -- true
export FIDUS_FAKE_OUTPUT_MODE=deferred
expect_rc 2 "$runner" --allow-output-mutation --output missing --scale 1.25 --transform 90 -- true
((failures == 0)) || exit 1
echo 'output mutation runner tests: passed'
