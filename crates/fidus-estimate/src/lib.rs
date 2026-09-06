//! C-layer estimators for fidus.
//!
//! # Status (spec §8 priorities)
//!
//! * **L1/L6 Fingerprint** — *wired* (P2-a): [`FingerprintEstimator`] tracks
//!   a registered [`TargetDescription`] by NCC template matching against
//!   fidus' own captures.
//! * **L8 EdgeSync** — *measurement primitive wired* (P2-b):
//!   [`edge_sync::EdgeSync`] measures displaced target bboxes by gated
//!   frame differencing; the fused estimator (L1 + L8 + L4 + EKF) that
//!   consumes it lands with P2-c.
//! * **L4 relative displacement** and **L7 EKF fusion** — P2-c.
//!
//! # Target registration (spec §3.2 / §6.1)
//!
//! The caller describes *what* to locate by registering a
//! [`TargetDescription`] — its own offscreen render, a pure visual input.
//! Nothing here reads or wraps a platform coordinate.

use fidus_core::coord::{LogicalPoint, PhysicalPoint};
use fidus_core::engine::Estimator;
use fidus_core::estimate::{EstimateError, MeasurementSource, ProbabilisticPosition};
use fidus_core::frame::CoordinateFrame;
use fidus_core::io::CaptureIo;

pub mod edge_sync;
pub mod fused;
pub mod kalman;
pub mod motion_gate;
pub mod template;

pub use fidus_core::target::{RgbaImage, TargetDescription};
pub use fused::FusedEstimator;
pub use kalman::VelocityTrack;

/// NCC score below which a match counts as "not found" (the usable floor
/// for textured templates, per the `template` module notes).
pub(crate) const MIN_SCORE: f64 = 0.35;
/// NCC score at which confidence saturates at 1.0.
pub(crate) const FULL_SCORE: f64 = 0.9;

/// One L1 template match, expressed in both coordinate spaces.
pub(crate) struct L1Match {
    /// Matched center in calibrated logical coordinates.
    pub position_logical: LogicalPoint,
    /// Matched center in capture pixels.
    pub center_physical: PhysicalPoint,
    /// Confidence mapped from the NCC score, in `[0, 1]`.
    pub confidence: f32,
    /// The template-sized bbox around the match, in capture pixels.
    pub bbox_physical: fidus_core::coord::BoundingBox,
}

/// Runs one L1 template match inside `roi` and maps the result through the
/// calibrated frame. Shared by the standalone [`FingerprintEstimator`] and
/// the fused L7 estimator.
pub(crate) fn l1_match(
    frame_img: &fidus_core::io::Frame,
    frame: &CoordinateFrame,
    tpl: &RgbaImage,
    roi: template::SearchRoi,
) -> Option<L1Match> {
    let m = template::match_template(frame_img, tpl, roi)?;
    if m.score < MIN_SCORE {
        return None;
    }
    let (tw, th) = (tpl.width as f64, tpl.height as f64);
    Some(L1Match {
        position_logical: frame.physical_to_logical(m.center),
        center_physical: m.center,
        confidence: ((m.score - MIN_SCORE) / (FULL_SCORE - MIN_SCORE)).clamp(0.0, 1.0) as f32,
        bbox_physical: fidus_core::coord::BoundingBox {
            x0: (m.center.x - tw / 2.0).round() as i64,
            y0: (m.center.y - th / 2.0).round() as i64,
            x1: (m.center.x + tw / 2.0).round() as i64,
            y1: (m.center.y + th / 2.0).round() as i64,
        },
    })
}

/// Placeholder estimator: always reports that tracking is not implemented.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullEstimator;

impl Estimator for NullEstimator {
    fn estimate(
        &mut self,
        _io: &mut dyn CaptureIo,
        _frame: &CoordinateFrame,
    ) -> Result<ProbabilisticPosition, EstimateError> {
        Err(EstimateError::NotImplementedYet {
            note: "L1/L8/EKF steady-state tracking is the P2 roadmap item",
        })
    }
}

/// L1 Fingerprint estimator: tracks the registered target by normalized
/// cross-correlation of its render against fidus' own captures.
///
/// # Search strategy
///
/// * first estimate: around the caller's `initial_center` belief when
///   provided, otherwise a full-screen scan;
/// * afterwards: a window around the last confirmed position, widened by
///   `64 px` per consecutive miss so a moved-away target is re-acquired
///   without unbounded cost.
///
/// # Honesty (spec §4.5)
///
/// A miss with a known prior reports the *prior* position at confidence
/// `0.0` — the pool never receives a fabricated measurement; sustained
/// low confidence is the caller's recalibration trigger. A miss without
/// any prior is [`EstimateError::TargetLost`].
pub struct FingerprintEstimator {
    target: Option<TargetDescription>,
    /// Last confirmed position in logical coordinates — a search hint only,
    /// never presented as a fresh measurement.
    last_logical: Option<LogicalPoint>,
    lost_streak: u32,
    /// Extra search margin around the predicted region, in capture pixels.
    pub search_margin_px: f64,
}

