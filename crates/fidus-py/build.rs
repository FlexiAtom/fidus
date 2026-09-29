// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Bakes the source-control identity into the extension so a consumer can
//! bind an installed `.so` to the exact `fidus` checkout it came from.
//!
//! The wheel `__version__` carries the release tag name as a PEP 440 local
//! label, but that label is a claim typed into the manifest; this stamp is the
//! measured source state, and it is what pins a build made between tags to a
//! commit (plan `meapet-embed-contract`, finished receipt §8-B). `git
//! describe --always --dirty` yields `v0.1.0-beta.1-36-g<sha>` at a clean HEAD
//! and appends `-dirty` when the tree has uncommitted changes — so the baked
//! string never over-claims the source state it was built from. Falls back to
//! `unknown` when git or a checkout is absent (e.g. a source-tarball build),
//! which `option_env!` in `lib.rs` tolerates.
//!
//! The anchor is only worth having if a rebuild actually refreshes it, and a
//! commit moves no file inside this package: `HEAD` keeps saying `ref:
//! refs/heads/main`, and the commit lands in the file that ref *resolves to*.
//! Watching `HEAD` alone therefore let "dirty build -> commit -> rebuild with
//! zero source edit" re-emit the previous cached string, i.e. an extension
//! that claimed to be built from a commit it was not built from. So the
//! rerun set below is resolved through git itself rather than guessed from a
//! fixed layout, and covers all three inputs of the string: `HEAD`, the ref it
//! points at, and the index that `--dirty` reads. `packed-refs` is included
//! for the case where that ref lives in the pack instead of its own file.

use std::path::{Path, PathBuf};
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn git_describe() -> Option<String> {
    git(&["describe", "--always", "--dirty"])
}

/// Existing files whose contents decide the `describe` string, as absolute
/// paths. `--git-path` is what makes this work in a linked worktree or a
/// `.git` file checkout; a path git names but that does not exist (no
/// `packed-refs`, detached `HEAD`) is dropped rather than emitted, because a
/// missing watch path makes cargo rerun the script on every build.
fn anchor_inputs() -> Vec<PathBuf> {
    let mut queries: Vec<String> = vec!["HEAD".into(), "index".into(), "packed-refs".into()];
    if let Some(target) = git(&["symbolic-ref", "-q", "HEAD"]) {
        queries.push(target);
    }
    queries
        .iter()
        .filter_map(|q| git(&["rev-parse", "--git-path", q]))
        .filter_map(|p| {
            let abs = std::path::absolute(Path::new(&p)).ok()?;
            abs.is_file().then_some(abs)
        })
        .collect()
}

fn main() {
    let describe = git_describe().unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=FIDUS_GIT_DESCRIBE={describe}");
    for path in anchor_inputs() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
}
