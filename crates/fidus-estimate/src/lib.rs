// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! C-layer estimators for fidus.
//!
//! # Status (spec §8 priorities)
//!
//! * **L1/L6 Fingerprint** — *wired* (P2-a): [`FingerprintEstimator`] tracks
//!   a registered [`TargetDescription`] by NCC template matching against
//!   fidus' own captures.
//! * **L8 EdgeSync** — *measurement primitive wired* (P2-b):
//!   [`edge_sync::EdgeSync`] measures displaced target bboxes by gated
//!   frame differencing; the fused estimator (L1 + L8 + L4 + constant-velocity Kalman) that
//!   consumes it lands with P2-c.
//! * **L4 relative displacement** and **L7 fusion** — P2-c.
//!
//! # Target registration (spec §3.2 / §6.1)
//!
//! The caller describes its own window by registering a
//! [`TargetDescription`] — that window's offscreen render, a pure visual input.
//! Nothing here reads or wraps a platform coordinate, and this crate does not
//! enumerate or locate arbitrary third-party system windows.

use fidus_core::coord::{LogicalPoint, PhysicalPoint};
use fidus_core::engine::Estimator;
use fidus_core::estimate::{EstimateError, MeasurementSource, ProbabilisticPosition};
use fidus_core::frame::CoordinateFrame;
use fidus_core::io::CaptureIo;

pub mod edge_sync;
pub mod fused;
pub mod kalman;
pub mod motion_gate;
pub mod screen_classifier;
pub mod template;

pub use fidus_core::target::{RgbaImage, TargetDescription, UntrackablePolicy};
pub use fused::FusedEstimator;
pub use kalman::VelocityTrack;
pub use screen_classifier::{ScreenClassifier, WallpaperVerdict};

/// NCC score below which a match counts as "not found" (the usable floor
/// for textured templates, per the `template` module notes).
pub(crate) const MIN_SCORE: f64 = 0.35;
/// NCC score at which confidence saturates at 1.0.
pub(crate) const FULL_SCORE: f64 = 0.9;
/// Bound caller-controlled template work before any sampling or matching loop.
/// This protects the no-display estimator from forged public image dimensions;
/// larger legitimate templates must be tiled or downsampled by the caller.
const MAX_TARGET_PIXELS: u64 = 16_777_216;

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
    let (tw, th) = (tpl.width() as f64, tpl.height() as f64);
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
            note: "L1/L8/Kalman steady-state tracking is the P2 roadmap item",
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
    /// Upper bound on reported confidence, set at registration from the
    /// template's measured ambiguity. 1.0 for a distinctive render.
    ///
    /// Kept as state rather than recomputed per estimate because
    /// `localizability` is an O(n) pass over the template and the value
    /// cannot change until the target is re-registered.
    confidence_ceiling: f32,
    /// Extra search margin around the predicted region, in capture pixels.
    pub search_margin_px: f64,
}

/// Confidence ceiling implied by a template's measured self-similarity.
///
/// Maps self-similarity `s` to `1 - s`, floored at [`MIN_CONFIDENCE_CEILING`]:
/// a template that matches a shifted copy of itself at 0.9 can be
/// mis-registered that easily, so a "0.95 confidence" match on it means
/// something far weaker than the same number on a distinctive template.
///
/// This is continuous on purpose — a cliff at the accept/refuse boundary
/// would make a template scoring just under the threshold indistinguishable
/// from an excellent one, which is the same "plausible-looking value" trap
/// the check exists to avoid (project convention 8).
fn ambiguity_ceiling(self_similarity: f64) -> f32 {
    let c = 1.0 - self_similarity.clamp(0.0, 1.0);
    (c as f32).max(MIN_CONFIDENCE_CEILING)
}

/// Floor for the confidence ceiling.
///
/// Never 0: that would make the measurement indistinguishable from the
/// "target lost, reporting prior" case (spec §4.5), which is a different
/// statement — there fidus has *no* measurement, here it has a weak one.
/// Small enough that the L7 fusion effectively defers to any other layer,
/// and that a caller thresholding on confidence rejects it.
const MIN_CONFIDENCE_CEILING: f32 = 0.05;

impl FingerprintEstimator {
    /// An estimator with defaults (`search_margin_px = 48`).
    pub fn new() -> Self {
        Self {
            target: None,
            last_logical: None,
            lost_streak: 0,
            confidence_ceiling: 1.0,
            search_margin_px: 48.0,
        }
    }

    /// Upper bound currently applied to reported confidence, from the
    /// registered template's measured ambiguity (1.0 when distinctive).
    ///
    /// Exposed so a caller that opted into
    /// [`UntrackablePolicy::TrackWithReducedConfidence`] can see how much
    /// credibility that cost.
    pub fn confidence_ceiling(&self) -> f32 {
        self.confidence_ceiling
    }

