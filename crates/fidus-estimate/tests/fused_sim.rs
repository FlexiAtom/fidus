// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! End-to-end fusion tests: L1 + L8 + constant-velocity track against a
//! scripted fake output with an injectable clock.

use std::time::{Duration, Instant, SystemTime};

use fidus_core::calibration::CalibrationMethod;
use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint, SolvedMap};
use fidus_core::engine::Estimator;
use fidus_core::estimate::EstimateError;
use fidus_core::frame::{CalibrationQuality, CoordinateFrame};
use fidus_core::io::{CaptureError, CaptureIo, Frame, PixelFormat};
use fidus_core::target::{RgbaImage, TargetDescription};
use fidus_estimate::FusedEstimator;

const W: u32 = 1000;
const H: u32 = 750;
const TW: u32 = 60; // logical template size; physical is 2× (120×80)
const TH: u32 = 40;

/// 500×375 logical output captured at 2×.
///
/// The map is *solved* from synthetic correspondences rather than written
/// down: `CoordinateFrame` only accepts a `SolvedMap`, which is how principle
/// 1 is enforced at the type level. Tests take the same road as production —
/// the one thing a test must never do is get a privileged shortcut past the
/// invariant it is supposed to be exercising.
fn frame() -> CoordinateFrame {
    let map = solved_2x();
    let quality = CalibrationQuality {
        rms_residual_px: 0.0,
        max_residual_px: 0.0,
        verification_max_err_px: 0.0,
        consistency_max_err_px: 0.0,
        sample_count: 4,
        independent_passes: 2,
    };
    CoordinateFrame::new(map, (W, H), CalibrationMethod::Crosshair, quality, SystemTime::now())
        .expect("invertible")
}

/// Solves the exact `physical = 2 · logical` map from four corner
/// correspondences, mimicking what a calibrator measures.
fn solved_2x() -> SolvedMap {
    let corr: Vec<_> = [(0.0, 0.0), (400.0, 0.0), (0.0, 300.0), (400.0, 300.0)]
        .into_iter()
        .map(|(x, y)| (LogicalPoint::new(x, y), PhysicalPoint::new(x * 2.0, y * 2.0)))
        .collect();
    AffineTransform::from_correspondences(&corr).expect("well-conditioned")
}

/// A deterministic, non-periodic texture matching the template module's
/// verified `hash_luma` fixture. The low hash byte is intentionally used: its
/// self-similarity is measured at 0.057 with every axis lag up to half the
/// template's extent swept, so the fusion tests below are not running on a
/// silently discounted track.
/// `salt` changes the appearance for animation/coasting cases but is zero for
/// the registered render.
fn pattern(x: u32, y: u32, salt: u32) -> [u8; 4] {
    let mut n = x.wrapping_mul(0x9E37_79B1)
        ^ y.wrapping_mul(0x85EB_CA6B)
        ^ salt.wrapping_mul(0xC2B2_AE35);
    n ^= n >> 13;
    n = n.wrapping_mul(0xC2B2_AE35);
    n ^= n >> 16;
    let luma = n as u8;
    [luma, luma, luma, 255]
}

fn template(salt: u32) -> RgbaImage {
    let mut data = Vec::with_capacity((TW * TH * 4) as usize);
    for y in 0..TH {
        for x in 0..TW {
            data.extend_from_slice(&pattern(x, y, salt));
        }
    }
    RgbaImage::from_raw(TW, TH, data)
}

fn gradient_template() -> RgbaImage {
    let mut data = Vec::with_capacity((TW * TH * 4) as usize);
    for _y in 0..TH {
        for x in 0..TW {
            let luma = (32.0 + 190.0 * x as f64 / (TW - 1) as f64).round() as u8;
            data.extend_from_slice(&[luma, luma, luma, 255]);
        }
    }
    RgbaImage::from_raw(TW, TH, data)
}

