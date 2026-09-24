// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! A target whose appearance repeats on a coarse period: what the registration
//! gate now says about it, and what the estimator does when a host overrides
//! that verdict.
//!
//! This file began as the measurement behind `finished/ceiling-metric-lag-window.md`:
//! `localizability()` used to sample self-similarity at radii `[2, 4, 8, 16]`
//! only, so an appearance repeating every 64 px registered with a ceiling of
//! 0.946 while two candidate windows sat equally good, one period apart — a
//! stable mis-lock reported at full credibility. The dense axis sweep
//! (`template.rs`, `dense_axis_worst`) closed that: the same template is now
//! refused by default, and a host that insists gets the *floor* as its ceiling
//! rather than the near-`1.0` the old probe handed out.
//!
//! What has **not** changed is the position error itself: the sweep labels the
//! ambiguity, it does not resolve it. The rows below still measure a one-period
//! mis-lock, which is why the opt-in exists and why the label is the whole
//! defence.
//!
//! No display, no compositor, no calibration mutation: a scripted screen built
//! from repeated blocks, driven through the same `FusedEstimator` production
//! uses. Run with `--nocapture` to read the table.

use std::time::{Duration, Instant, SystemTime};

use fidus_core::calibration::CalibrationMethod;
use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint, SolvedMap};
use fidus_core::engine::Estimator;
use fidus_core::frame::{CalibrationQuality, CoordinateFrame};
use fidus_core::io::{CaptureError, CaptureIo, Frame, PixelFormat};
use fidus_core::target::{RgbaImage, TargetDescription};
use fidus_estimate::FusedEstimator;

const W: u32 = 620;
const H: u32 = 420;
/// One period of the repeated content, in logical pixels — twice the largest
/// radius the gate probes.
const AW: u32 = 32;
const AH: u32 = 32;
/// Where the leftmost cell sits on screen.
const SX: f64 = 60.0;
const SY: f64 = 40.0;
const FRAMES: usize = 6;

/// Content of cell `half` at logical offset `(x, y)` within that cell. `half`
/// is the cell's identity: two cells with the same `half` are pixel-identical.
fn cell_luma(half: u32, x: u32, y: u32) -> u8 {
    let mut n =
        x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B) ^ half.wrapping_mul(0xC2B2_AE35);
    n ^= n >> 13;
    n = n.wrapping_mul(0xC2B2_AE35);
    n ^= n >> 16;
    40 + (n as u8 % 180)
}

fn solved_2x() -> SolvedMap {
    let corr: Vec<_> = [(0.0, 0.0), (400.0, 0.0), (0.0, 300.0), (400.0, 300.0)]
        .into_iter()
        .map(|(x, y)| {
            (
                LogicalPoint::new(x, y),
                PhysicalPoint::new(x * 2.0, y * 2.0),
            )
        })
        .collect();
    AffineTransform::from_correspondences(&corr).expect("well-conditioned")
}

fn frame() -> CoordinateFrame {
    let quality = CalibrationQuality {
        rms_residual_px: 0.0,
        max_residual_px: 0.0,
        verification_max_err_px: 0.0,
        consistency_max_err_px: 0.0,
        sample_count: 4,
        independent_passes: 2,
    };
    CoordinateFrame::new(
        solved_2x(),
        (W, H),
        CalibrationMethod::Crosshair,
        quality,
        SystemTime::now(),
    )
    .expect("invertible")
}

/// `[A|A]` when `periodic`, i.e. two identical halves with a 64 px period;
/// `[A|B]` otherwise, which has no coarse repetition and serves as the control.
fn make_template(periodic: bool) -> RgbaImage {
    let tw = AW * 2;
    let mut data = Vec::with_capacity((tw * AH * 4) as usize);
    for y in 0..AH {
        for x in 0..tw {
            let half = if periodic || x < AW { 0 } else { 1 };
            let luma = cell_luma(half, x % AW, y);
            data.extend_from_slice(&[luma, luma, luma, 255]);
        }
    }
    RgbaImage::from_raw(tw, AH, data)
}

/// Cells laid out left to right from `SX`, one `AW × AH` block each.
/// `[A, A, A]` gives a periodic appearance with two *equally* valid template
/// windows (copies 0+1 and 1+2, one period apart); `[A, B]` gives the control
/// with exactly one valid window. `perturb_first` flips the low bits of copy 0
/// — a degradation NCC cannot normalize away, unlike a contrast change.
struct Scene {
    cells: Vec<u32>,
    perturb_first: bool,
}

