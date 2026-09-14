#!/usr/bin/env bash
# Copyright 2026 Flexiatom
# SPDX-License-Identifier: Apache-2.0
#
# Contract test for the no-fake-cache CI boundary; no display or network needed.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
config="$root/.cargo/config.ci.toml"
runner="$root/scripts/ci_fidus_test.sh"
dockerfile="$root/Dockerfile.ci"

 grep -Fxq 'replace-with = "vendored-sources"' "$config"
 grep -Fxq 'directory = "vendor"' "$config"
 grep -Fq 'cargo vendor --locked vendor' "$dockerfile"
 grep -Fq 'cargo metadata' "$runner"
 grep -Fq 'requires cargo vendor output' "$runner"
 grep -Fq -- '--offline' "$runner"

echo "offline CI contract: pass"
