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

fn protocol_context() -> (String, String) {
    let mode = match std::env::var("FIDUS_EXECUTION_MODE").as_deref() {
        Ok("ci") => "ci",
        Ok("live-host") => "live-host",
        Ok("live-container") => "live-container",
        Ok(_) => "invalid",
        Err(_) => "live-host",
    };
    let requested_id =
        std::env::var("FIDUS_RUN_ID").unwrap_or_else(|_| format!("pid{}", std::process::id()));
    let run_id = if !requested_id.is_empty()
        && requested_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        requested_id
    } else {
        format!("pid{}", std::process::id())
    };
    (mode.to_owned(), run_id)
}

fn emit_result(line: String) {
    println!("{line}");
}

fn emit_summary(run_id: &str, mode: &str, status: &str, total: usize, ok: usize) {
    emit_result(format!(
        "FIDUS_RESULT version=1 kind=summary run_id={run_id} status={status} execution_mode={mode} records_total={total} records_ok={ok} records_failed={}",
        total - ok
    ));
}

fn choice_name(choice: BackendChoice) -> &'static str {
    match choice {
        BackendChoice::Auto => "auto",
        BackendChoice::WaylandLayer => "wayland",
        BackendChoice::X11 => "x11",
    }
}

fn method_token(method: CalibrationMethod) -> &'static str {
    match method {
        CalibrationMethod::Crosshair => "crosshair",
        CalibrationMethod::GradientField => "gradient-field",
        CalibrationMethod::Anchor => "anchor",
    }
}

fn compositor_token(kind: &fidus_core::env::CompositorKind) -> &'static str {
    match kind {
        fidus_core::env::CompositorKind::Niri => "niri",
        fidus_core::env::CompositorKind::Sway => "sway",
        fidus_core::env::CompositorKind::Hyprland => "hyprland",
        fidus_core::env::CompositorKind::KWin => "kwin",
        fidus_core::env::CompositorKind::Mutter => "mutter",
        fidus_core::env::CompositorKind::Weston => "weston",
        fidus_core::env::CompositorKind::Labwc => "labwc",
        fidus_core::env::CompositorKind::Wayfire => "wayfire",
        fidus_core::env::CompositorKind::X11 => "x11",
        fidus_core::env::CompositorKind::Other(_) => "other",
        fidus_core::env::CompositorKind::Unknown => "unknown",
    }
}

fn main() {
    let (mode, run_id) = protocol_context();
    if mode == "invalid" {
        eprintln!("harness error: unknown FIDUS_EXECUTION_MODE");
        std::process::exit(3);
    }
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
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=environment run_id={run_id} status=unavailable execution_mode={mode} backend=unknown compositor=unknown output=unknown scale=not-measured transform=not-measured"
            ));
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=lifecycle run_id={run_id} status=failed execution_mode={mode} teardown=not_started recovery=not_requested"
            ));
            emit_summary(&run_id, &mode, "failed", 2, 0);
            std::process::exit(2);
        }
    };
    let environment = engine.environment();
    println!("[stage 1] environment: {environment:?}");
    emit_result(format!(
        "FIDUS_RESULT version=1 kind=environment run_id={run_id} status=ready execution_mode={mode} backend={} compositor={} output=not-measured scale=not-measured transform=not-measured",
        choice_name(choice),
        compositor_token(&environment.compositor_type)
    ));

    // Stage 2: gate answers, all methods.
    let mut any = false;
    for method in CalibrationMethod::ALL {
        let status = engine.gate().query_calibrator_availability(method);
        any |= status.is_usable();
        println!("[stage 2] gate {:>16}: {status:?}", method.name());
    }
    if !any {
        eprintln!("[stage 2] no calibration method is usable here");
        emit_result(format!(
            "FIDUS_RESULT version=1 kind=calibration run_id={run_id} status=failed execution_mode={mode} backend={} method=none",
            choice_name(choice)
        ));
        emit_result(format!(
            "FIDUS_RESULT version=1 kind=lifecycle run_id={run_id} status=failed execution_mode={mode} teardown=not_started recovery=not_requested"
        ));
        emit_summary(&run_id, &mode, "failed", 3, 1);
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
            println!("[stage 3] map: [{a:.4} {b:.4} {c:.2}; {d:.4} {e:.4} {f:.2}]");
            println!(
                "[stage 3] quality: rms {:.3}px, max {:.3}px, verify {:.3}px, consistency {:.3}px, samples {}, passes {}",
                q.rms_residual_px,
                q.max_residual_px,
                q.verification_max_err_px,
                q.consistency_max_err_px,
                q.sample_count,
                q.independent_passes
            );
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=calibration run_id={run_id} status=ok execution_mode={mode} backend={} method={} rms_residual_px={:.6} verification_max_err_px={:.6} consistency_max_err_px={:.6}",
                choice_name(choice),
                method_token(frame.method()),
                q.rms_residual_px,
                q.verification_max_err_px,
                q.consistency_max_err_px
            ));
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=lifecycle run_id={run_id} status=ok execution_mode={mode} teardown=confirmed recovery=not_requested"
            ));
            emit_summary(&run_id, &mode, "ok", 3, 3);
        }
        Err(e) => {
            eprintln!("[stage 3] calibration failed: {e}");
            eprintln!(
                "[stage 4] teardown is enforced internally; external compositor queries are diagnostics only"
            );
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=calibration run_id={run_id} status=failed execution_mode={mode} backend={} method=selected",
                choice_name(choice)
            ));
            emit_result(format!(
                "FIDUS_RESULT version=1 kind=lifecycle run_id={run_id} status=failed execution_mode={mode} teardown=unknown recovery=not_requested"
            ));
            emit_summary(&run_id, &mode, "failed", 3, 1);
            std::process::exit(1);
        }
    }

    // Stage 4: teardown is enforced inside calibrate(); verify externally.
    println!(
        "[stage 4] verify teardown: `niri msg layers | grep fidus` (Wayland) / `xwininfo -root -tree` (X11) — expect no fidus windows"
    );
}
