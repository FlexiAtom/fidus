// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! End-to-end simulation of the L9 pipeline against a fake compositor.
//!
//! `FakeIo` models what the real backend + compositor do — with the quirks
//! that showed up in live Niri runs baked in:
//!
//! * the marker occupies `round(pos·scale)` … `round((pos+size)·scale)`
//!   physical pixels (compositor placement semantics, including rounding
//!   asymmetry);
//! * a panel strip offsets the usable area inside the capture;
//! * dynamic wallpaper produces small moving diffs (rejected by the
//!   detector's area gate);
//! * a cursor can punch a hole through the marker (bbox center stays exact).
//!
//! These tests are the regression guard for everything that was chased down
//! on the live compositor: the bbox-center correspondence, margin
//! quantization, and pass independence.

use fidus_calibrate::crosshair::{CrosshairCalibrator, CrosshairConfig};
use fidus_calibrate::detect::{detect_single_change, DetectConfig, DetectError};
use fidus_calibrate::rng::Rng;
use fidus_core::calibration::CalibrationError;
use fidus_core::coord::LogicalPoint;
use fidus_core::engine::Calibrator;
use fidus_core::io::{
    CalibrationIo, CaptureError, Frame, MarkerError, MarkerStyle, PixelFormat,
};

// ---------------------------------------------------------------------------
// Synthetic screen
// ---------------------------------------------------------------------------

/// Geometry of the fake output: physical size, output scale, and a top panel
/// strip reserving space at the top of the capture.
#[derive(Clone, Copy)]
struct Screen {
    phys_w: u32,
    phys_h: u32,
    panel_h: f64,
    scale: f64,
}

impl Screen {
    /// The usable area the compositor would announce, in logical pixels.
    fn usable(&self) -> (f64, f64) {
        (
            self.phys_w as f64 / self.scale,
            (self.phys_h as f64 - self.panel_h) / self.scale,
        )
    }

    fn stride(&self) -> u32 {
        // Deliberately padded: exercises stride-aware pixel access everywhere.
        self.phys_w * 4 + 24
    }
}

/// Physical rectangle `[x0, x1) × [y0, y1)`.
type Rect = (i64, i64, i64, i64);

/// Renders one capture: gradient background + panel + wallpaper patches +
/// optional marker + optional holes punched into the marker.
fn render(
    screen: &Screen,
    marker: Option<(LogicalPoint, MarkerStyle)>,
    patches: &[Rect],
    holes: &[Rect],
) -> Frame {
    render_tinted(screen, marker, patches, holes, None)
}

/// `patch_rgba` forces every patch to one colour, which is how
/// `Noise::ForeignRedrawSameColor` defeats the colour gate on purpose.
fn render_tinted(
    screen: &Screen,
    marker: Option<(LogicalPoint, MarkerStyle)>,
    patches: &[Rect],
    holes: &[Rect],
    patch_rgba: Option<[u8; 4]>,
) -> Frame {
    let (w, h) = (screen.phys_w, screen.phys_h);
    let stride = screen.stride() as usize;
    let mut data = vec![0u8; stride * h as usize];
    let panel_rows = screen.panel_h as i64;
    let fmt = PixelFormat::Argb8888;

    let bg = |x: i64, y: i64| -> [u8; 4] {
        [
            (x.wrapping_mul(3) % 251) as u8,
            (y.wrapping_mul(5) % 247) as u8,
            (((x / 9) ^ (y / 7)) % 253) as u8,
            255,
        ]
    };
    let put = |data: &mut [u8], x: i64, y: i64, rgba: [u8; 4]| {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return;
        }
        let i = y as usize * stride + x as usize * 4;
        fmt.write_rgba(data, i, rgba);
    };

    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let c = if y < panel_rows { [20, 20, 24, 255] } else { bg(x, y) };
            put(&mut data, x, y, c);
        }
    }
    for &(px, py, pw, ph) in patches {
        let c = patch_rgba.unwrap_or([
            (px % 256) as u8,
            (py % 256) as u8,
            ((px ^ py) % 256) as u8,
            255,
        ]);
        for y in py..py + ph {
            for x in px..px + pw {
                put(&mut data, x, y, c);
            }
        }
    }
    if let Some((pos, style)) = marker {
        let s = screen.scale;
        let size = style.size_logical;
        let x0 = (pos.x * s).round() as i64;
        let x1 = ((pos.x + size) * s).round() as i64;
        let y0 = panel_rows + (pos.y * s).round() as i64;
        let y1 = panel_rows + ((pos.y + size) * s).round() as i64;
        for y in y0..y1 {
            for x in x0..x1 {
                put(&mut data, x, y, style.rgba);
            }
        }
        for &(hx, hy, hw, hh) in holes {
            for y in hy..hy + hh {
                for x in hx..hx + hw {
                    put(&mut data, x, y, bg(x, y));
                }
            }
        }
    }

    Frame { width: w, height: h, stride: stride as u32, format: fmt, data }
}