/// A fake output the test scripts: target at a logical top-left (or absent),
/// with an animatable content salt.
struct FakeOutput {
    top_left_logical: Option<(f64, f64)>,
    salt: u32,
    gradient: bool,
}

impl CaptureIo for FakeOutput {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        let format = PixelFormat::Argb8888;
        let mut data = vec![90u8; (W * H * 4) as usize];
        if let Some((lx, ly)) = self.top_left_logical {
            // Logical → physical at 2×: top-left scales, size doubles.
            let (tx, ty) = ((lx * 2.0).round() as i64, (ly * 2.0).round() as i64);
            for yy in 0..TH * 2 {
                for xx in 0..TW * 2 {
                    let px = tx + xx as i64;
                    let py = ty + yy as i64;
                    if px < 0 || py < 0 || px >= W as i64 || py >= H as i64 {
                        continue;
                    }
                    let i = (py as u32 * W + px as u32) as usize * 4;
                    let pixel = if self.gradient {
                        let luma = (32.0 + 190.0 * (xx / 2) as f64 / (TW - 1) as f64).round() as u8;
                        [luma, luma, luma, 255]
                    } else {
                        pattern(xx / 2, yy / 2, self.salt)
                    };
                    format.write_rgba(&mut data, i, pixel);
                }
            }
        }
        Ok(Frame { width: W, height: H, stride: W * 4, format, data })
    }
}

/// The registered appearance drawn at two independent places, so a test can
/// put a copy on screen that the caller does not consider the target.
/// Appearance alone cannot tell these apart — that is the whole point.
struct TwoInstances {
    near: Option<(f64, f64)>,
    far: Option<(f64, f64)>,
}

impl CaptureIo for TwoInstances {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        let format = PixelFormat::Argb8888;
        let mut data = vec![90u8; (W * H * 4) as usize];
        for (tx, ty) in [self.near, self.far].into_iter().flatten().map(|(lx, ly)| {
            ((lx * 2.0).round() as i64, (ly * 2.0).round() as i64)
        }) {
            for yy in 0..TH * 2 {
                for xx in 0..TW * 2 {
                    let (px, py) = (tx + xx as i64, ty + yy as i64);
                    if px < 0 || py < 0 || px >= W as i64 || py >= H as i64 {
                        continue;
                    }
                    let i = (py as u32 * W + px as u32) as usize * 4;
                    format.write_rgba(&mut data, i, pattern(xx / 2, yy / 2, 0));
                }
            }
        }
        Ok(Frame { width: W, height: H, stride: W * 4, format, data })
    }
}

/// A scripted clock stepping 600 ms per estimate, so every L8 observation
/// crosses the differ window.
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

fn estimator(clock: &Clock) -> FusedEstimator {
    let shared = clock.t;
    let mut e = FusedEstimator::with_clock(Box::new(move || shared));
    e.register_target(
        TargetDescription::new(template(0))
            .with_initial_center(LogicalPoint::new(200.0, 150.0)),
    )
    .expect("valid target");
    e
}

/// Runs one estimate with the clock advanced by one window.
fn step(
    est: &mut FusedEstimator,
    io: &mut dyn CaptureIo,
    clock: &mut Clock,
) -> Result<fidus_core::estimate::ProbabilisticPosition, EstimateError> {
    let t = clock.tick();
    est.set_clock(Box::new(move || t));
    est.estimate(io, &frame())
}

