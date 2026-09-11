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

/// Integer-frequency product gratings. Distinct salts are **exactly
/// orthogonal** on the (60×40) tile at any relative alignment — the DFT
/// orthogonality of integer-frequency sinusoids — so an "animation frame"
/// can never alias into the registered appearance (hash-based patterns
/// leak: cross-salt shifts leave enough self-similarity for a spurious
/// ~0.4 NCC peak, which the probe test caught).
fn pattern(x: u32, y: u32, salt: u32) -> [u8; 4] {
    let (fx, fy): (f64, f64) = match salt {
        0 => (1.0, 2.0),
        1 => (3.0, 1.0),
        2 => (2.0, 5.0),
        _ => (5.0, 3.0),
    };
    let t = (std::f64::consts::TAU * fx * x as f64 / TW as f64).sin()
        * (std::f64::consts::TAU * fy * y as f64 / TH as f64).sin();
    let luma = (128.0 + 100.0 * t).round().clamp(0.0, 255.0) as u8;
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

/// A fake output the test scripts: target at a logical top-left (or absent),
/// with an animatable content salt.
struct FakeOutput {
    top_left_logical: Option<(f64, f64)>,
    salt: u32,
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
                    format.write_rgba(&mut data, i, pattern(xx / 2, yy / 2, self.salt));
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
    io: &mut FakeOutput,
    clock: &mut Clock,
) -> Result<fidus_core::estimate::ProbabilisticPosition, EstimateError> {
    let t = clock.tick();
    est.set_clock(Box::new(move || t));
    est.estimate(io, &frame())
}

#[test]
fn acquires_from_belief_and_follows_a_drag() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0 };

    // First fix at the caller's belief (center at 200,150 logical).
    let p = step(&mut est, &mut io, &mut clock).expect("first fix");
    assert!((p.position.x - 200.0).abs() < 2.0, "x = {}", p.position.x);
    assert!((p.position.y - 150.0).abs() < 2.0);
    assert!(p.confidence > 0.5);

    // A drag: 20 logical px per 600 ms step, 8 steps.
    for k in 1..=8 {
        io.top_left_logical = Some((170.0 + 20.0 * k as f64, 130.0 + 8.0 * k as f64));
        let p = step(&mut est, &mut io, &mut clock).expect("tracked");
        let want_x = 200.0 + 20.0 * k as f64;
        let want_y = 150.0 + 8.0 * k as f64;
        assert!(
            (p.position.x - want_x).abs() < 3.0,
            "step {k}: x = {} want {want_x}",
            p.position.x
        );
        assert!((p.position.y - want_y).abs() < 3.0, "step {k}: y = {}", p.position.y);
        assert!(p.confidence > 0.5, "step {k}: confidence = {}", p.confidence);
    }
}

#[test]
fn animation_coasts_at_zero_confidence_and_reacquires() {
    let mut clock = Clock::new();
    let mut est = estimator(&clock);
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0 };

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
    let mut io = FakeOutput { top_left_logical: Some((170.0, 130.0)), salt: 0 };

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
    let mut io = FakeOutput { top_left_logical: None, salt: 0 };
    assert!(matches!(
        step(&mut est, &mut io, &mut clock),
        Err(EstimateError::TargetLost)
    ));
}