/// The marker's physical rectangle for a logical top-left, as the compositor
/// would place it.
fn marker_rect(screen: &Screen, pos: LogicalPoint, style: &MarkerStyle) -> Rect {
    let s = screen.scale;
    let size = style.size_logical;
    let panel = screen.panel_h as i64;
    (
        (pos.x * s).round() as i64,
        panel + (pos.y * s).round() as i64,
        ((pos.x + size) * s).round() as i64,
        panel + ((pos.y + size) * s).round() as i64,
    )
}

// ---------------------------------------------------------------------------
// Fake CalibrationIo
// ---------------------------------------------------------------------------

enum Noise {
    Clean,
    /// Two small wallpaper patches whose positions drift every few captures.
    Wallpaper { seed: u64 },
    /// Another application repainting itself, which is the normal state of a
    /// desktop somebody is actually using (AGENTS §11).
    ///
    /// Measured on live Niri with nothing projected: 29 consecutive capture
    /// pairs contained 28 identical ones and a single pair differing by
    /// 144386 pixels, its bounding box starting at a terminal window's
    /// corner. A calibration takes ~28 frames, so the chance of at least one
    /// collision is 1-(1-1/29)^28 = 63%, which matched the observed failure
    /// rate of 4/12 to 7/15.
    ///
    /// `period` controls how often the foreign window repaints: every
    /// `period`-th capture renders it in a different colour, so a
    /// baseline/post pair that straddles the change sees a large region
    /// appear out of nowhere. Unlike `Wallpaper`, the patch is *large*, so it
    /// survives the area gate and fragments into many plausible components.
    ForeignRedraw { period: u64 },
    /// The adversarial case for the colour gate: a foreign window repainting
    /// in *the marker's own colour*.
    ///
    /// Why this exists: mutation-testing `ForeignRedraw` showed the colour
    /// gate alone carried it, so `corroborations` was an untested knob
    /// (AGENTS §7). Colour cannot separate these blocks from the marker;
    /// only looking twice can, because the marker stays put and the repaint
    /// moves.
    ForeignRedrawSameColor { period: u64 },
}

struct FakeIo {
    screen: Screen,
    noise: Noise,
    style: MarkerStyle,
    marker: Option<LogicalPoint>,
    /// When set, `show_marker` silently does nothing (marker never appears).
    blind: bool,
    step: u64,
    destroyed: bool,
    show_count: u32,
}

impl FakeIo {
    fn new(screen: Screen, noise: Noise) -> Self {
        FakeIo {
            screen,
            noise,
            style: MarkerStyle::DEFAULT,
            marker: None,
            blind: false,
            step: 0,
            destroyed: false,
            show_count: 0,
        }
    }