impl CaptureIo for Scene {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        let format = PixelFormat::Argb8888;
        let mut data = vec![90u8; (W * H * 4) as usize];
        for (c, &half) in self.cells.iter().enumerate() {
            for yy in 0..AH {
                for xx in 0..AW {
                    let mut luma = u32::from(cell_luma(half, xx, yy));
                    if self.perturb_first && c == 0 {
                        luma ^= 3;
                    }
                    let luma = luma as u8;
                    let x0 = ((SX + f64::from(c as u32 * AW + xx)) * 2.0).round() as u32;
                    let y0 = ((SY + f64::from(yy)) * 2.0).round() as u32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (px, py) = (x0 + dx, y0 + dy);
                            if px >= W || py >= H {
                                continue;
                            }
                            let i = (py * W + px) as usize * 4;
                            format.write_rgba(&mut data, i, [luma, luma, luma, 255]);
                        }
                    }
                }
            }
        }
        Ok(Frame {
            width: W,
            height: H,
            stride: W * 4,
            format,
            data,
        })
    }
}

struct Clock {
    t: Instant,
}

impl Clock {
    fn new() -> Self {
        Clock { t: Instant::now() }
    }
    fn tick(&mut self) -> Instant {
        self.t += Duration::from_millis(600);
        self.t
    }
}

/// Registers, then reports where the estimate actually sits, relative to the
/// window whose left edge is copy 0 — the reading a host that believes the
/// target starts at `SX` would call "correct".
fn probe(
    label: &str,
    periodic: bool,
    cells: Vec<u32>,
    perturb_first: bool,
    guess_shift: f64,
    opt_in: bool,
) -> Option<(f64, f32)> {
    let mut clock = Clock::new();
    let shared = clock.t;
    let mut scene = Scene {
        cells,
        perturb_first,
    };
    let mut est = FusedEstimator::with_clock(Box::new(move || shared));

    let center = LogicalPoint::new(SX + f64::from(AW) + guess_shift, SY + f64::from(AH / 2));
    let mut target = TargetDescription::new(make_template(periodic)).with_initial_center(center);
    if opt_in {
        target = target.tracking_ambiguous_appearance();
    }
    if est.register_target(target).is_err() {
        println!("{label:<38} REFUSED by the gate");
        return None;
    }
    let ceiling = est.confidence_ceiling();

    let mut dx_last = f64::NAN;
    let mut conf_last = f32::NAN;
    let mut dx_first = f64::NAN;
    let mut ok = 0;
    for i in 0..FRAMES {
        let t = clock.tick();
        est.set_clock(Box::new(move || t));
        match est.estimate(&mut scene, &frame()) {
            Ok(p) => {
                let dx = p.position.x - (SX + f64::from(AW));
                if i == 0 {
                    dx_first = dx;
                }
                dx_last = dx;
                conf_last = p.confidence;
                ok += 1;
            }
            Err(err) => {
                if i == 0 {
                    println!("{label:<38} frame 0: {err}");
                }
            }
        }
    }
    println!(
        "{label:<38} ceiling={ceiling:?} ok={ok}/{FRAMES} dx_first={dx_first:>8.2} \
         dx_last={dx_last:>8.2} conf_last={conf_last:.4}"
    );
    (ok > 0).then_some((dx_last, conf_last))
}

/// The gate's verdict on the shape this file was written about, both policies.
#[test]
fn a_coarse_period_is_refused_by_default_and_floored_when_overridden() {
    let shared = Clock::new().t;
    let mut est = FusedEstimator::with_clock(Box::new(move || shared));
    let err = est
        .register_target(TargetDescription::new(make_template(true)))
        .expect_err("[A|A] repeats at 64 px, inside its own 64 px extent: the sweep must see it");
    println!("default policy: {err}");

    let mut opted = FusedEstimator::with_clock(Box::new(move || shared));
    opted
        .register_target(
            TargetDescription::new(make_template(true)).tracking_ambiguous_appearance(),
        )
        .expect("an explicit opt-in must still produce a track");
    let ceiling = opted.confidence_ceiling().expect("registered");

    let mut control = FusedEstimator::with_clock(Box::new(move || shared));
    control
        .register_target(TargetDescription::new(make_template(false)))
        .expect("[A|B] must clear the gate");
    let ceiling_control = control.confidence_ceiling().expect("registered");

    println!("ceiling: opted-in [A|A] = {ceiling}, aperiodic [A|B] = {ceiling_control}");
    // 0.946 is what this template used to be allowed to claim, because the
    // probe stopped at 16 px. Anything near that is the hole reopening.
    assert!(ceiling <= 0.06, "a fully ambiguous repeat must sit at the floor, got {ceiling}");
    assert!(ceiling_control > 0.9, "the aperiodic control must stay undiscounted: {ceiling_control}");
}

