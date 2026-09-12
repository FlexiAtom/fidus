// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! L0 Anchor end-to-end simulation: a fake X-like root with multi-marker
//! projection, plus the adversarial cases the protocol's four defenses
//! exist for — static sentinel-colored wallpaper patches (must be harmless),
//! animated sentinel-colored decoys (must be refused, never poison), a
//! scaled+offset capture plane, a blind projector, a tiny screen.

use fidus_calibrate::anchor::{AnchorCalibrator, AnchorConfig, SENTINEL_COLORS};
use fidus_core::calibration::CalibrationError;
use fidus_core::coord::LogicalPoint;
use fidus_core::engine::Calibrator;
use fidus_core::io::{
    CalibrationIo, CaptureError, CaptureIo, Frame, MarkerError, MarkerStyle, PixelFormat,
};

/// A fake root window: textured background, wallpaper-painted decoys, and
/// the projected markers rendered as exact-color squares (override-redirect
/// semantics: no blending).
///
/// `scale` / `offset` simulate a capture plane whose pixel grid differs
/// from the projection space (HiDPI, virtual screen origin): a marker at
/// logical `p` lands at `p * scale + offset` in the capture.
struct FakeRoot {
    size: (u32, u32),
    hint: (f64, f64),
    scale: f64,
    offset: (f64, f64),
    /// Static wallpaper decoys: (top-left, color) in capture pixels.
    static_decoys: Vec<(LogicalPoint, [u8; 4])>,
    /// Animated decoys: a sentinel-colored square that moves every capture.
    moving_decoys: Vec<[u8; 4]>,
    marks: Vec<(LogicalPoint, MarkerStyle)>,
    blind: bool,
    destroyed: bool,
    show_calls: u32,
    captures: u32,
}

impl FakeRoot {
    fn new(size: (u32, u32)) -> Self {
        FakeRoot {
            size,
            hint: (size.0 as f64, size.1 as f64),
            scale: 1.0,
            offset: (0.0, 0.0),
            static_decoys: Vec::new(),
            moving_decoys: Vec::new(),
            marks: Vec::new(),
            blind: false,
            destroyed: false,
            show_calls: 0,
            captures: 0,
        }
    }

    fn put_square(data: &mut [u8], w: u32, h: u32, x0: f64, y0: f64, side: f64, rgba: [u8; 4]) {
        let (x0, y0, x1, y1) =
            (x0.round() as i64, y0.round() as i64, (x0 + side).round() as i64, (y0 + side).round() as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
                    continue;
                }
                let i = (y as u32 * w + x as u32) as usize * 4;
                PixelFormat::Xrgb8888.write_rgba(data, i, rgba);
            }
        }
    }
}

impl CaptureIo for FakeRoot {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.captures += 1;
        let (w, h) = self.size;
        let mut data = vec![0u8; w as usize * h as usize * 4];
        for y in 0..h {
            for x in 0..w {
                let v = ((x.wrapping_mul(29) ^ y.wrapping_mul(17)) % 97) as u8;
                let i = (y * w + x) as usize * 4;
                PixelFormat::Xrgb8888.write_rgba(
                    &mut data,
                    i,
                    [40 + v % 40, 60 + v % 30, 90 + v % 25, 255],
                );
            }
        }
        for (pos, rgba) in &self.static_decoys {
            Self::put_square(&mut data, w, h, pos.x, pos.y, 8.0, *rgba);
        }
        for (k, rgba) in self.moving_decoys.iter().enumerate() {
            // Wanders across the middle band, a new spot every capture.
            let t = (self.captures as f64) * 37.0 + k as f64 * 101.0;
            let x = 100.0 + (t * 7.0) % (w as f64 - 200.0);
            let y = 100.0 + (t * 3.0) % (h as f64 - 200.0);
            Self::put_square(&mut data, w, h, x, y, 8.0, *rgba);
        }
        for (pos, style) in &self.marks {
            Self::put_square(
                &mut data,
                w,
                h,
                pos.x * self.scale + self.offset.0,
                pos.y * self.scale + self.offset.1,
                style.size_logical * self.scale,
                style.rgba,
            );
        }
        Ok(Frame { width: w, height: h, stride: w * 4, format: PixelFormat::Xrgb8888, data })
    }
}

impl CalibrationIo for FakeRoot {
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        Ok(self.hint)
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        self.show_markers(&[(pos, style)])
    }

    fn show_markers(&mut self, marks: &[(LogicalPoint, MarkerStyle)]) -> Result<(), MarkerError> {
        if self.destroyed {
            return Err(MarkerError::AlreadyDestroyed);
        }
        self.show_calls += 1;
        if !self.blind {
            self.marks = marks.to_vec();
        }
        Ok(())
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        self.marks.clear();
        Ok(())
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        self.marks.clear();
        self.destroyed = true;
        Ok(())
    }
}

fn config(seed: u64) -> AnchorConfig {
    AnchorConfig { seed: Some(seed), ..AnchorConfig::default() }
}