    fn patches_for_step(&self) -> Vec<Rect> {
        match self.noise {
            Noise::Clean => Vec::new(),
            Noise::ForeignRedrawSameColor { period } => {
                let phase = self.step / period;
                if phase % 2 == 0 {
                    return Vec::new();
                }
                let mut rng = Rng::seed_from(0x5A5A_1234 ^ phase);
                let marker_px = (self.style.size_logical * self.screen.scale) as i64;
                (0..8)
                    .map(|_| {
                        let x = rng.range(40.0, (self.screen.phys_w - 120) as f64) as i64;
                        let y = rng.range(
                            self.screen.panel_h + 40.0,
                            (self.screen.phys_h - 120) as f64,
                        ) as i64;
                        (x, y, marker_px, marker_px)
                    })
                    .collect()
            }
            Noise::ForeignRedraw { period } => {
                // A window repainting its *contents* — think a terminal
                // redrawing text. What matters for the detector is not the
                // total changed area but its shape: the live failures
                // reported "68 plausible regions", meaning the change broke
                // into many solid, marker-sized blocks that each cleared the
                // fill-ratio and area gates.
                //
                // An earlier version of this model displaced one large
                // rectangle instead. That produces a 6 px-wide L-shaped rim
                // whose fill ratio is 4.9% against a 0.45 gate, so the
                // detector discarded it and the test passed while the real
                // bug went unreproduced.
                let phase = self.step / period;
                if phase % 2 == 0 {
                    return Vec::new();
                }
                let mut rng = Rng::seed_from(0xF0E1_D2C3 ^ phase);
                let marker_px = (self.style.size_logical * self.screen.scale) as i64;
                (0..24)
                    .map(|_| {
                        let x = rng.range(40.0, (self.screen.phys_w - 120) as f64) as i64;
                        let y = rng.range(
                            self.screen.panel_h + 40.0,
                            (self.screen.phys_h - 120) as f64,
                        ) as i64;
                        (x, y, marker_px, marker_px)
                    })
                    .collect()
            }
            Noise::Wallpaper { seed } => {
                // Positions change only every 3rd capture: consecutive
                // baseline/post pairs usually share patches, and when they
                // do not, the resulting blobs (~16×16 px) fall below the
                // detector's area gate for a 28-logical marker.
                let window = self.step / 3;
                let mut rng = Rng::seed_from(seed ^ window);
                (0..2)
                    .map(|_| {
                        let x = rng.range(50.0, (self.screen.phys_w - 80) as f64) as i64;
                        let y = rng.range(
                            self.screen.panel_h + 50.0,
                            (self.screen.phys_h - 80) as f64,
                        ) as i64;
                        (x, y, 16, 16)
                    })
                    .collect()
            }
        }
    }
}

impl fidus_core::io::CaptureIo for FakeIo {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.step += 1;
        let marker = if self.blind { None } else { self.marker.map(|p| (p, self.style)) };
        // The same-colour adversary paints in whatever colour the calibrator
        // last asked for, so the colour gate cannot separate it from a marker.
        let tint = match self.noise {
            Noise::ForeignRedrawSameColor { .. } => Some(self.style.rgba),
            _ => None,
        };
        Ok(render_tinted(&self.screen, marker, &self.patches_for_step(), &[], tint))
    }
}

impl CalibrationIo for FakeIo {
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        Ok(self.screen.usable())
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        if self.destroyed {
            return Err(MarkerError::AlreadyDestroyed);
        }
        self.show_count += 1;
        if !self.blind {
            self.marker = Some(pos);
            self.style = style;
        }
        Ok(())
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        if self.destroyed {
            return Err(MarkerError::AlreadyDestroyed);
        }
        self.marker = None;
        Ok(())
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        self.destroyed = true;
        Ok(())
    }
}

fn calibrator(seed: u64) -> CrosshairCalibrator {
    CrosshairCalibrator::new(CrosshairConfig { seed: Some(seed), ..CrosshairConfig::default() })
}

// ---------------------------------------------------------------------------
// End-to-end tests
// ---------------------------------------------------------------------------