#[test]
fn ambiguous_template_ceiling_survives_fused_kalman_path() {
    let mut clock = Clock::new();
    let shared = clock.t;
    let mut est = FusedEstimator::with_clock(Box::new(move || shared));
    assert_eq!(
        est.confidence_ceiling(),
        None,
        "an estimator without a registered target must not advertise a bound"
    );
    est.register_target(
        TargetDescription::new(gradient_template())
            .with_initial_center(LogicalPoint::new(200.0, 150.0))
            .tracking_ambiguous_appearance(),
    )
    .expect("explicit ambiguity opt-in");
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0, gradient: true };

    let p = step(&mut est, &mut io, &mut clock).expect("gradient match");
    assert!(p.confidence <= 0.05 + f32::EPSILON, "ceiling bypassed: {}", p.confidence);
    // The bound the API advertises must be the bound the layers applied
    // (project convention 7): a host that reads `confidence_ceiling` to tell
    // "capped by a weak appearance" from "genuinely low this frame" would be
    // lied to if the fused path delegated elsewhere or reported nothing.
    assert!(
        est.confidence_ceiling().is_some_and(|cap| p.confidence <= cap + f32::EPSILON
            && (cap - 0.05).abs() < 0.01),
        "advertised ceiling does not match the applied cap: {:?}",
        est.confidence_ceiling()
    );
}

#[test]
fn normal_fixture_is_localizable_not_ceiling_limited() {
    let similarity = fidus_estimate::template::localizability(&template(0))
        .expect("normal tracking fixture must be localizable");
    assert!(similarity < 0.98, "fixture became ambiguous: {similarity}");
}

#[test]
fn acquires_from_belief_and_follows_a_drag() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0, gradient: false };

    // First fix at the caller's belief (center at 200,150 logical).
    let p = step(&mut est, &mut io, &mut clock).expect("first fix");
    assert!((p.position.x - 200.0).abs() < 2.0, "x = {}", p.position.x);
    assert!((p.position.y - 150.0).abs() < 2.0);
    assert!(p.confidence > 0.25, "confidence = {}", p.confidence);

    // A drag: 20 logical px per 600 ms step, 8 steps.
    for k in 1..=8 {
        io.top_left_logical = Some((170.0 + 20.0 * k as f64, 130.0 + 8.0 * k as f64));
        let p = step(&mut est, &mut io, &mut clock).expect("tracked");
        let want_x = 200.0 + 20.0 * k as f64;
        let want_y = 150.0 + 8.0 * k as f64;
        // The first step after acquisition is one velocity behind: the constant
        // velocity track needs a differ window before it predicts the drag at
        // all, and a discounted appearance lowers the gain that pulls it back.
        // Measured for this fixture: 9.8 px in x, 3.9 px in y on step 1, then
        // ≤0.7 px for every step after. The claim under test is that the track
        // *follows* the drag, so the loose bound is spent on the transient and
        // the rest is held tight.
        let (x_tol, y_tol) = if k == 1 { (10.0, 5.0) } else { (2.0, 2.0) };
        assert!(
            (p.position.x - want_x).abs() < x_tol,
            "step {k}: x = {} want {want_x}, confidence = {}",
            p.position.x,
            p.confidence
        );
        assert!((p.position.y - want_y).abs() < y_tol, "step {k}: y = {}", p.position.y);
        assert!(p.confidence > 0.5, "step {k}: confidence = {}", p.confidence);
    }
}

#[test]
fn animation_coasts_at_zero_confidence_and_reacquires() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0, gradient: false };

    let _ = step(&mut est, &mut io, &mut clock).expect("first fix");

    // The target's content starts animating in place: L1 cannot verify the
    // registered appearance, L8 is gated DYNAMIC → the track coasts at
    // confidence 0.
    for k in 1..=3 {
        io.salt = k;
        let p = step(&mut est, &mut io, &mut clock).expect("coasting");
        assert_eq!(p.confidence, 0.0, "step {k} must be a labeled belief, not a measurement");
        // Static in place: the coast must stay near the truth.
        assert!((p.position.x - 200.0).abs() < 4.0, "coast x = {}", p.position.x);
    }

    // The animation settles and the target has been moved meanwhile: the
    // re-acquisition must reset onto the measured position, not smear the
    // stale velocity across the jump.
    io.salt = 0;
    io.top_left_logical = Some((290.0, 170.0));
    let p = step(&mut est, &mut io, &mut clock).expect("re-acquired");
    assert!((p.position.x - 320.0).abs() < 3.0, "re-acquire x = {}", p.position.x);
    assert!((p.position.y - 190.0).abs() < 3.0, "re-acquire y = {}", p.position.y);
    assert!(p.confidence > 0.4);
}

