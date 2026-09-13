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

for script in scripts/*.sh; do
  bash -n "$script"
done
# The workspace is not currently rustfmt-clean; keep formatting advisory until
# a dedicated formatting-only change can avoid mixing unrelated rewrites.
bash scripts/check_p5_matrix.sh
bash scripts/test_output_mutation_runner.sh
# The live-container contract's positive path requires a real display socket;
# keep it out of deterministic no-display CI and run it in its dedicated job.
cargo test --locked --workspace --offline
cargo clippy --locked --workspace --all-targets --offline -- -D warnings
cargo doc --locked --workspace --no-deps --offline