#[test]
fn recovers_known_mapping_end_to_end() {
    // physical = 1.25 · logical + (0, 40): fractional scale + panel offset.
    let mut io = FakeIo::new(
        Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 },
        Noise::Clean,
    );
    let frame = calibrator(7).calibrate(&mut io).expect("calibration succeeds");

    let m = frame.map();
    let [a, _b, c, _d, e, f] = m.coefficients();
    assert!((a - 1.25).abs() < 0.02, "a = {a}");
    assert!(c.abs() < 0.6, "c = {c}");
    assert!((e - 1.25).abs() < 0.02, "e = {e}");
    assert!((f - 40.0).abs() < 0.8, "f = {f}");
    assert!((m.linear_scale() - 1.25).abs() < 0.02);

    // Frame roundtrip through the calibrated map, away from the sample area.
    let p = frame.logical_to_physical(LogicalPoint::new(512.0, 304.0));
    assert!((p.x - 640.0).abs() < 1.0, "x = {}", p.x);
    assert!((p.y - 420.0).abs() < 1.0, "y = {}", p.y);
    let back = frame.physical_to_logical(p);
    assert!((back.x - 512.0).abs() < 1.0 && (back.y - 304.0).abs() < 1.0);

    // Both passes ran, full protocol including teardown.
    let q = frame.quality();
    assert_eq!(q.independent_passes, 2);
    assert_eq!(q.sample_count, 4);
    // Bounds are deliberately tighter than the calibrator's own rejection
    // gates (1.5 px each). Asserting at the gate value only restates what the
    // calibrator already enforces — it would hold equally at 0.001 and 1.499,
    // and so cannot notice quality decaying toward the threshold. That is how
    // the fractional-scale degradation stayed invisible here despite this
    // test already running at scale 1.25; see
    // `integer_scales_are_exact_and_fractional_ones_stay_bounded`.
    assert!(q.verification_max_err_px < 0.9, "verify = {}", q.verification_max_err_px);
    assert!(q.consistency_max_err_px < 1.2, "consistency = {}", q.consistency_max_err_px);
    assert!(io.destroyed, "projector torn down after success");
}

#[test]
fn wallpaper_noise_still_converges() {
    let mut io = FakeIo::new(
        Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 },
        Noise::Wallpaper { seed: 0x0BEA },
    );
    let frame = calibrator(11).calibrate(&mut io).expect("calibration survives wallpaper");
    let [a, _b, _c, _d, _e, f] = frame.map().coefficients();
    assert!((a - 1.25).abs() < 0.03, "a = {a}");
    assert!((f - 40.0).abs() < 1.0, "f = {f}");
    assert!(io.destroyed);
}

#[test]
fn zero_passes_and_insufficient_samples_fail_without_display() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.0 };
    let mut zero_passes = FakeIo::new(screen, Noise::Clean);
    let cfg = CrosshairConfig { passes: 0, ..CrosshairConfig::default() };
    let err = CrosshairCalibrator::new(cfg).calibrate(&mut zero_passes).expect_err("zero passes");
    assert!(matches!(err, CalibrationError::InvalidConfiguration(_)), "{err}");
    assert!(zero_passes.destroyed);

    let mut too_few = FakeIo::new(screen, Noise::Clean);
    let cfg = CrosshairConfig { primary_positions: 2, ..CrosshairConfig::default() };
    let err = CrosshairCalibrator::new(cfg).calibrate(&mut too_few).expect_err("too few primary samples");
    assert!(matches!(err, CalibrationError::AccuracyBelowThreshold { .. }), "{err}");
    assert!(too_few.destroyed);
}

#[test]
fn usable_area_too_small_fails_and_tears_down() {
    let mut io = FakeIo::new(
        Screen { phys_w: 200, phys_h: 150, panel_h: 10.0, scale: 1.0 },
        Noise::Clean,
    );
    let err = calibrator(1).calibrate(&mut io).expect_err("tiny screen must fail");
    assert!(matches!(err, CalibrationError::UsableAreaTooSmall { .. }), "{err}");
    assert!(io.destroyed, "teardown runs on the early-exit path too");
}

