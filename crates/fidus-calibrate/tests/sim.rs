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
        let c = [(px % 256) as u8, (py % 256) as u8, ((px ^ py) % 256) as u8, 255];
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

impl CalibrationIo for FakeIo {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.step += 1;
        let marker = if self.blind { None } else { self.marker.map(|p| (p, self.style)) };
        Ok(render(&self.screen, marker, &self.patches_for_step(), &[]))
    }

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
    assert!((m.a - 1.25).abs() < 0.02, "a = {}", m.a);
    assert!(m.c.abs() < 0.6, "c = {}", m.c);
    assert!((m.e - 1.25).abs() < 0.02, "e = {}", m.e);
    assert!((m.f - 40.0).abs() < 0.8, "f = {}", m.f);
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
    assert!(q.verification_max_err_px <= 1.5, "verify = {}", q.verification_max_err_px);
    assert!(q.consistency_max_err_px <= 1.5, "consistency = {}", q.consistency_max_err_px);
    assert!(io.destroyed, "projector torn down after success");
}

#[test]
fn wallpaper_noise_still_converges() {
    let mut io = FakeIo::new(
        Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 },
        Noise::Wallpaper { seed: 0x0BEA },
    );
    let frame = calibrator(11).calibrate(&mut io).expect("calibration survives wallpaper");
    let m = frame.map();
    assert!((m.a - 1.25).abs() < 0.03, "a = {}", m.a);
    assert!((m.f - 40.0).abs() < 1.0, "f = {}", m.f);
    assert!(io.destroyed);
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
fn no_prior_picks_the_largest_coherent_change() {
    let screen = Screen { phys_w: 1280, phys_h: 800, panel_h: 40.0, scale: 1.25 };
    let style = MarkerStyle::DEFAULT;
    let pos = LogicalPoint::new(350.0, 220.0);
    // 30×30 patch (900 px²) vs marker (~1225 px²): marker must win when no
    // area prior exists (first measurement of a pass).
    let baseline = render(&screen, None, &[(900, 400, 30, 30)], &[]);
    let post = render(&screen, Some((pos, style)), &[(500, 550, 30, 30)], &[]);

    let det = detect_single_change(&baseline, &post, None, &DetectConfig::default())
        .expect("largest change wins without prior");
    let (x0, y0, x1, y1) = marker_rect(&screen, pos, &style);
    assert_eq!((det.bbox.x0, det.bbox.y0, det.bbox.x1, det.bbox.y1), (x0, y0, x1, y1));
}
