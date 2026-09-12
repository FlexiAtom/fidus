#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Deterministic CI only: this script must not acquire a display session.
set -euo pipefail

if [[ -n "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]]; then
  echo "ci mode refuses display environment" >&2
  exit 2
fi

if [[ -e /dev/dri ]]; then
  echo "ci mode refuses /dev/dri" >&2
  exit 2
fi

cargo test --workspace --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo doc --workspace --no-deps --offline