#[test]
fn blind_projection_fails_with_detection_error_and_tears_down() {
    let mut io = FakeIo::new(
        Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 },
        Noise::Clean,
    );
    io.blind = true;
    let err = calibrator(3).calibrate(&mut io).expect_err("blind projector must fail");
    assert!(
        matches!(err, CalibrationError::DetectionFailed { attempts: 3, .. }),
        "{err}"
    );
    assert!(io.destroyed);
}

// ---------------------------------------------------------------------------
// Detector-level tests (the same quirks, in isolation)
// ---------------------------------------------------------------------------

#[test]
fn detector_rejects_small_background_blips() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 };
    let style = MarkerStyle::DEFAULT;
    let pos = LogicalPoint::new(300.0, 200.0);
    let baseline = render(&screen, None, &[(700, 500, 12, 12)], &[]);
    let post = render(&screen, Some((pos, style)), &[(100, 600, 12, 12)], &[]);
    let prior = {
        let (x0, y0, x1, y1) = marker_rect(&screen, pos, &style);
        ((x1 - x0) * (y1 - y0)) as f64
    };

    let det = detect_single_change(&baseline, &post, Some(prior), &DetectConfig::default())
        .expect("marker found despite blips");
    let (x0, y0, x1, y1) = marker_rect(&screen, pos, &style);
    assert_eq!((det.bbox.x0, det.bbox.y0, det.bbox.x1, det.bbox.y1), (x0, y0, x1, y1));
}

#[test]
fn cursor_hole_through_marker_keeps_bbox_center_exact() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 };
    let style = MarkerStyle::DEFAULT;
    let pos = LogicalPoint::new(400.0, 250.0);
    let baseline = render(&screen, None, &[], &[]);
    let (x0, y0, x1, y1) = marker_rect(&screen, pos, &style);
    // A 20×20 cursor sits dead-center in the projected marker.
    let hole = ((x0 + x1) / 2 - 10, (y0 + y1) / 2 - 10, 20, 20);
    let post = render(&screen, Some((pos, style)), &[], &[hole]);

    let prior = ((x1 - x0) * (y1 - y0)) as f64;
    let det = detect_single_change(&baseline, &post, Some(prior), &DetectConfig::default())
        .expect("perforated marker still detected");
    assert_eq!((det.bbox.x0, det.bbox.y0, det.bbox.x1, det.bbox.y1), (x0, y0, x1, y1));
    let c = det.bbox.center();
    let truth = fidus_core::coord::PhysicalPoint::new(
        (x0 + x1) as f64 / 2.0,
        (y0 + y1) as f64 / 2.0,
    );
    assert!(c.distance(truth) < 1e-9, "hole must not shift the box center");
}

#[test]
fn two_plausible_blobs_are_ambiguous() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 };
    let style = MarkerStyle::DEFAULT;
    let baseline = render(&screen, None, &[], &[]);
    let a = LogicalPoint::new(200.0, 200.0);
    let b = LogicalPoint::new(600.0, 300.0);
    // Two same-size magenta squares: frame A shows one, frame B shows both.
    let post = {
        let mut f = render(&screen, Some((b, style)), &[], &[]);
        // Overlay the second marker manually.
        let extra = render(&screen, Some((a, style)), &[], &[]);
        for i in 0..f.data.len() {
            if extra.data[i] != baseline.data[i] {
                f.data[i] = extra.data[i];
            }
        }
        f
    };
    let prior = {
        let (x0, y0, x1, y1) = marker_rect(&screen, a, &style);
        ((x1 - x0) * (y1 - y0)) as f64
    };
    let err = detect_single_change(&baseline, &post, Some(prior), &DetectConfig::default())
        .expect_err("two candidates must be ambiguous");
    assert!(matches!(err, DetectError::Ambiguous(2)), "{err}");
}

