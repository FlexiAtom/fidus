//! fidus-calibrate — P1 smoke test: the full L9 loop on the live compositor.
//!
//! Stages (each prints before the next runs, so failures localize):
//!   1. connect + primitive probe on a dedicated backend connection
//!   2. one screen capture (screencopy path)
//!   3. gate query + full Crosshair calibration through the engine
//!   4. teardown reminder (`niri msg layers` must show no fidus-calib)
//!
//! Exit codes: 0 success, 1 calibration failed, 2 init/connect failed,
//! 3 gate says unavailable.

use fidus::prelude::*;
use fidus::wayland::WaylandLayerBackend;
use fidus::FidusBuilder;

fn main() {
    // Stage 1+2: raw backend diagnostics on a dedicated connection, so
    // capture problems are separated from engine-flow problems.
    let mut backend = match WaylandLayerBackend::connect() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[stage 1] connect failed: {e}");
            std::process::exit(2);
        }
    };
    println!("[stage 1] primitives available: {}", backend.primitives_available());
    match backend.capture_once() {
        Ok(frame) => println!(
            "[stage 2] capture ok: {}x{} px, stride {}, format {:?}",
            frame.width, frame.height, frame.stride, frame.format
        ),
        Err(e) => {
            eprintln!("[stage 2] capture failed: {e}");
            std::process::exit(2);
        }
    }
    drop(backend);

    // Stage 3: the full public flow on a fresh connection. A fixed seed
    // keeps diagnostic runs reproducible while we chase the noise: two runs
    // with the same seed re-sample the same positions, so centroid deltas
    // between runs isolate environment noise exactly.
    let mut engine = match FidusBuilder::new()
        .with_crosshair_config(CrosshairConfig {
            seed: Some(42),
            ..CrosshairConfig::default()
        })
        .build()
    {
        Ok(e) => e,
            Err(e) => {
            eprintln!("[stage 3] calibration failed: {e}");
            // exit(1) skips stage 4 below, so remind here — teardown ran via
            // the calibrator + session Drop; verify externally.
            eprintln!("[stage 4] verify teardown: `niri msg layers | grep -i fidus` — expect no output");
            std::process::exit(1);
        }
    };
    let status = engine.gate().query_calibrator_availability(CalibrationMethod::Crosshair);
    println!("[stage 3] gate: {status:?}");
    if !status.is_usable() {
        std::process::exit(3);
    }

    let t0 = std::time::Instant::now();
    match engine.calibrate() {
        Ok(frame) => {
            let q = frame.quality();
            println!(
                "[stage 3] calibrated in {:.1}s: {}, capture {}x{}, linear scale {:.4}",
                t0.elapsed().as_secs_f64(),
                frame.method().name(),
                frame.capture_size().0,
                frame.capture_size().1,
                frame.map().linear_scale()
            );
            println!(
                "[stage 3] quality: rms {:.3}px, max {:.3}px, verify {:.3}px, consistency {:.3}px, samples {}, passes {}",
                q.rms_residual_px, q.max_residual_px, q.verification_max_err_px,
                q.consistency_max_err_px, q.sample_count, q.independent_passes
            );
        }
        Err(e) => {
            eprintln!("[stage 3] calibration failed: {e}");
            std::process::exit(1);
        }
    }

    // Stage 4: teardown is enforced by the session's Drop inside calibrate();
    // verify externally that no layer surface lingers.
    println!("[stage 4] verify teardown: `niri msg layers | grep fidus` — expect no output");
}
