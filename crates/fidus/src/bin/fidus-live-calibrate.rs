// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! fidus-live-calibrate — live smoke test: the full calibration loop on
//! whatever backend the builder picks (layer-shell first, then X11).
//!
//! Stages (each prints before the next runs, so failures localize):
//!   1. backend connect + probed environment (via the engine builder)
//!   2. gate answers for every calibration method
//!   3. full calibration through the engine with the gate-selected method
//!   4. teardown reminder (`niri msg layers` / `xwininfo -root -tree` must
//!      show no fidus windows)
//!
//! `FIDUS_BACKEND=wayland|x11` forces a backend; `FIDUS_METHOD=crosshair|anchor`
//! prefers a calibrator (the gate still decides).
//!
//! Exit codes: 0 success, 1 calibration failed, 2 init/connect failed,
//! 3 gate says nothing is available.

use fidus::prelude::*;

fn main() {
    let choice = match std::env::var("FIDUS_BACKEND").as_deref() {
        Ok("wayland") | Ok("wayland-layer") => BackendChoice::WaylandLayer,
        Ok("x11") => BackendChoice::X11,
        _ => BackendChoice::Auto,
    };
    let mut builder = FidusBuilder::new();
    match std::env::var("FIDUS_METHOD").as_deref() {
        Ok("anchor") => builder = builder.prefer_method(CalibrationMethod::Anchor),
        Ok("crosshair") => builder = builder.prefer_method(CalibrationMethod::Crosshair),
        _ => {}
    }

    // Stage 1: connect + probe.
    let mut engine = match builder.build_with(choice) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[stage 1] backend init failed ({choice:?}): {e}");
            std::process::exit(2);
        }
    };
    println!("[stage 1] environment: {:?}", engine.environment());

    // Stage 2: gate answers, all methods.
    let mut any = false;
    for method in CalibrationMethod::ALL {
        let status = engine.gate().query_calibrator_availability(method);
        any |= status.is_usable();
        println!("[stage 2] gate {:>16}: {status:?}", method.name());
    }
    if !any {
        eprintln!("[stage 2] no calibration method is usable here");
        std::process::exit(3);
    }

    // Stage 3: calibrate with the gate-selected method.
    let t0 = std::time::Instant::now();
    match engine.calibrate() {
        Ok(frame) => {
            let q = frame.quality();
            let m = frame.map();
            println!(
                "[stage 3] calibrated in {:.2}s: {}, capture {}x{}, linear scale {:.4}",
                t0.elapsed().as_secs_f64(),
                frame.method().name(),
                frame.capture_size().0,
                frame.capture_size().1,
                m.linear_scale()
            );
            let [a, b, c, d, e, f] = m.coefficients();
            println!(
                "[stage 3] map: [{a:.4} {b:.4} {c:.2}; {d:.4} {e:.4} {f:.2}]"
            );
            println!(
                "[stage 3] quality: rms {:.3}px, max {:.3}px, verify {:.3}px, consistency {:.3}px, samples {}, passes {}",
                q.rms_residual_px, q.max_residual_px, q.verification_max_err_px,
                q.consistency_max_err_px, q.sample_count, q.independent_passes
            );
        }
        Err(e) => {
            eprintln!("[stage 3] calibration failed: {e}");
            eprintln!("[stage 4] verify teardown externally (see below)");
            std::process::exit(1);
        }
    }

    // Stage 4: teardown is enforced inside calibrate(); verify externally.
    println!(
        "[stage 4] verify teardown: `niri msg layers | grep fidus` (Wayland) / `xwininfo -root -tree` (X11) — expect no fidus windows"
    );
}