/// The scenario the finding is about: an identical region sits **one period
/// away** from the target, so the NCC surface has two peaks of exactly equal
/// score and nothing in the appearance separates them.
///
/// Reached through the opt-in, which is the only way this shape can be tracked
/// at all now. The mis-lock is still a mis-lock — the sweep labels ambiguity,
/// it does not resolve it — but it is no longer silent: the reading sits at the
/// discounted ceiling instead of at the 0.95 the old probe handed out.
///
/// This pins what the engine does today, not what it ought to do. Changing how
/// an equal-peak tie breaks must change these expectations on purpose.
#[test]
fn an_equal_peak_one_period_away_is_broken_by_search_order_not_evidence() {
    let truth = SX + f64::from(2 * AW); // centre of the window over copies 1+2
    let decoy = SX + f64::from(AW); // centre of the window over copies 0+1

    for (belief, name) in [
        (truth, "belief on the truth"),
        (decoy, "belief on the decoy"),
    ] {
        let mut clock = Clock::new();
        let shared = clock.t;
        let mut scene = Scene {
            cells: vec![0, 0, 0],
            perturb_first: false,
        };
        let mut est = FusedEstimator::with_clock(Box::new(move || shared));
        est.register_target(
            TargetDescription::new(make_template(true))
                .with_initial_center(LogicalPoint::new(belief, SY + f64::from(AH / 2)))
                .tracking_ambiguous_appearance(),
        )
        .expect("[A|A] must track once the host insists");
        let ceiling = est.confidence_ceiling().expect("registered");

        let mut errs = Vec::new();
        let mut confs = Vec::new();
        for _ in 0..FRAMES {
            let t = clock.tick();
            est.set_clock(Box::new(move || t));
            let p = est.estimate(&mut scene, &frame()).expect("a match");
            errs.push(p.position.x - truth);
            confs.push(p.confidence);
        }
        let e = errs
            .iter()
            .map(|d| format!("{d:.0}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{name:<20} belief={belief} truth={truth} ceiling={ceiling} \
             err = [{e}] conf_last = {:.4}",
            *confs.last().unwrap()
        );

        // Measured, both rows: every frame reports the decoy, one full period
        // left of the truth. The position is still wrong.
        assert!(
            errs.iter().all(|d| (d + f64::from(AW)).abs() < 2.0),
            "{name}: expected a stable lock one period ({AW} px) left of the truth, got [{e}]"
        );
        // …and every frame says so, at a ceiling the sweep computed from the
        // template's own repetition rather than from the fine radii.
        assert!(
            ceiling <= 0.06,
            "{name}: the opt-in must inherit the discount, got ceiling {ceiling}"
        );
        assert!(
            confs.iter().all(|c| (c - ceiling).abs() < 1e-3),
            "{name}: the mislock must be reported at the ceiling, not above it: {confs:?}"
        );
    }
}

#[test]
fn equal_peaks_one_period_apart_report_which_one_won() {
    println!("period = {AW} logical px; dense axis sweep covers lags 2..=64; twin spacing = {AW} px");
    let periodic = vec![0, 0, 0];
    let aperiodic = vec![0, 1];
    // The default policy first, as its own row: nothing below this shape can
    // reach a matcher unless the host asked for it.
    let refused = probe("R0 periodic, default policy", true, periodic.clone(), false, 0.0, false);
    let tie = probe("R1 periodic, opt-in, guess correct", true, periodic.clone(), false, 0.0, true);
    let twin_better = probe(
        "R2 periodic, opt-in, copy0 perturbed",
        true,
        periodic.clone(),
        true,
        0.0,
        true,
    );
    let guess_twin = probe(
        "R3 periodic, opt-in, guess one period off",
        true,
        periodic.clone(),
        false,
        f64::from(AW),
        true,
    );
    let guess_twin_twin_better = probe(
        "R4 periodic, opt-in, guess off + copy0 bad",
        true,
        periodic,
        true,
        f64::from(AW),
        true,
    );
    let unique = probe(
        "R5 control [A|B], guess correct",
        false,
        aperiodic.clone(),
        false,
        0.0,
        false,
    );
    probe(
        "R6 control [A|B], guess 24 off",
        false,
        aperiodic,
        false,
        24.0,
        false,
    );

    assert!(refused.is_none(), "R0: the gate must refuse, not report a position");
    // R2 is the decisive row: window 0 is now *strictly worse* than window 1, so
    // a score-driven chooser must move and a belief-driven one must not.
    let (dx2, conf2) = twin_better.expect("R2 must match");
    println!(
        "decisive row R2: dx={dx2:.2} conf={conf2:.4} (one period = {AW} logical px); \
         R3 belief was at +{AW}, ended at {:?}",
        guess_twin.map(|(dx, _)| format!("{dx:.2}"))
    );
    assert!(
        dx2.abs() < 2.0 || (dx2 - f64::from(AW)).abs() < 2.0,
        "R2 must land on one of the two candidate windows, not between them, got {dx2}"
    );
    assert!(tie.is_some() && guess_twin.is_some() && guess_twin_twin_better.is_some());
    // The control is what makes the periodic numbers readable: a unique
    // appearance of the same texture must localize, and report higher
    // confidence than the equal-peak case does.
    let (dx5, conf5) = unique.expect("the control appearance is on screen");
    assert!(
        dx5.abs() < 2.0,
        "control must sit on the only valid window, got {dx5}"
    );
    assert!(
        conf5 > conf2,
        "a unique appearance must outrank an equal-peak one: {conf5} vs {conf2}"
    );
}
