// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Bakes the source-control identity into the extension so a consumer can
//! bind an installed `.so` to the exact `fidus` checkout it came from.
//!
//! The wheel `__version__` is a static `0.1.0-dev.0` and carries no commit,
//! so without this the only anchor a downstream project has is the artifact's
//! own sha256 (plan `meapet-embed-contract`, finished receipt §8-B). `git
//! describe --always --dirty` yields `v0.1.0-beta.1-36-g<sha>` at a clean HEAD
//! and appends `-dirty` when the tree has uncommitted changes — so the baked
//! string never over-claims the source state it was built from. Falls back to
//! `unknown` when git or a checkout is absent (e.g. a source-tarball build),
//! which `option_env!` in `lib.rs` tolerates.
//!
//! Caveat: the script reruns on a HEAD move or a source edit. A pure commit
//! followed by a rebuild with zero source change can read one commit stale —
//! but that case is already flagged `-dirty` at build time, so it is honest.

use std::process::Command;

fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    let describe = git_describe().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=FIDUS_GIT_DESCRIBE={describe}");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
}