#[test]
fn disappearing_target_coasts_then_recovers() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0, gradient: false };

    let _ = step(&mut est, &mut io, &mut clock).expect("first fix");

    // The target leaves the screen entirely.
    io.top_left_logical = None;
    for k in 1..=2 {
        let p = step(&mut est, &mut io, &mut clock).expect("coast while gone");
        assert_eq!(p.confidence, 0.0);
        assert!(p.bbox_physical.is_none(), "step {k}: no fabricated bbox");
    }

    // It comes back at a nearby position.
    io.top_left_logical = Some((180.0, 140.0));
    let p = step(&mut est, &mut io, &mut clock).expect("re-acquired");
    assert!((p.position.x - 210.0).abs() < 3.0);
    assert!(p.confidence > 0.4);
}

#[test]
fn first_search_without_target_is_target_lost() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: None, salt: 0, gradient: false };
    assert!(matches!(
        step(&mut est, &mut io, &mut clock),
        Err(EstimateError::TargetLost)
    ));
}

/// One estimate with the clock advanced by `ms`, so a settle can be observed
/// at a real host's cadence instead of the fixture's 600 ms default.
fn step_at(
    est: &mut FusedEstimator,
    io: &mut dyn CaptureIo,
    clock: &mut Clock,
    ms: u64,
) -> Result<fidus_core::estimate::ProbabilisticPosition, EstimateError> {
    clock.t += Duration::from_millis(ms);
    let t = clock.t;
    est.set_clock(Box::new(move || t));
    est.estimate(io, &frame())
}

/// Re-registering is the flush a host asks for: "the target just moved and I
/// need one clean reading now" cannot be answered by waiting out the settle,
/// and there is no reset entry point besides this one.
///
/// Run in MeaPet's regime — an appearance discounted to the ceiling floor, so
/// every reading is a weak one. The drifted leg also demonstrates the sharper
/// half of why the ceiling exists: on a *fully* self-similar appearance the
/// match is located by the search window, so a belief that has already walked
/// off gets corroborated instead of corrected, and the readings keep leaving a
/// target that is standing still. A real sprite at ceiling 0.107 does
/// self-correct (MeaPet's control arm converges to 0.01 px); that difference
/// belongs to the appearance, not to the filter, and this test is only about
/// the flush.
#[test]
fn re_registering_the_target_flushes_the_settle_transient() {
    let mut clock = Clock::new();
    let shared = clock.t;
    let mut est = FusedEstimator::with_clock(Box::new(move || shared));
    est.register_target(
        TargetDescription::new(gradient_template())
            .with_initial_center(LogicalPoint::new(200.0, 150.0))
            .tracking_ambiguous_appearance(),
    )
    .expect("explicit ambiguity opt-in");
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0, gradient: true };
    step_at(&mut est, &mut io, &mut clock, 300).expect("first fix");

    // The target moves once and then stands still at logical x = 308.
    io.top_left_logical = Some((278.0, 130.0));
    let mut last = 308.0;
    for _ in 0..4 {
        last = step_at(&mut est, &mut io, &mut clock, 1270)
            .expect("a weak measurement is still a measurement")
            .position
            .x;
    }
    assert!(
        (last - 308.0).abs() > 20.0,
        "the belief was supposed to be off-target before the flush, got {last}"
    );

    est.register_target(
        TargetDescription::new(gradient_template())
            .with_initial_center(LogicalPoint::new(308.0, 150.0))
            .tracking_ambiguous_appearance(),
    )
    .expect("re-registration");
    let p = step_at(&mut est, &mut io, &mut clock, 1270).expect("first fix after flush");
    assert!(
        (p.position.x - 308.0).abs() < 1.0,
        "re-registration must snap onto the measurement, not continue the \
         transient: x = {}, was {last}",
        p.position.x
    );
}