impl FingerprintEstimator {
    /// An estimator with defaults (`search_margin_px = 48`).
    pub fn new() -> Self {
        Self { target: None, last_logical: None, lost_streak: 0, search_margin_px: 48.0 }
    }

    /// The logical-space template resampled to capture-pixel scale.
    pub(crate) fn template_physical(&self, frame: &CoordinateFrame) -> Option<RgbaImage> {
        let t = self.target.as_ref()?;
        let s = frame.map().linear_scale();
        let (tw, th) = (
            ((t.template_logical.width as f64 * s).round() as u32).max(1),
            ((t.template_logical.height as f64 * s).round() as u32).max(1),
        );
        if (tw, th) == (t.template_logical.width, t.template_logical.height) {
            return Some(t.template_logical.clone());
        }
        Some(t.template_logical.resample(tw, th))
    }
}

impl Default for FingerprintEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl Estimator for FingerprintEstimator {
    fn register_target(&mut self, target: TargetDescription) -> Result<(), EstimateError> {
        if target.template_logical.width == 0 || target.template_logical.height == 0 {
            return Err(EstimateError::NoTarget);
        }
        self.target = Some(target);
        // The last position hint is deliberately kept: re-registration
        // (e.g. an appearance refresh) usually happens while the target is
        // on screen, and a stale hint only costs widened searches.
        Ok(())
    }

    fn estimate(
        &mut self,
        io: &mut dyn CaptureIo,
        frame: &CoordinateFrame,
    ) -> Result<ProbabilisticPosition, EstimateError> {
        let tpl = self.template_physical(frame).ok_or(EstimateError::NoTarget)?;
        let frame_img = io.capture()?;
        let (fw, fh) = frame_img.size();

        // Search window: prior position → caller belief → whole screen.
        let (center, half) = match self.last_logical {
            Some(p) => {
                let c = frame.logical_to_physical(p);
                let base = tpl.width.max(tpl.height) as f64;
                (
                    c,
                    base * 1.5 + self.search_margin_px + 64.0 * self.lost_streak as f64,
                )
            }
            None => match self.target.as_ref().and_then(|t| t.initial_center) {
                Some(c0) => {
                    let base = tpl.width.max(tpl.height) as f64;
                    (
                        frame.logical_to_physical(c0),
                        base * 1.5 + self.search_margin_px,
                    )
                }
                None => {
                    let c = fidus_core::coord::PhysicalPoint::new(fw as f64 / 2.0, fh as f64 / 2.0);
                    (c, fw.max(fh) as f64)
                }
            },
        };

        let roi = template::SearchRoi { center: (center.x, center.y), half };
        let source = MeasurementSource::Single("L1 Fingerprint");
        match l1_match(&frame_img, frame, &tpl, roi) {
            Some(m) => {
                let position = m.position_logical;
                self.last_logical = Some(position);
                self.lost_streak = 0;
                let confidence = m.confidence;
                Ok(ProbabilisticPosition {
                    position,
                    confidence,
                    bbox_physical: Some(m.bbox_physical),
                    measured_at: std::time::Instant::now(),
                    source,
                })
            }
            _ => {
                self.lost_streak = self.lost_streak.saturating_add(1);
                match self.last_logical {
                    Some(p) => Ok(ProbabilisticPosition {
                        position: p,
                        confidence: 0.0,
                        bbox_physical: None,
                        measured_at: std::time::Instant::now(),
                        source,
                    }),
                    None => Err(EstimateError::TargetLost),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidus_core::calibration::CalibrationMethod;
    use fidus_core::coord::AffineTransform;
    use fidus_core::frame::CalibrationQuality;
    use fidus_core::io::{CaptureError, Frame, PixelFormat};
    use std::time::SystemTime;

    const W: u32 = 1000;
    const H: u32 = 750;

    /// Frame for a 500×375 logical output captured at 2× (1000×750 px).
    fn test_frame() -> CoordinateFrame {
        let map = AffineTransform { a: 2.0, b: 0.0, c: 0.0, d: 0.0, e: 2.0, f: 0.0 };
        let quality = CalibrationQuality {
            rms_residual_px: 0.0,
            max_residual_px: 0.0,
            verification_max_err_px: 0.0,
            consistency_max_err_px: 0.0,
            sample_count: 4,
            independent_passes: 2,
        };
        CoordinateFrame::new(map, (W, H), CalibrationMethod::Crosshair, quality, SystemTime::UNIX_EPOCH)
            .expect("invertible map")
    }

    /// A deterministic textured 60×40 logical template (NCC needs texture).
    fn test_template() -> RgbaImage {
        let (w, h) = (60u32, 40u32);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = (((x * 7) ^ (y * 13)) % 251) as u8;
                let g = ((v as u16 * 3) % 251) as u8;
                data.extend_from_slice(&[v, g, 250 - v, 255]);
            }
        }
        RgbaImage::from_raw(w, h, data)
    }

    /// Synthetic output: flat gray background with the template pasted at
    /// the given physical top-left corner (`None` = target absent).
    struct FakeCapture {
        top_left: Option<(f64, f64)>,
    }

    impl CaptureIo for FakeCapture {
        fn capture(&mut self) -> Result<Frame, CaptureError> {
            let format = PixelFormat::Argb8888;
            let mut data = vec![90u8; (W * H * 4) as usize];
            if let Some((tx, ty)) = self.top_left {
                // The target is a 60×40-*logical* window; the compositor
                // renders it at this output's 2× scale, so the scene shows
                // it at 120×80 *physical* pixels — exactly what the
                // estimator resamples the logical template into.
                let tpl = test_template().resample(120, 80);
                for yy in 0..tpl.height {
                    for xx in 0..tpl.width {
                        let px = tx as i64 + xx as i64;
                        let py = ty as i64 + yy as i64;
                        if px < 0 || py < 0 || px >= W as i64 || py >= H as i64 {
                            continue;
                        }
                        let [r, g, b, _] = tpl.rgba(xx, yy);
                        let i = (py as u32 * W + px as u32) as usize * 4;
                        format.write_rgba(&mut data, i, [r, g, b, 255]);
                    }
                }
            }
            Ok(Frame { width: W, height: H, stride: W * 4, format, data })
        }
    }

    fn register(est: &mut FingerprintEstimator, initial: Option<LogicalPoint>) {
        est.register_target(TargetDescription {
            template_logical: test_template(),
            initial_center: initial,
        })
        .expect("valid target");
    }

    #[test]
    fn tracks_from_caller_belief_and_reports_confidence() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        register(&mut est, Some(LogicalPoint::new(200.0, 150.0)));
        // Belief (200,150) logical → (400,300) physical; the 120×80 px
        // template centered there has its top-left at (340,260).
        let mut io = FakeCapture { top_left: Some((340.0, 260.0)) };
        let p = est.estimate(&mut io, &frame).expect("found");
        assert!((p.position.x - 200.0).abs() < 1.5, "x = {}", p.position.x);
        assert!((p.position.y - 150.0).abs() < 1.5, "y = {}", p.position.y);
        assert!(p.confidence > 0.6, "confidence = {}", p.confidence);
        assert_eq!(p.source, MeasurementSource::Single("L1 Fingerprint"));
        let bbox = p.bbox_physical.expect("bbox present");
        assert!((bbox.center().x - 400.0).abs() <= 1.0);
    }

    #[test]
    fn follows_movement_within_the_search_window() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        register(&mut est, Some(LogicalPoint::new(200.0, 150.0)));
        let mut io = FakeCapture { top_left: Some((340.0, 260.0)) };
        let _ = est.estimate(&mut io, &frame).expect("first fix");

        // Target moved +40 logical px to the right (+80 capture px).
        io.top_left = Some((420.0, 260.0));
        let p = est.estimate(&mut io, &frame).expect("second fix");
        assert!((p.position.x - 240.0).abs() < 1.5, "x = {}", p.position.x);
        assert!((p.position.y - 150.0).abs() < 1.5);
    }