#[test]
fn no_prior_rejects_multiple_coherent_changes() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 };
    let style = MarkerStyle::DEFAULT;
    let pos = LogicalPoint::new(350.0, 220.0);
    // Two coherent changes without an area prior are ambiguous, even when one
    // is smaller: background repaint size is not a trustworthy marker signal.
    let baseline = render(&screen, None, &[(900, 400, 30, 30)], &[]);
    let post = render(&screen, Some((pos, style)), &[(500, 550, 30, 30)], &[]);

    let err = detect_single_change(&baseline, &post, None, &DetectConfig::default())
        .expect_err("multiple changes must fail closed without an area prior");
    assert!(matches!(err, DetectError::Ambiguous(count) if count >= 2), "{err}");
}


// ---------------------------------------------------------------------------
// Fractional-scale quantization (measured on live Niri, 2026-09).
// ---------------------------------------------------------------------------

/// Integer output scales must calibrate *exactly*; fractional ones must not
/// silently drift past the point where the consistency gate has no headroom
/// left.
///
/// **Why this test exists**: `recovers_known_mapping_end_to_end` already ran
/// at scale 1.25 and asserted `consistency <= 1.5` — the *rejection
/// threshold*. That assertion passes whether the true value is 0.001 or
/// 1.499, so it could never notice the degradation this test pins down
/// (AGENTS §5: an assertion that encodes the threshold instead of the
/// expected behaviour ossifies whatever the code happens to do).
///
/// **What it guards**: fidus commands marker positions in whole logical
/// pixels (`crosshair.rs`, to match the backend's rounding), so the
/// compositor lands them on `round(L·scale)` — up to 0.5 physical pixels off.
/// At integer scales `round(L·s) == L·s`, so the residual is necessarily
/// zero; at fractional scales it is systematic, not noise. Live Niri measured
/// rms 0.000 / 0.089 / 0.141 / 0.208 / 0.000 px at scale 1 / 1.25 / 1.5 /
/// 1.75 / 2, and this simulation reproduces it (0.000 / 0.160 / 0.147 /
/// 0.000).
///
/// **Failure mode**: if a future change makes integer scales inexact, the
/// projection path has acquired a new rounding error. If the fractional
/// bound is exceeded, the error source grew and the `consistency` gate —
/// already down to ~26% headroom live — will start rejecting good
/// calibrations.
///
/// See `docs/measurements/l9-fractional-scaling.md` and spec §4.1.
#[test]
fn integer_scales_are_exact_and_fractional_ones_stay_bounded() {
    for scale in [1.0, 2.0] {
        let mut io = FakeIo::new(
            Screen { phys_w: 1280, phys_h: 800, panel_h: 0.0, scale },
            Noise::Clean,
        );
        let q = *calibrator(7)
            .calibrate(&mut io)
            .unwrap_or_else(|e| panic!("scale {scale} must calibrate: {e}"))
            .quality();
        assert_eq!(
            (q.rms_residual_px, q.max_residual_px), (0.0, 0.0),
            "scale {scale}: integer scales land on exact pixels, so residuals \
             must be exactly zero; got rms={} max={}",
            q.rms_residual_px, q.max_residual_px,
        );
        assert_eq!((q.verification_max_err_px, q.consistency_max_err_px), (0.0, 0.0));
    }

    for scale in [1.25, 1.5] {
        let mut io = FakeIo::new(
            Screen { phys_w: 1280, phys_h: 800, panel_h: 0.0, scale },
            Noise::Clean,
        );
        let q = *calibrator(7)
            .calibrate(&mut io)
            .unwrap_or_else(|e| panic!("scale {scale} must still calibrate: {e}"))
            .quality();
        // Quantization is bounded by half a physical pixel per marker, so the
        // fitted residual cannot reach 0.5; anything beyond that is a new bug.
        assert!(
            q.rms_residual_px > 0.0 && q.rms_residual_px < 0.5,
            "scale {scale}: expected small systematic quantization residual, got {}",
            q.rms_residual_px,
        );
        // Live worst case was 1.114 against a 1.5 gate. Hold the simulation to
        // a tighter bound so regressions surface here, not on a user's laptop.
        assert!(
            q.consistency_max_err_px < 1.2,
            "scale {scale}: consistency {} is eating the gate's headroom",
            q.consistency_max_err_px,
        );
    }
}