/// Reads until the reported position lands within 3 px of `want_logical`,
/// or `None` if that does not happen inside `limit` reads.
fn reads_until(
    est: &mut FusedEstimator,
    io: &mut dyn CaptureIo,
    clock: &mut Clock,
    want_logical: (f64, f64),
    limit: usize,
) -> Option<usize> {
    for k in 1..=limit {
        let Ok(p) = step(est, io, clock) else { return None };
        if (p.position.x - want_logical.0).abs() < 3.0 && (p.position.y - want_logical.1).abs() < 3.0
        {
            return Some(k);
        }
    }
    None
}

/// A hit anywhere clears `lost_streak` (`FusedEstimator::absorb`), and
/// `lost_streak` is the only thing that widens the search window — the window
/// that both L1 and L8's blob extraction are gated by. So a screen that keeps
/// returning one exact copy never widens, and widening is the only way a
/// far-away truth gets found again: the false hit does not merely mislead, it
/// removes the recovery route.
#[test]
fn a_confident_false_hit_pins_the_window_that_would_have_rescued_it() {
    let near_top = (170.0, 130.0); // center logical (200, 150)
    let far_top = (420.0, 320.0); // center logical (450, 340)
    // 628 physical px apart, against a window half-extent of
    // 1.5·120 + 48 = 228 px at streak 0, growing 64 px per both-layers-miss.
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = TwoInstances { near: Some(near_top), far: None };
    let _ = step(&mut est, &mut io, &mut clock).expect("first fix on the near copy");
    let cap = est.confidence_ceiling().expect("a registered target advertises a ceiling");

    // The truth moves away; a copy stays where the track believes it is.
    io.far = Some(far_top);
    for k in 1..=12 {
        let p = step(&mut est, &mut io, &mut clock).expect("a hit is returned, never raised");
        assert_eq!(
            p.confidence,
            cap,
            "read {k}: an exact copy scores full marks, so confidence sits bit-equal at \
             the ceiling — the signature a host reported from a real screen"
        );
        assert!(
            (p.position.x - 200.0).abs() < 2.0 && (p.position.y - 150.0).abs() < 2.0,
            "read {k}: the near copy must hold the track, got {:?}",
            p.position
        );
    }

    // Only now is the decoy removed. Had those twelve confident reads been
    // misses, the window would already be 228 + 12·64 = 996 px wide and the far
    // copy reachable on the next read. It is not: recovery lands on the read
    // predicted from a streak of *zero*. The far copy's top-left sits 440 px
    // from the belief, so it enters the window once 228 + 64·n ≥ 440, i.e.
    // n = 4 — the fifth read. Recompute both figures if `search_margin_px` or
    // `widen_per_miss_px` ever move.
    io.near = None;
    let acquired_on = reads_until(&mut est, &mut io, &mut clock, (450.0, 340.0), 10)
        .expect("the widening window must eventually reach the far copy");
    assert_eq!(
        acquired_on, 5,
        "twelve reads at full ceiling must have bought nothing toward recovery: acquiring \
         on read {acquired_on} means those hits did not reset the streak"
    );
}

/// A capture of `BW × BH` with one `BS²` patch of the same hash texture the
/// other fixtures use, at a fixed physical top-left.
struct BigOutput {
    top_left: (i64, i64),
}

const BW: u32 = 1920;
const BH: u32 = 1080;
const BS: u32 = 96;

impl CaptureIo for BigOutput {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        let format = PixelFormat::Argb8888;
        let mut data = vec![90u8; (BW * BH * 4) as usize];
        let (tx, ty) = self.top_left;
        for yy in 0..BS {
            for xx in 0..BS {
                let (px, py) = (tx + xx as i64, ty + yy as i64);
                if px < 0 || py < 0 || px >= BW as i64 || py >= BH as i64 {
                    continue;
                }
                let i = (py as u32 * BW + px as u32) as usize * 4;
                format.write_rgba(&mut data, i, pattern(xx, yy, 0));
            }
        }
        Ok(Frame { width: BW, height: BH, stride: BW * 4, format, data })
    }
}