    #[test]
    fn full_screen_search_when_no_belief() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        register(&mut est, None);
        let mut io = FakeCapture { top_left: Some((340.0, 260.0)) };
        let p = est.estimate(&mut io, &frame).expect("found");
        assert!((p.position.x - 200.0).abs() < 1.5);
        assert!((p.position.y - 150.0).abs() < 1.5);
    }

    #[test]
    fn estimate_without_target_is_no_target() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        let mut io = FakeCapture { top_left: None };
        assert!(matches!(est.estimate(&mut io, &frame), Err(EstimateError::NoTarget)));
    }

    #[test]
    fn lost_target_reports_prior_at_zero_confidence() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        register(&mut est, Some(LogicalPoint::new(200.0, 150.0)));
        let mut io = FakeCapture { top_left: Some((340.0, 260.0)) };
        let _ = est.estimate(&mut io, &frame).expect("first fix");

        // The target disappears from the screen.
        io.top_left = None;
        let p = est.estimate(&mut io, &frame).expect("prior reported");
        assert_eq!(p.confidence, 0.0);
        assert!((p.position.x - 200.0).abs() < 1.5);
    }

    #[test]
    fn first_search_failure_is_target_lost() {
        let frame = test_frame();
        let mut est = FingerprintEstimator::new();
        register(&mut est, None);
        let mut io = FakeCapture { top_left: None };
        assert!(matches!(est.estimate(&mut io, &frame), Err(EstimateError::TargetLost)));
    }

    #[test]
    fn null_estimator_refuses_registration() {
        let mut e = NullEstimator;
        let r = e.register_target(TargetDescription {
            template_logical: test_template(),
            initial_center: None,
        });
        assert!(matches!(r, Err(EstimateError::NotImplementedYet { .. })));
    }
}
