// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Diagnostic: projects one marker and reports what the capture actually
//! contains — edge sharpness, changed-pixel count, fill ratio.
//!
//! Exists because the fractional-scale `AreaMismatch` failures (spec §4.1,
//! `docs/measurements/l9-fractional-scaling.md`) needed evidence rather than a
//! hypothesis: at scale 1.25 the marker buffer is drawn at logical size with
//! `buffer_scale = 1`, so the compositor rescales it, and whether it does so
//! with nearest-neighbour or bilinear filtering decides whether the detector's
//! `min_fill_ratio` gate can still see a solid square.
//!
//! Run under the target compositor, optionally after `niri msg output eDP-1
//! scale 1.25`.

use fidus_backend_wayland_layer::{BackendSession, WaylandLayerBackend};
use fidus_core::coord::LogicalPoint;
use fidus_core::io::{CalibrationIo, CaptureIo, MarkerStyle};

fn main() {
    let mut backend = match WaylandLayerBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("no layer-shell backend: {e}");
            std::process::exit(1);
        }
    };

    let mut io = match BackendSession::open(&mut backend) {
        Ok(io) => io,
        Err(e) => {
            eprintln!("cannot open calibration session: {e}");
            std::process::exit(1);
        }
    };

    let style = MarkerStyle::DEFAULT;
    let at = LogicalPoint::new(101.0, 101.0);

    // FIDUS_PROBE_NOMARKER=1 skips projection, so the two captures differ only
    // by whatever the rest of the screen did — the background-noise floor the
    // detector has to survive.
    let no_marker = std::env::var("FIDUS_PROBE_NOMARKER").is_ok();

    // FIDUS_PROBE_BURST=N captures N frames back-to-back inside one process and
    // reports every consecutive pair. Launching a fresh process between
    // captures perturbs the screen (terminal output, window focus), which is
    // exactly the confound that made an earlier one-shot reading of "30361
    // changed pixels" irreproducible.
    // FIDUS_PROBE_CYCLE=N repeats the real show/capture/clear cycle N times in
    // one process and reports what the detector would see each round. This is
    // how the intermittent "N plausible regions" failures were tracked down:
    // they are invisible to a single-shot probe.
    if let Ok(n) = std::env::var("FIDUS_PROBE_CYCLE") {
        let n: usize = n.parse().unwrap_or(10);
        for round in 0..n {
            let base = io.capture().expect("baseline");
            io.show_marker(at, style).expect("show");
            let post = io.capture().expect("post");
            io.clear_marker().expect("clear");

            let mut changed = 0u32;
            let mut above = 0u32;
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
            for y in 0..post.height.min(base.height) {
                for x in 0..post.width.min(base.width) {
                    let a = base.rgba_at(x, y);
                    let b = post.rgba_at(x, y);
                    let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
                    if d > 0 {
                        changed += 1;
                    }
                    if d >= 12 {
                        above += 1;
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x);
                        y1 = y1.max(y);
                    }
                }
            }
            if x0 == u32::MAX {
                println!("round {round}: {changed} changed, NOTHING above threshold");
            } else {
                println!(
                    "round {round}: {changed} changed, {above} above, span ({x0},{y0})..({x1},{y1}) [{}x{}]",
                    x1 - x0 + 1,
                    y1 - y0 + 1
                );
            }
        }
        io.destroy_projector().expect("teardown");
        return;
    }

    if let Ok(n) = std::env::var("FIDUS_PROBE_BURST") {
        let n: usize = n.parse().unwrap_or(8);
        let mut frames = Vec::new();
        for _ in 0..n {
            frames.push(io.capture().expect("burst capture"));
        }
        io.destroy_projector().expect("teardown");
        for w in frames.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let mut changed = 0u32;
            let mut above = 0u32;
            let mut maxd = 0u8;
            for y in 0..a.height.min(b.height) {
                for x in 0..a.width.min(b.width) {
                    let pa = a.rgba_at(x, y);
                    let pb = b.rgba_at(x, y);
                    let d = (0..3).map(|i| pa[i].abs_diff(pb[i])).max().unwrap_or(0);
                    if d > 0 {
                        changed += 1;
                        maxd = maxd.max(d);
                        if d >= 12 {
                            above += 1;
                        }
                    }
                }
            }
            println!("pair: {changed} changed, {above} above threshold, max delta {maxd}");
        }
        return;
    }

    let baseline = io.capture().expect("baseline capture");
    if !no_marker {
        io.show_marker(at, style).expect("show marker");
    }
    let post = io.capture().expect("post capture");
    if !no_marker {
        io.clear_marker().expect("clear marker");
    }
    io.destroy_projector().expect("teardown");

    println!("capture {}x{}", post.width, post.height);
    println!("marker: logical {:?} size {}", (at.x, at.y), style.size_logical);

    // Per-pixel max channel difference, so we can see the *distribution* of
    // edge softness rather than a single thresholded count.
    let mut diffs: Vec<u8> = Vec::new();
    let mut bbox = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..post.height.min(baseline.height) {
        for x in 0..post.width.min(baseline.width) {
            let a = baseline.rgba_at(x, y);
            let b = post.rgba_at(x, y);
            let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
            if d > 0 {
                diffs.push(d);
                bbox.0 = bbox.0.min(x);
                bbox.1 = bbox.1.min(y);
                bbox.2 = bbox.2.max(x);
                bbox.3 = bbox.3.max(y);
            }
        }
    }

    if diffs.is_empty() {
        println!("no changed pixels at all");
        return;
    }
    if no_marker {
        let strong = diffs.iter().filter(|&&d| d >= 12).count();
        println!("NO-MARKER background diff: {} changed px, {strong} above threshold", diffs.len());
        return;
    }

    let (w, h) = (bbox.2 - bbox.0 + 1, bbox.3 - bbox.1 + 1);
    println!("changed bbox: {}x{} at ({}, {})", w, h, bbox.0, bbox.1);

    // Changed pixels *outside* the marker's own footprint. The detector
    // assumes the marker is the only thing that changes between the two
    // captures; anything counted here violates that assumption. Projecting an
    // overlay can make the compositor recomposite the whole output, and under
    // fractional scaling a recomposite need not be bit-identical.
    // Exclude the marker using the *measured* bbox in capture pixels. Deriving
    // it from the logical request instead was a bug in this probe: under
    // fractional scaling the marker lands at round(L*scale), so a logical-space
    // exclusion box sits in the wrong place and reports the marker itself as an
    // outside-the-marker change.
    let pad = 4i64;
    let mx0 = bbox.0 as i64 - pad;
    let my0 = bbox.1 as i64 - pad;
    let mx1 = bbox.2 as i64 + pad;
    let my1 = bbox.3 as i64 + pad;
    let mut outside = 0u32;
    let mut outside_above = 0u32;
    let mut outside_max = 0u8;
    for y in 0..post.height.min(baseline.height) {
        for x in 0..post.width.min(baseline.width) {
            let (xi, yi) = (x as i64, y as i64);
            if xi >= mx0 && xi <= mx1 && yi >= my0 && yi <= my1 {
                continue;
            }
            let a = baseline.rgba_at(x, y);
            let b = post.rgba_at(x, y);
            let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
            if d > 0 {
                outside += 1;
                outside_max = outside_max.max(d);
                if d >= 12 {
                    outside_above += 1;
                }
            }
        }
    }
    println!(
        "outside marker: {outside} changed, {outside_above} above threshold, max delta {outside_max}"
    );

    // Where are they? Report the bounding box of the out-of-footprint changes
    // so we can tell a compositor artifact from a real second object.
    let mut ob = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..post.height.min(baseline.height) {
        for x in 0..post.width.min(baseline.width) {
            let (xi, yi) = (x as i64, y as i64);
            if xi >= mx0 && xi <= mx1 && yi >= my0 && yi <= my1 {
                continue;
            }
            let a = baseline.rgba_at(x, y);
            let b = post.rgba_at(x, y);
            let d = (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
            if d >= 12 {
                ob.0 = ob.0.min(x);
                ob.1 = ob.1.min(y);
                ob.2 = ob.2.max(x);
                ob.3 = ob.3.max(y);
            }
        }
    }
    if ob.0 != u32::MAX {
        println!(
            "  their bbox: ({}, {}) .. ({}, {})  [{}x{}]",
            ob.0, ob.1, ob.2, ob.3, ob.2 - ob.0 + 1, ob.3 - ob.1 + 1
        );
        println!("  capture is {}x{}", post.width, post.height);
    }

    // A nearest-neighbour rescale keeps every marker pixel at full difference.
    // Bilinear filtering produces a rim of intermediate values, which is what
    // erodes the fill ratio once the detector thresholds at 12.
    let full = diffs.iter().filter(|&&d| d >= 200).count();
    let mid = diffs.iter().filter(|&&d| (12..200).contains(&d)).count();
    let faint = diffs.iter().filter(|&&d| d < 12).count();
    println!("changed pixels: {} total", diffs.len());
    println!("  >=200 (crisp)      : {full}");
    println!("  12..200 (blended)  : {mid}");
    println!("  <12 (below thresh) : {faint}   <- invisible to the detector");

    let above = full + mid;
    println!(
        "fill ratio seen by detector: {:.3} (gate rejects below 0.45)",
        above as f64 / (w as f64 * h as f64)
    );
}