#[test]
fn recovers_identity_mapping() {
    let mut io = FakeRoot::new((1024, 768));
    let frame = AnchorCalibrator::new(config(7)).calibrate(&mut io).expect("calibrates");
    let [a, _b, c, _d, e, f] = frame.map().coefficients();
    assert!((a - 1.0).abs() < 0.01, "a = {a}");
    assert!(c.abs() < 0.5, "c = {c}");
    assert!((e - 1.0).abs() < 0.01, "e = {e}");
    assert!(f.abs() < 0.5, "f = {f}");
    let q = frame.quality();
    assert_eq!(q.independent_passes, 2);
    assert_eq!(q.sample_count, 4);
    assert!(q.verification_max_err_px <= 1.5);
    assert!(io.destroyed, "teardown after success");
    // Two passes × (one corner projection + one verification projection).
    assert!(io.show_calls >= 4, "show calls = {}", io.show_calls);
}

#[test]
fn recovers_scale_and_offset() {
    // HiDPI-like 1.5× capture plane with a displaced origin: the solved map
    // must surface exactly that, from fidus' own pixels only.
    let mut io = FakeRoot::new((1600, 1200));
    io.hint = (1024.0, 768.0);
    io.scale = 1.5;
    io.offset = (32.0, 16.0);
    let frame = AnchorCalibrator::new(config(11)).calibrate(&mut io).expect("calibrates");
    let [a, _b, c, _d, e, f] = frame.map().coefficients();
    assert!((a - 1.5).abs() < 0.01, "a = {a}");
    assert!((e - 1.5).abs() < 0.01, "e = {e}");
    assert!((c - 32.0).abs() < 1.0, "c = {c} (want 32)");
    assert!((f - 16.0).abs() < 1.0, "f = {f} (want 16)");
    assert!((frame.map().linear_scale() - 1.5).abs() < 0.01);
}

#[test]
fn static_sentinel_colored_wallpaper_is_harmless() {
    // The wallpaper contains patches of every sentinel color, right where
    // the corners go. Baseline differencing cancels them: they are present
    // in both captures. Calibration must succeed unperturbed.
    let mut io = FakeRoot::new((1024, 768));
    io.static_decoys = vec![
        (LogicalPoint::new(30.0, 30.0), SENTINEL_COLORS[2]),
        (LogicalPoint::new(970.0, 30.0), SENTINEL_COLORS[1]),
        (LogicalPoint::new(30.0, 720.0), SENTINEL_COLORS[0]),
        (LogicalPoint::new(500.0, 400.0), SENTINEL_COLORS[3]),
    ];
    let frame = AnchorCalibrator::new(config(5)).calibrate(&mut io).expect("static decoys cancel out");
    let [a, _b, c, _d, _e, f] = frame.map().coefficients();
    assert!((a - 1.0).abs() < 0.01 && c.abs() < 0.5 && f.abs() < 0.5, "{:?}", frame.map());
    assert!(io.destroyed);
}

#[test]
fn animated_sentinel_colored_decoys_are_refused_not_absorbed() {
    // An animation paints sentinel-colored squares that move between the
    // baseline and the post capture — the one thing differencing cannot
    // cancel. Every color has a moving twin, so each detection sees two
    // plausible blobs (ambiguous) or, if the decoy is off-grid, a broken
    // rectangle. The calibrator must refuse; it must never emit a frame.
    let mut io = FakeRoot::new((1024, 768));
    io.moving_decoys = SENTINEL_COLORS.to_vec();
    let err = AnchorCalibrator::new(config(5)).calibrate(&mut io).expect_err("must refuse");
    assert!(
        matches!(
            err,
            CalibrationError::Inconsistent { .. }
                | CalibrationError::DetectionFailed { .. }
                | CalibrationError::AccuracyBelowThreshold { .. }
        ),
        "unexpected error: {err}"
    );
    assert!(io.destroyed, "teardown on the failure path too");
}

#[test]
fn blind_projection_fails_with_detection_error() {
    let mut io = FakeRoot::new((1024, 768));
    io.blind = true;
    let err = AnchorCalibrator::new(config(3)).calibrate(&mut io).expect_err("must fail");
    assert!(matches!(err, CalibrationError::DetectionFailed { attempts: 3, .. }), "{err}");
    assert!(io.destroyed);
}

#[test]
fn tiny_screen_is_rejected_upfront() {
    let mut io = FakeRoot::new((60, 50));
    let err = AnchorCalibrator::new(config(1)).calibrate(&mut io).expect_err("must fail");
    assert!(matches!(err, CalibrationError::UsableAreaTooSmall { .. }), "{err}");
    assert!(io.destroyed);
}

#[test]
fn single_surface_backend_is_refused_cleanly() {
    // A backend that keeps the default `show_markers` (layer-shell style)
    // cannot host L0: the error must be the primitive error, with teardown.
    struct SingleSurface(FakeRoot);
    impl CaptureIo for SingleSurface {
        fn capture(&mut self) -> Result<Frame, CaptureError> {
            self.0.capture()
        }
    }
    impl CalibrationIo for SingleSurface {
        fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
            self.0.usable_size_hint()
        }
        fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
            self.0.show_marker(pos, style)
        }
        fn clear_marker(&mut self) -> Result<(), MarkerError> {
            self.0.clear_marker()
        }
        fn destroy_projector(&mut self) -> Result<(), MarkerError> {
            self.0.destroy_projector()
        }
    }
    let mut io = SingleSurface(FakeRoot::new((1024, 768)));
    let err = AnchorCalibrator::new(config(1)).calibrate(&mut io).expect_err("must fail");
    assert!(
        matches!(err, CalibrationError::Marker(MarkerError::MultiMarkersUnsupported)),
        "{err}"
    );
    assert!(io.0.destroyed);
}