/// The same desktop at 1× — the scale where "logical" and "capture pixel"
/// coincide, so a belief can be written down without a second conversion.
fn big_frame() -> CoordinateFrame {
    let corr: Vec<_> = [(0.0, 0.0), (640.0, 0.0), (0.0, 420.0), (640.0, 420.0)]
        .into_iter()
        .map(|(x, y)| (LogicalPoint::new(x, y), PhysicalPoint::new(x, y)))
        .collect();
    let quality = CalibrationQuality {
        rms_residual_px: 0.0,
        max_residual_px: 0.0,
        verification_max_err_px: 0.0,
        consistency_max_err_px: 0.0,
        sample_count: 4,
        independent_passes: 2,
    };
    CoordinateFrame::new(
        AffineTransform::from_correspondences(&corr).expect("well-conditioned"),
        (BW, BH),
        CalibrationMethod::Crosshair,
        quality,
        SystemTime::now(),
    )
    .expect("invertible")
}

fn big_template() -> RgbaImage {
    let mut data = Vec::with_capacity((BS * BS * 4) as usize);
    for y in 0..BS {
        for x in 0..BS {
            data.extend_from_slice(&pattern(x, y, 0));
        }
    }
    RgbaImage::from_raw(BS, BS, data)
}

/// The crossing MeaPet measured on a real desktop, reproduced on pure CPU: a
/// cold start with no belief asks for the whole output, and the position budget
/// prices that request against the *output* — so it is refused on a 1920-wide
/// screen whatever the target looks like. The same screen, the same render and
/// a belief at the truth need a template-derived window instead, which fits.
#[test]
fn a_belief_replaces_the_whole_output_scan_the_budget_refuses() {
    let (tx, ty) = (1400_i64, 700_i64);
    let half = f64::from(BS) / 2.0;
    let belief = LogicalPoint::new(tx as f64 + half, ty as f64 + half);
    let mut io = BigOutput { top_left: (tx, ty) };

    let mut blind = FusedEstimator::new();
    blind
        .register_target(TargetDescription::new(big_template()))
        .expect("valid target");
    assert!(
        matches!(blind.estimate(&mut io, &big_frame()), Err(EstimateError::TargetLost)),
        "without a belief the request is the whole output and must be refused"
    );

    let mut told = FusedEstimator::new();
    told.register_target(TargetDescription::new(big_template()).with_initial_center(belief))
        .expect("valid target");
    let p = told.estimate(&mut io, &big_frame()).expect("a belief must be searched");
    assert!(
        (p.position.x - belief.x).abs() < 2.0 && (p.position.y - belief.y).abs() < 2.0,
        "the belief narrowed the window but did not find the render: {:?}",
        p.position
    );
}

/// A belief that is wrong is a cost, not a trap: while there is no fix, every
/// miss widens the window by the same 64 px the steady state uses, so the truth
/// is reached once `228 + 64·n` covers the 440 px from the belief to it — read 5.
#[test]
fn a_wrong_belief_costs_reads_and_then_finds_the_target() {
    let mut clock = Clock::new();
    // `estimator` places the belief at logical (200, 150); the target sits at
    // center (450, 340), out of every window until the widening reaches it.
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((420.0, 320.0)), salt: 0, gradient: false };

    let mut acquired_on = None;
    for k in 1..=8 {
        if let Ok(p) = step(&mut est, &mut io, &mut clock)
            && (p.position.x - 450.0).abs() < 3.0
            && (p.position.y - 340.0).abs() < 3.0
        {
            acquired_on = Some(k);
            break;
        }
    }
    let on = acquired_on.expect("a wrong belief must not make the target unfindable");
    assert_eq!(
        on, 5,
        "read {on} rather than the 5 the widening arithmetic predicts — the cold-start \
         window no longer widens per miss"
    );
}