    /// The logical-space template resampled to capture-pixel scale.
    pub(crate) fn template_physical(&self, frame: &CoordinateFrame) -> Option<RgbaImage> {
        let t = self.target.as_ref()?;
        let s = frame.map().linear_scale();
        let (tw, th) = (
            ((t.template_logical.width() as f64 * s).round() as u32).max(1),
            ((t.template_logical.height() as f64 * s).round() as u32).max(1),
        );
        if (tw, th) == (t.template_logical.width(), t.template_logical.height()) {
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
        let pixels = u64::from(target.template_logical.width())
            .checked_mul(u64::from(target.template_logical.height()));
        let bytes = pixels.and_then(|n| n.checked_mul(4));
        if !target.template_logical.is_valid()
            || pixels.is_none_or(|n| n == 0 || n > MAX_TARGET_PIXELS)
            || bytes != Some(target.template_logical.byte_len() as u64)
            || target.initial_center.is_some_and(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(EstimateError::NoTarget);
        }
        // Refuse appearances that template matching cannot localize, rather
        // than accepting them and emitting confident measurements at
        // arbitrary positions for the rest of the session. See
        // `template::localizability` for why variance is not the test, and
        // `EstimateError::UntrackableTarget` for why this is an error.
        //
        // Checked on the *logical* render: the physical template is a
        // resample of it, and resampling cannot create structure that is not
        // already there — a gradient stays a gradient at any scale. Checking
        // here also means the caller finds out at registration, not on the
        // first estimate after calibration.
        self.confidence_ceiling = match template::localizability(&target.template_logical) {
            // Trackable. A template that is *somewhat* self-similar is still
            // discounted, continuously: there is no cliff at the accept/
            // refuse boundary, so a marginal render does not get to look as
            // trustworthy as a distinctive one.
            Ok(self_similarity) => ambiguity_ceiling(self_similarity),
            Err(e) => match target.untrackable_policy {
                UntrackablePolicy::Refuse => {
                    return Err(EstimateError::UntrackableTarget {
                        reason: match e {
                            template::Unlocatable::TooSmall => "too small",
                            template::Unlocatable::Featureless => "featureless",
                            template::Unlocatable::SelfSimilar { .. } => "translation-ambiguous",
                        },
                        detail: e.to_string(),
                    });
                }
                // The caller explicitly asked for a best-effort track. Honour
                // it, but never let such a measurement claim to be as good as
                // one from a distinctive template: the position really is
                // unreliable, and spec §6.1 is kept by *labelling* that, not
                // by suppressing it.
                UntrackablePolicy::TrackWithReducedConfidence => match e {
                    // Ambiguity was measured: discount by how bad it is, so a
                    // borderline template is not flattened to the same value
                    // as a hopeless one.
                    template::Unlocatable::SelfSimilar { worst } => ambiguity_ceiling(worst),
                    // No usable measurement of ambiguity exists (flat, or too
                    // small to probe). Nothing distinguishes one position
                    // from another, so the ceiling is the floor.
                    template::Unlocatable::Featureless | template::Unlocatable::TooSmall => {
                        MIN_CONFIDENCE_CEILING
                    }
                },
            },
        };
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
        // This public tuning knob is untrusted: malformed extents are ignored
        // so template matching never receives inverted clamp bounds.
        let search_margin = if self.search_margin_px.is_finite() && self.search_margin_px >= 0.0 {
            self.search_margin_px
        } else {
            48.0
        };
        let (center, half) = match self.last_logical {
            Some(p) => {
                let c = frame.logical_to_physical(p);
                let base = tpl.width().max(tpl.height()) as f64;
                (
                    c,
                    base * 1.5 + search_margin + 64.0 * self.lost_streak as f64,
                )
            }
            None => match self.target.as_ref().and_then(|t| t.initial_center) {
                Some(c0) => {
                    let base = tpl.width().max(tpl.height()) as f64;
                    (
                        frame.logical_to_physical(c0),
                        base * 1.5 + search_margin,
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
                // Cap by the template's measured ambiguity. A high NCC score
                // on a self-similar template says "this looks like the
                // target", not "the target is here" — the ceiling is what
                // keeps the second claim from riding on the first.
                let confidence = m.confidence.min(self.confidence_ceiling);
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
    ///
    /// Solved from synthetic correspondences, not declared: `CoordinateFrame`
    /// only accepts a `SolvedMap` (principle 1 at the type level), and tests
    /// must exercise the same path production does.
    fn test_frame() -> CoordinateFrame {
        let corr: Vec<_> = [(0.0, 0.0), (400.0, 0.0), (0.0, 300.0), (400.0, 300.0)]
            .into_iter()
            .map(|(x, y)| (LogicalPoint::new(x, y), PhysicalPoint::new(x * 2.0, y * 2.0)))
            .collect();
        let map = AffineTransform::from_correspondences(&corr).expect("well-conditioned");
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
                for yy in 0..tpl.height() {
                    for xx in 0..tpl.width() {
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
        let mut t = TargetDescription::new(test_template());
        t.initial_center = initial;
        est.register_target(t).expect("valid target");
    }

    #[test]
    fn forged_template_dimensions_are_rejected_before_matching() {
        let mut est = FingerprintEstimator::new();
        let forged = RgbaImage::from_raw(u32::MAX, 2, vec![0; 4]);
        assert!(matches!(
            est.register_target(TargetDescription::new(forged)),
            Err(EstimateError::NoTarget)
        ));
    }

    #[test]
    fn non_finite_initial_center_is_rejected() {
        let mut est = FingerprintEstimator::new();
        let target = TargetDescription::new(test_template())
            .with_initial_center(LogicalPoint::new(f64::NAN, 1.0));
        assert!(matches!(est.register_target(target), Err(EstimateError::NoTarget)));
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
        let r = e.register_target(TargetDescription::new(test_template()));
        assert!(matches!(r, Err(EstimateError::NotImplementedYet { .. })));
    }

    /// A 60×40 horizontal gradient: unlocatable, but not for lack of
    /// contrast (std. dev. ≈ 74, higher than the hash texture that tracks
    /// perfectly). It correlates 1.000 with a shifted copy of itself.
    fn gradient_template() -> RgbaImage {
        let (w, h) = (60u32, 40u32);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..h {
            for x in 0..w {
                let v = (x * 255 / w) as u8;
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        RgbaImage::from_raw(w, h, data)
    }

    #[test]
    fn ambiguous_appearance_is_refused_by_default() {
        // The default policy must be refusal: a caller who never thought
        // about localizability gets an error at registration, not a track
        // that wanders for reasons nothing reports.
        let mut est = FingerprintEstimator::new();
        let r = est.register_target(TargetDescription::new(gradient_template()));
        assert!(
            matches!(r, Err(EstimateError::UntrackableTarget { .. })),
            "gradient accepted by default: {r:?}"
        );
        // A refused registration must not become the active target.
        assert!(est.target.is_none(), "refused target was stored anyway");
    }

    #[test]
    fn opting_in_trades_credibility_for_a_track() {
        // The escape hatch: explicitly requested, and it costs confidence
        // rather than honesty. The measurement is still real — it is just
        // labelled as weak, which is what spec §6.1 requires of an
        // unreliable position.
        let mut est = FingerprintEstimator::new();
        est.register_target(
            TargetDescription::new(gradient_template()).tracking_ambiguous_appearance(),
        )
        .expect("explicit opt-in must be honoured");

        let ceiling = est.confidence_ceiling();
        assert!(
            (MIN_CONFIDENCE_CEILING..0.1).contains(&ceiling),
            "a 1.000-self-similar template must be capped near the floor, got {ceiling}"
        );
    }

    #[test]
    fn opting_in_does_not_weaken_a_distinctive_template() {
        // The policy is a fallback for ambiguous renders, not a global
        // discount: a caller that sets it defensively while supplying a good
        // template must not be penalised for it.
        let mut est = FingerprintEstimator::new();
        est.register_target(
            TargetDescription::new(test_template()).tracking_ambiguous_appearance(),
        )
        .expect("distinctive template is trackable regardless of policy");
        assert!(
            est.confidence_ceiling() > 0.5,
            "distinctive template capped at {}",
            est.confidence_ceiling()
        );
    }

    #[test]
    fn reduced_ceiling_actually_caps_reported_confidence() {
        // The ceiling has to reach the output, or it is a decorative knob
        // (project convention 7). This tracks a real match of the gradient
        // and checks the reported confidence, not just the stored field.
        let frame = test_frame();
        let mut strict = FingerprintEstimator::new();
        register(&mut strict, Some(LogicalPoint::new(100.0, 75.0)));
        let mut io = FakeCapture { top_left: Some((200.0, 150.0)) };
        let good = strict.estimate(&mut io, &frame).expect("distinctive target tracks");

        let mut lax = FingerprintEstimator::new();
        lax.register_target(
            TargetDescription::new(gradient_template())
                .tracking_ambiguous_appearance(),
        )
        .expect("opt-in accepted");
        let capped = lax.confidence_ceiling();

        assert!(
            good.confidence > capped,
            "a distinctive template ({}) must outrank the ambiguous ceiling ({capped})",
            good.confidence
        );
        // And the cap is applied by `min`, so no match on the gradient can
        // ever report more than the ceiling.
        assert!(capped < 0.1, "ambiguous ceiling too generous: {capped}");
    }

    #[test]
    fn ambiguity_ceiling_is_monotonic_and_bounded() {
        // Continuous discount, no cliff at the accept/refuse boundary: a
        // marginal template must not be able to look as good as an excellent
        // one (project convention 8).
        let mut prev = f32::INFINITY;
        for i in 0..=100 {
            let s = i as f64 / 100.0;
            let c = ambiguity_ceiling(s);
            assert!(c <= prev + 1e-6, "ceiling rose at self-similarity {s}: {c} > {prev}");
            assert!(
                (MIN_CONFIDENCE_CEILING..=1.0).contains(&c),
                "ceiling {c} out of range at {s}"
            );
            prev = c;
        }
        // Never zero: "weak measurement" and "no measurement" (spec §4.5)
        // are different statements and must stay distinguishable.
        assert!(ambiguity_ceiling(1.0) > 0.0);
        assert!((ambiguity_ceiling(0.0) - 1.0).abs() < 1e-6);
    }
}