// ---------------------------------------------------------------------------
// Foreign window repaints (measured on live Niri, 2026-09).
// ---------------------------------------------------------------------------

/// L9 must survive another application repainting itself mid-calibration.
///
/// **Why this test exists**: the difference detector's contract is "the
/// marker is the only thing that changed between these two frames". That
/// premise was never written down and is false on any desktop somebody is
/// using — measured live, 1 in 29 consecutive capture pairs differed by
/// 144386 pixels because a terminal repainted. Spread over the ~28 frames a
/// calibration needs, that is a 63% chance of at least one collision, and
/// the observed failure rate was 4/12 to 7/15 (AGENTS §11).
///
/// **What it guards**: that a large foreign repaint cannot be mistaken for
/// the marker. The failures were not subtle — the detector reported "68
/// plausible regions found" as the background fragmented, and one run
/// produced a 287 px residual, meaning a background fragment was accepted as
/// a correspondence point and fed to the least-squares solve.
///
/// **Failure mode if this regresses**: calibration either fails outright
/// (honest, but unusable) or, worse, solves against a fragment. The residual
/// gate catches the latter today, which is exactly why it must stay.
#[test]
fn foreign_window_repaints_do_not_break_calibration() {
    // period 4: the foreign window's colour flips often enough that several
    // baseline/post pairs straddle a repaint, but not so often that every
    // single pair does — mirroring a real desktop.
    let mut io = FakeIo::new(
        Screen { phys_w: 1366, phys_h: 768, panel_h: 0.0, scale: 1.0 },
        Noise::ForeignRedraw { period: 4 },
    );

    let frame = calibrator(11)
        .calibrate(&mut io)
        .expect("calibration must survive a foreign window repainting");

    let [a, b, c, d, e, f] = frame.map().coefficients();
    assert!((a - 1.0).abs() < 0.01, "a = {a}");
    assert!((e - 1.0).abs() < 0.01, "e = {e}");
    assert!(b.abs() < 0.01 && d.abs() < 0.01, "no skew expected: b={b} d={d}");
    assert!(c.abs() < 0.5 && f.abs() < 0.5, "no offset expected: c={c} f={f}");

    let q = frame.quality();
    // Exact up to float noise: the least-squares solve accumulates ~1e-13
    // even when every correspondence is pixel-perfect.
    assert!(q.rms_residual_px < 1e-9, "scale 1 must stay exact: {q:?}");
    assert!(io.destroyed, "projector torn down");
}


/// The colour gate alone is not enough: corroboration must carry the case
/// where the interference is the marker's own colour.
///
/// **Why this test exists**: mutation-testing
/// `foreign_window_repaints_do_not_break_calibration` showed it still passed
/// with `corroborations` turned down to 1, which made that knob untested
/// (AGENTS §7) — the colour gate was doing all the work. This test removes
/// colour as a discriminator so only "look again, the marker has not moved"
/// can succeed.
#[test]
fn same_colored_repaints_need_corroboration() {
    let mut io = FakeIo::new(
        Screen { phys_w: 1366, phys_h: 768, panel_h: 0.0, scale: 1.0 },
        Noise::ForeignRedrawSameColor { period: 4 },
    );

    let frame = calibrator(23)
        .calibrate(&mut io)
        .expect("corroboration must survive same-coloured interference");

    let q = frame.quality();
    assert!(q.rms_residual_px < 1e-9, "scale 1 must stay exact: {q:?}");
    let [a, _, c, _, e, f] = frame.map().coefficients();
    assert!((a - 1.0).abs() < 0.01 && (e - 1.0).abs() < 0.01, "a={a} e={e}");
    assert!(c.abs() < 0.5 && f.abs() < 0.5, "c={c} f={f}");
}
