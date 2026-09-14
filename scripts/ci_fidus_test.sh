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

# Offline mode alone is not a dependency manifest: Cargo can still appear to
# work from a partial CARGO_HOME cache.  CI must use the complete vendor tree
# produced by `cargo vendor --locked`; otherwise a missing crate can be hidden
# until a cache eviction or a different runner exposes it.
ci_config="${CARGO_CI_CONFIG:-.cargo/config.ci.toml}"
if [[ ! -f "$ci_config" || ! -d vendor ]]; then
  echo "ci mode requires cargo vendor output ($ci_config and vendor/)" >&2
  exit 2
fi
cargo_args=(--locked --offline --config "$ci_config")
# Resolve the graph through the vendored source before compiling.  This is a
# deliberate fail-fast check: a stale or incomplete vendor tree must never be
# mistaken for a warm registry cache.
cargo metadata "${cargo_args[@]}" --format-version 1 >/dev/null

# The workspace is not currently rustfmt-clean; keep formatting advisory until
# a dedicated formatting-only change can avoid mixing unrelated rewrites.
bash scripts/check_p5_matrix.sh
bash scripts/test_output_mutation_runner.sh
bash scripts/test_ci_offline_contract.sh
bash scripts/verify_sbom.sh
python3 tools/test_generate_test_images.py
# The live-container contract's positive path requires a real display socket;
# keep it out of deterministic no-display CI and run it in its dedicated job.
cargo test "${cargo_args[@]}" --workspace
cargo clippy "${cargo_args[@]}" --workspace --all-targets -- -D warnings
cargo doc "${cargo_args[@]}" --workspace --no-deps
