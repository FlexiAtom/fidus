#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Run a fidus test binary in a container against the current display session.
# This is not display isolation: a mounted Wayland/X11 socket still lets the
# client affect the host compositor through that protocol.
set -euo pipefail

usage() {
  echo "usage: $0 --allow-live-container --image IMAGE [--allow-unpinned-image] [--container-user UID:GID] [-- subcommand args...]" >&2
}

allow=0
allow_unpinned=0
image=
container_user="$(id -u):$(id -g)"
# Dockerfile.live-container sets ENTRYPOINT=fidus-test; only pass its subcommand.
command_args=(live-calibrate)
while (($#)); do
  case "$1" in
    --allow-live-container) allow=1; shift ;;
    --allow-unpinned-image) allow_unpinned=1; shift ;;
    --image) (($# >= 2)) || { usage; exit 2; }; image=$2; shift 2 ;;
    --container-user) (($# >= 2)) || { usage; exit 2; }; container_user=$2; shift 2 ;;
    --allow-output-mutation)
      echo "live-container refuses output mutation; only live-host may own it" >&2
      exit 2
      ;;
    --) shift; command_args=("$@"); break ;;
    *) usage; exit 2 ;;
  esac
done

((allow == 1)) || {
  echo "live-container requires explicit --allow-live-container" >&2
  exit 2
}
[[ -n "$image" ]] || { usage; exit 2; }
if [[ "$image" != *@sha256:* && "$allow_unpinned" != 1 ]]; then
  echo "image must include a digest (or explicitly use --allow-unpinned-image for local testing)" >&2
  exit 2
fi

if [[ ! "$container_user" =~ ^[0-9]+:[0-9]+$ ]]; then
  echo "HarnessError: --container-user must be UID:GID" >&2
  exit 3
fi
uid=${container_user%%:*}
gid=${container_user##*:}
runtime_dir=${XDG_RUNTIME_DIR:-}
wayland_display=${WAYLAND_DISPLAY:-}
display=${DISPLAY:-}

# A missing session is an environment result, not a fidus calibration result.
if [[ -n "$wayland_display" ]]; then
  [[ -n "$runtime_dir" ]] || { echo "EnvironmentUnavailable: XDG_RUNTIME_DIR" >&2; exit 2; }
  wayland_socket="$runtime_dir/$wayland_display"
  [[ -S "$wayland_socket" ]] || { echo "EnvironmentUnavailable: Wayland socket" >&2; exit 2; }
  display_env=("--env" "WAYLAND_DISPLAY=$wayland_display" "--env" "XDG_RUNTIME_DIR=/run/user/$uid")
  mounts=("--mount" "type=bind,src=$wayland_socket,dst=/run/user/$uid/$wayland_display,readonly")
elif [[ -n "$display" ]]; then
  x11_socket_dir=/tmp/.X11-unix
  [[ -d "$x11_socket_dir" ]] || { echo "EnvironmentUnavailable: X11 socket directory" >&2; exit 2; }
  display_env=("--env" "DISPLAY=$display")
  mounts=("--mount" "type=bind,src=$x11_socket_dir,dst=/tmp/.X11-unix,readonly")
  if [[ -n "${XAUTHORITY:-}" ]]; then
    [[ -f "$XAUTHORITY" ]] || { echo "EnvironmentUnavailable: Xauthority file" >&2; exit 2; }
    display_env+=("--env" "XAUTHORITY=$XAUTHORITY")
    mounts+=("--mount" "type=bind,src=$XAUTHORITY,dst=$XAUTHORITY,readonly")
  fi
else
  echo "EnvironmentUnavailable: no Wayland or X11 display" >&2
  exit 2
fi

run_id="container-$$"
metadata_env=()
for key in XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP; do
  value=${!key:-}
  if [[ -n "$value" && "$value" =~ ^[A-Za-z0-9_.:-]+$ ]]; then
    metadata_env+=("--env" "$key=$value")
  fi
done
container_runtime=${FIDUS_CONTAINER_RUNTIME:-docker}
case "$container_runtime" in
  docker|podman|/*) ;;
  *) echo "HarnessError: container runtime must be docker, podman, or an absolute path" >&2; exit 3 ;;
esac
set +e
"$container_runtime" run --rm \
  --user "$uid:$gid" \
  --read-only \
  --cap-drop=ALL \
  --security-opt=no-new-privileges \
  --network=none \
  --tmpfs /tmp:rw,noexec,nosuid,size=16m \
  --tmpfs /dev/shm:rw,noexec,nosuid,size=16m \
  "${display_env[@]}" \
  "${metadata_env[@]}" \
  --env "FIDUS_EXECUTION_MODE=live-container" \
  --env "FIDUS_RUN_ID=$run_id" \
  "${mounts[@]}" \
  "$image" "${command_args[@]}"
rc=$?
set -e
if [[ "$rc" == 125 || "$rc" == 126 || "$rc" == 127 ]]; then
  echo "EnvironmentUnavailable: container runtime failed before test execution (rc=$rc)" >&2
  exit 2
fi
exit "$rc"
