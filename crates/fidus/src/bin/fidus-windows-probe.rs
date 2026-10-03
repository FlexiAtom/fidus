// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Diagnostic: measures the **shipped** Windows backend on a live desktop.
//!
//! This is the Windows counterpart of `fidus-probe-marker` (Wayland) and the
//! permanent form of the tool the Windows-backend proposal was measured with:
//! it drives the real `WindowsBackend`, the real `AnchorCalibrator` and the
//! real detector — no private copies of the primitives — and prints readings
//! for every checklist item in `docs/backend-contract.md` §4 that a machine can
//! answer by itself.
//!
//! Run it when changing anything in the backend, or when a calibration fails on
//! a new machine:
//!
//! ```text
//! cargo run --release --no-default-features --features windows \
//!     -p fidus --bin fidus-windows-probe
//! ```
//!
//! It flashes small markers on the desktop and destroys them again (including
//! on the failure and panic paths). It never injects input, never moves the
//! pointer and never changes display settings.

#[cfg(windows)]
fn main() {
    windows_impl::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("fidus-windows-probe requires a Windows host");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows_impl {
    use std::time::{Duration, Instant};

    use fidus::prelude::*;
    use fidus_backend_windows::api::MarkerSpec;
    use fidus_backend_windows::plan::plan_marker;
    use fidus_calibrate::detect::{detect_colored_change, DetectConfig};
    use fidus_core::io::Frame;

    /// Samples per projection cell.
    const TRIALS: u32 = 20;

    /// Deterministic LCG, so a run is reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 11
        }
        fn range(&mut self, lo: i32, hi: i32) -> i32 {
            if hi <= lo {
                return lo;
            }
            lo + (self.next() % (hi - lo) as u64) as i32
        }
    }

    #[derive(Default)]
    struct Tally {
        trials: u32,
        found: u32,
        centroid_exact: u32,
        colour_exact: u32,
        area_exact: u32,
        clear_checked: u32,
        clear_clean: u32,
        residual_max: f64,
        residual_sum: f64,
    }

    impl Tally {
        fn report(&self, label: &str) {
            let mean = if self.found > 0 {
                self.residual_sum / f64::from(self.found)
            } else {
                0.0
            };
            println!(
                "{label}: trials={} found={} centroid_exact={} colour_exact={} area_exact={} clear_clean={}/{} residual_max={:.3}px residual_mean={:.3}px",
                self.trials,
                self.found,
                self.centroid_exact,
                self.colour_exact,
                self.area_exact,
                self.clear_clean,
                self.clear_checked,
                self.residual_max,
                mean,
            );
        }
    }

    fn nonblack_percent(frame: &Frame) -> f64 {
        let mut lit = 0u64;
        let mut total = 0u64;
        for y in (0..frame.height).step_by(3) {
            for x in (0..frame.width).step_by(3) {
                total += 1;
                let px = frame.rgba_at(x, y);
                if px[0] | px[1] | px[2] != 0 {
                    lit += 1;
                }
            }
        }
        if total == 0 {
            0.0
        } else {
            100.0 * lit as f64 / total as f64
        }
    }

    fn changed_pixels(a: &Frame, b: &Frame, threshold: i32) -> u64 {
        if a.width != b.width || a.height != b.height {
            return 0;
        }
        let mut changed = 0u64;
        for y in 0..a.height {
            for x in (0..a.width).step_by(3) {
                let (p, q) = (a.rgba_at(x, y), b.rgba_at(x, y));
                if (0..3).any(|c| (i32::from(p[c]) - i32::from(q[c])).abs() > threshold) {
                    changed += 1;
                }
            }
        }
        changed
    }

    pub fn run() {
        println!("== fidus Windows backend probe ==");
        let mut backend = match fidus::windows::WindowsBackend::connect() {
            Ok(b) => b,
            Err(e) => {
                println!("connect = FAILED {e}");
                std::process::exit(1);
            }
        };
        let area = backend.platform().facts().work_area;
        let facts = backend.platform().facts().clone();
        println!("dpi_awareness = {:?} (physical={})", facts.dpi_awareness, facts.dpi_awareness.is_physical());
        println!("monitors = {}", facts.monitors);
        println!(
            "work_area = {}x{} at ({}, {})",
            area.width, area.height, area.left, area.top
        );
        println!(
            "capture_probe = {}",
            match &facts.capture_probe {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("FAILED {e}"),
            }
        );

        let env = backend.probe_environment();
        println!(
            "environment = layer_shell:{} multi_marker:{} capture:{:?} compositor:{:?}",
            env.has_layer_shell,
            env.multi_marker_projection,
            env.screen_capture_permission,
            env.compositor_type
        );

        // ---- capture sanity: a real desktop, and one that is not frozen ----
        {
            let first = match backend.capture_once() {
                Ok(f) => f,
                Err(e) => {
                    println!("capture = FAILED {e}");
                    std::process::exit(1);
                }
            };
            std::thread::sleep(Duration::from_millis(250));
            let second = backend.capture_once().expect("second capture");
            println!(
                "capture = {}x{} stride={} format={:?} nonblack={:.2}% changed_over_250ms={}",
                first.width,
                first.height,
                first.stride,
                first.format,
                nonblack_percent(&first),
                changed_pixels(&first, &second, 12)
            );
        }

        // ---- projection: does our own capture contain our own marker? ----
        println!();
        println!("== projection (baseline -> show -> capture -> clear -> capture) ==");
        let detect = DetectConfig::default();
        let mut tally = Tally::default();
        let mut rng = Lcg(0x5EED_1234_ABCD_0001);
        let mut capture_ms: Vec<f64> = Vec::new();
        let colours = [255u8, 0, 255, 255];
        {
            let mut io = match backend.open() {
                Ok(io) => io,
                Err(e) => {
                    println!("session = FAILED {e}");
                    std::process::exit(1);
                }
            };
            for trial in 0..TRIALS {
                tally.trials += 1;
                let size = if trial % 2 == 0 { 8.0 } else { 28.0 };
                let style = MarkerStyle { rgba: colours, size_logical: size, shape: MarkerShape::SolidSquare };
                let max_x = area.width - size as i32 - 40;
                let max_y = area.height - size as i32 - 40;
                let pos = LogicalPoint::new(
                    f64::from(rng.range(40, max_x.max(41))),
                    f64::from(rng.range(40, max_y.max(41))),
                );

                let started = Instant::now();
                let baseline = io.capture().expect("baseline capture");
                capture_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                if let Err(e) = io.show_marker(pos, style) {
                    println!("trial{trial}: show_marker FAILED {e}");
                    continue;
                }
                let started = Instant::now();
                let post = io.capture().expect("post capture");
                capture_ms.push(started.elapsed().as_secs_f64() * 1000.0);
                if let Err(e) = io.clear_marker() {
                    println!("trial{trial}: clear_marker FAILED {e}");
                }
                let after = io.capture().expect("after capture");

                let centre = LogicalPoint::new(pos.x + size / 2.0, pos.y + size / 2.0);
                match detect_colored_change(
                    &baseline,
                    &post,
                    colours,
                    8,
                    Some(size * size),
                    &detect,
                ) {
                    Ok(found) => {
                        tally.found += 1;
                        let detected = found.bbox.center();
                        let (dx, dy) = (detected.x - centre.x, detected.y - centre.y);
                        let residual = (dx * dx + dy * dy).sqrt();
                        tally.residual_max = tally.residual_max.max(residual);
                        tally.residual_sum += residual;
                        if dx == 0.0 && dy == 0.0 {
                            tally.centroid_exact += 1;
                        }
                        if found.area == (size * size) as u32 {
                            tally.area_exact += 1;
                        }
                        let mut exact = true;
                        for y in found.bbox.y0.max(0) as u32..found.bbox.y1.max(0) as u32 {
                            for x in found.bbox.x0.max(0) as u32..found.bbox.x1.max(0) as u32 {
                                let px = post.rgba_at(x, y);
                                if px[0] != colours[0] || px[1] != colours[1] || px[2] != colours[2] {
                                    exact = false;
                                }
                            }
                        }
                        if exact {
                            tally.colour_exact += 1;
                        }
                        // Removal must already be visible in the next capture.
                        tally.clear_checked += 1;
                        if detect_colored_change(&baseline, &after, colours, 8, Some(size * size), &detect)
                            .is_err()
                        {
                            tally.clear_clean += 1;
                        }
                    }
                    Err(e) => println!("trial{trial}: not detected ({e})"),
                }
            }
            tally.report("projection");
        }

        // ---- click-through: the marker must not intercept a mouse hit ----
        println!();
        println!("== input ==");
        // This section drives the primitives directly instead of going through
        // a session, because it needs the planned screen rectangles to hit-test.
        // Windows are therefore created here, on this thread, and destroyed here
        // on this thread as well — the owner-thread rule the backend enforces
        // for its own projection.
        {
            let mut intercepted = 0;
            let mut planned: Vec<MarkerSpec> = Vec::new();
            let corners = [
                (40, 40),
                (area.width - 48, 40),
                (40, area.height - 48),
                (area.width - 48, area.height - 48),
            ];
            for (index, (x, y)) in corners.iter().enumerate() {
                let style = MarkerStyle {
                    rgba: fidus_calibrate::SENTINEL_COLORS[index],
                    size_logical: 8.0,
                    shape: MarkerShape::SolidSquare,
                };
                match plan_marker(LogicalPoint::new(f64::from(*x), f64::from(*y)), style, area) {
                    Ok(spec) => planned.push(spec),
                    Err(e) => println!("corner {index}: plan FAILED {e}"),
                }
            }
            let platform = backend.platform();
            let mut live = Vec::new();
            for spec in &planned {
                match platform.create_marker(spec) {
                    Ok(id) => {
                        if let Err(e) = platform.present_marker(id) {
                            println!("present FAILED {e}");
                        }
                        live.push((id, spec));
                    }
                    Err(e) => println!("create FAILED {e}"),
                }
            }
            let _ = platform.sync_presentation();
            for (_, spec) in &live {
                // A hit at the marker's centre must reach the window below it.
                if backend.point_intercepted_by_marker(
                    spec.x + spec.size / 2,
                    spec.y + spec.size / 2,
                ) {
                    intercepted += 1;
                }
            }
            println!(
                "four_sentinels={} intercepted_by_marker={}/{} live_marker_windows={}",
                live.len(),
                intercepted,
                live.len(),
                backend.live_marker_windows()
            );
            for (id, _) in &live {
                let _ = platform.destroy_marker(*id);
            }
            let _ = platform.sync_presentation();
            println!("after_teardown_live_marker_windows = {}", backend.live_marker_windows());
        }

        // ---- end to end: the assembled engine, gate included ----
        println!();
        println!("== assembled engine (builder -> gate -> L0 Anchor) ==");
        let started = Instant::now();
        match FidusBuilder::new().build_with(BackendChoice::Windows) {
            Ok(mut engine) => {
                let environment = engine.environment().clone();
                println!(
                    "engine_environment = layer_shell:{} multi_marker:{} monitors:{} composite:{:?}",
                    environment.has_layer_shell,
                    environment.multi_marker_projection,
                    environment.multi_monitor_count,
                    environment.compositor_type
                );
                for method in [CalibrationMethod::Crosshair, CalibrationMethod::Anchor] {
                    println!(
                        "gate_{:?} = {:?}",
                        method,
                        engine.gate().query_calibrator_availability(method)
                    );
                }
                match engine.calibrate() {
                    Ok(frame) => {
                        let quality = frame.quality();
                        println!("calibrate = OK method {:?}", frame.method());
                        println!("calibrate_map_scale = {:.6}", frame.map().linear_scale());
                        println!("calibrate_map_coefficients = {:?}", frame.map().coefficients());
                        println!("calibrate_rms_residual_px = {:.3}", quality.rms_residual_px);
                        println!("calibrate_max_residual_px = {:.3}", quality.max_residual_px);
                        println!(
                            "calibrate_verification_max_err_px = {:.3}",
                            quality.verification_max_err_px
                        );
                        println!(
                            "calibrate_consistency_max_err_px = {:.3}",
                            quality.consistency_max_err_px
                        );
                        println!("calibrate_sample_count = {}", quality.sample_count);
                        println!("calibrate_capture_size = {:?}", frame.capture_size());
                    }
                    Err(e) => println!("calibrate = FAILED {e}"),
                }
                println!("calibrate_wall_ms = {:.0}", started.elapsed().as_secs_f64() * 1000.0);
                // The frame outlives the overlay: estimation needs no projector.
                match engine.estimate() {
                    Ok(estimate) => println!("estimate = ok confidence {:.4}", estimate.confidence),
                    Err(e) => println!("estimate = {e} (expected without a registered target)"),
                }
            }
            Err(e) => println!("builder = FAILED {e}"),
        }
        println!(
            "final_live_marker_windows = {} (must be 0: no residual overlay)",
            backend.live_marker_windows()
        );

        capture_ms.sort_by(f64::total_cmp);
        if !capture_ms.is_empty() {
            println!(
                "capture_timing = n={} min {:.2}ms median {:.2}ms max {:.2}ms",
                capture_ms.len(),
                capture_ms[0],
                capture_ms[capture_ms.len() / 2],
                capture_ms[capture_ms.len() - 1]
            );
        }
    }
}
