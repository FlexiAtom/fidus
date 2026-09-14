// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! L7 fusion (P2-c): L1 template matching + L8 gated differencing into a
//! constant-velocity track.
//!
//! # Flow per estimate
//!
//! 1. capture once;
//! 2. KF predict (when a fix exists) → the predicted position drives both
//!    layers' search windows;
//! 3. L8 differ (only with a prior): gated, template-verified displacement
//!    measurements (see the `edge_sync` module for the gate refinement);
//! 4. L1 template match inside the predicted ROI;
//! 5. fuse: agreeing measurements corroborate each other (confidence bonus),
//!    disagreeing ones are resolved by confidence (L1 wins ties — its
//!    subpixel NCC over the full window is the stronger instrument);
//! 6. the fused measurement updates the track; the posterior is reported.
//!
//! # Honesty (spec §4.5)
//!
//! A frame where *neither* layer finds the target never produces a fabricated
//! measurement: with a fix the track simply **coasts** (constant velocity)
//! at confidence `0.0` — exactly the "prior at zero confidence" rule of the
//! L1 estimator, with motion extrapolation instead of a frozen position. A
//! re-acquisition after a loss **resets** the track onto the measurement
//! (stale velocity would smear the jump); the first fix initializes it.
//!
//! All inputs are fidus' own captures plus the caller's own render — the
//! pool stays pure.

use std::time::Instant;

use fidus_core::coord::{BoundingBox, LogicalPoint, PhysicalPoint};
use fidus_core::engine::Estimator;
use fidus_core::estimate::{EstimateError, MeasurementSource, ProbabilisticPosition};
use fidus_core::frame::CoordinateFrame;
use fidus_core::io::CaptureIo;

use crate::edge_sync::{EdgeSync, EdgeSyncOutcome};
use crate::kalman::VelocityTrack;
use crate::template::SearchRoi;
use crate::{l1_match, FingerprintEstimator};

/// L1/L8 centers within this distance count as agreeing.
const AGREE_PX: f64 = 8.0;
/// Confidence bonus when both layers measured and agree.
const CORROBORATION_BONUS: f32 = 0.15;
/// Default measurement variance at confidence 1.0 (≈1 px std. dev.).
const DEFAULT_R_MIN_PX2: f64 = 1.0;
/// Default measurement variance at confidence 0.0 (≈20 px std. dev.).
const DEFAULT_R_MAX_PX2: f64 = 400.0;

fn finite_nonnegative(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value >= 0.0 { value } else { fallback }
}

/// The fused L7 estimator (P2-c).
pub struct FusedEstimator {
    /// The L1 layer: target storage and template matching.
    l1: FingerprintEstimator,
    /// The L8 layer: gated frame differencing.
    differ: EdgeSync,
    /// The motion track (L4 model).
    track: VelocityTrack,
    has_fix: bool,
    last_update: Option<Instant>,
    /// Last measured target bbox in capture pixels — the L8 gate region.
    last_bbox: Option<BoundingBox>,
    lost_streak: u32,
    /// Injectable clock (tests script time; production uses `Instant::now`).
    clock: Box<dyn Fn() -> Instant + Send>,

    /// White-acceleration process noise, px/s². Higher follows drags more
    /// aggressively at the cost of noisier coasting.
    pub process_noise: f64,
    /// Measurement variance at confidence 1.0, px². Read by
    /// `r_of`; values that are not finite and non-negative fall back
    /// to the default.
    pub r_min_px2: f64,
    /// Measurement variance at confidence 0.0, px². Read by
    /// `r_of`; same validation as [`Self::r_min_px2`].
    pub r_max_px2: f64,
    /// Extra search margin around the predicted region, in capture pixels.
    pub search_margin_px: f64,
    /// Search-window widening per consecutive miss, in capture pixels.
    pub widen_per_miss_px: f64,
}

impl FusedEstimator {
    /// Smallest measurement variance the filter will ever accept, px².
    ///
    /// `r = 0` claims a noiseless measurement: the Kalman gain becomes 1, the
    /// posterior variance collapses to 0, and from then on the filter ignores
    /// every future measurement — an absorbing state (project convention 2).
    /// 1e-6 px² is far below any real detector's precision while keeping the
    /// gain finite.
    pub const MIN_VARIANCE_PX2: f64 = 1e-6;

    /// A fused estimator with defaults and the wall clock.
    pub fn new() -> Self {
        Self::with_clock(Box::new(Instant::now))
    }

    /// A fused estimator with a scripted clock (tests).
    pub fn with_clock(clock: Box<dyn Fn() -> Instant + Send>) -> Self {
        FusedEstimator {
            l1: FingerprintEstimator::new(),
            differ: EdgeSync::new(),
            track: VelocityTrack::uninformative(),
            has_fix: false,
            last_update: None,
            last_bbox: None,
            lost_streak: 0,
            clock,
            process_noise: 2000.0,
            r_min_px2: DEFAULT_R_MIN_PX2,
            r_max_px2: DEFAULT_R_MAX_PX2,
            search_margin_px: 48.0,
            widen_per_miss_px: 64.0,
        }
    }

    /// Replaces the clock. Production code never needs this; deterministic
    /// callers (tests, replay harnesses) do.
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> Instant + Send>) {
        self.clock = clock;
    }

    /// Measurement variance derived from confidence, interpolating between
    /// the configured [`r_min_px2`](Self::r_min_px2) (confidence 1) and
    /// [`r_max_px2`](Self::r_max_px2) (confidence 0).
    ///
    /// # Why this reads the fields (project convention 7)
    ///
    /// It used to hard-code `1.0 + (1 - c) * 399.0`, i.e. the *default*
    /// values of the two public fields, baked in. Both knobs were therefore
    /// decorative: a caller tuning `r_max_px2` for a noisy display changed
    /// nothing, and no error said so. A public field the implementation does
    /// not read is a lie told by the API.
    ///
    /// # Failure mode and why it is contained
    ///
    /// The fields are `pub`, so a caller can set them to anything, including
    /// `r_min > r_max` or negative values. Rather than trusting them:
    ///
    /// * non-finite or negative inputs fall back to the defaults — a variance
    ///   must be a non-negative real or the Kalman gain is meaningless;
    /// * the interval is ordered by `min`/`max`, so a swapped pair degrades
    ///   into a valid (if inverted-intent) range instead of producing a
    ///   negative width and a variance that *decreases* with uncertainty;
    /// * the result is floored at [`MIN_VARIANCE_PX2`](Self::MIN_VARIANCE_PX2)
    ///   because `r = 0` asserts a perfect measurement: the filter would
    ///   discard its entire prior in one step, and any later disagreement
    ///   would be impossible to reconcile.
    fn r_of(&self, confidence: f32) -> f64 {
        let sane = |v: f64, fallback: f64| if v.is_finite() && v >= 0.0 { v } else { fallback };
        let lo = sane(self.r_min_px2, DEFAULT_R_MIN_PX2);
        let hi = sane(self.r_max_px2, DEFAULT_R_MAX_PX2);
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let c = if confidence.is_finite() { confidence.clamp(0.0, 1.0) as f64 } else { 0.0 };
        (hi + (lo - hi) * c).max(Self::MIN_VARIANCE_PX2)
    }

    /// Half-extent of the search window around a predicted center.
    fn search_half(&self, tpl: &fidus_core::target::RgbaImage) -> f64 {
        // These knobs are public input. Negative or non-finite extents can make
        // SearchRoi's lower bound exceed its upper bound and panic in clamp;
        // ignore malformed tuning rather than turning it into a fake geometry.
        let margin = finite_nonnegative(self.search_margin_px, 48.0);
        let widen = finite_nonnegative(self.widen_per_miss_px, 64.0);
        let base = tpl.width().max(tpl.height()) as f64 * 1.5;
        (base + margin + widen * self.lost_streak as f64).max(0.0)
    }

    /// Assimilates a fused measurement: reset on first fix / re-acquisition,
    /// KF update otherwise. Returns the posterior position.
    fn absorb(
        &mut self,
        position_logical: LogicalPoint,
        confidence: f32,
        bbox_physical: BoundingBox,
    ) {
        if !self.has_fix || self.lost_streak > 0 {
            // First fix, or re-acquisition: stale velocity must not smear
            // the jump onto the measured position.
            self.track.reset(position_logical);
            self.has_fix = true;
        } else {
            let r = self.r_of(confidence);
            self.track.update(position_logical, r);
        }
        self.lost_streak = 0;
        self.last_bbox = Some(bbox_physical);
    }
}

impl Default for FusedEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl Estimator for FusedEstimator {
    fn register_target(
        &mut self,
        target: fidus_core::target::TargetDescription,
    ) -> Result<(), EstimateError> {
        // A new appearance invalidates the differ's reference and the
        // position belief; start acquisition fresh.
        self.l1.register_target(target)?;
        self.differ = EdgeSync::new();
        self.has_fix = false;
        self.last_update = None;
        self.last_bbox = None;
        self.lost_streak = 0;
        Ok(())
    }

    fn estimate(
        &mut self,
        io: &mut dyn CaptureIo,
        frame: &CoordinateFrame,
    ) -> Result<ProbabilisticPosition, EstimateError> {
        let tpl = self.l1.template_physical(frame).ok_or(EstimateError::NoTarget)?;
        let img = io.capture()?;
        let now = (self.clock)();
        let (fw, fh) = img.size();
        let source = MeasurementSource::Single("L7 Fusion");

        // 1. Predict (physical space) when a fix exists.
        let predicted: Option<PhysicalPoint> = if self.has_fix {
            let dt = self
                .last_update
                .map(|t| now.duration_since(t).as_secs_f64())
                .unwrap_or(0.0)
                .clamp(0.0, 5.0);
            self.track.predict(dt, self.process_noise);
            self.last_update = Some(now);
            Some(frame.logical_to_physical(self.track.position()))
        } else {
            None
        };

        // 2. L8 gated differencing (needs a prior region to gate on).
        let l8 = predicted.and_then(|pred| {
            let gate_region = self
                .last_bbox
                .unwrap_or_else(|| bbox_around(pred, tpl.width() as f64, tpl.height() as f64));
            let half = self.search_half(&tpl);
            let search_roi = BoundingBox {
                x0: (pred.x - half) as i64,
                y0: (pred.y - half) as i64,
                x1: (pred.x + half) as i64,
                y1: (pred.y + half) as i64,
            };
            match self.differ.observe(&img, gate_region, search_roi, &tpl, now) {
                EdgeSyncOutcome::Measured(o) => Some((o.bbox.center(), o.confidence, o.bbox)),
                _ => None,
            }
        });

        // 3. L1 template match inside the predicted window.
        let l1 = {
            let (center, half) = match predicted {
                Some(p) => (p, self.search_half(&tpl)),
                None => {
                    let (tw, th) = (tpl.width().max(tpl.height()) as f64, tpl.height() as f64);
                    (
                        PhysicalPoint::new(fw as f64 / 2.0, fh as f64 / 2.0),
                        fw.max(fh) as f64 + tw.max(th),
                    )
                }
            };
            l1_match(&img, frame, &tpl, SearchRoi { center: (center.x, center.y), half })
        };

        // 4.–6. Fuse and assimilate.
        // Both layers verify the same registered appearance, so both must obey
        // its ambiguity ceiling before fusion.  Capping only FingerprintEstimator
        // is insufficient: L1Match is shared with this path and L8 can otherwise
        // win the comparison (or the corroboration bonus can raise L1 above the
        // ceiling), turning a known ambiguous render into a high-confidence KF
        // measurement.  If this cap is ever removed, the failure mode is a
        // silent fictitious position, not an error; the registration policy's
        // explicit reduced-confidence opt-in is the only safe fallback.
        let cap = self.l1.confidence_ceiling();
        let l1 = l1.map(|mut a| {
            a.confidence = a.confidence.min(cap);
            a
        });
        let l8 = l8.map(|(p, confidence, bbox)| (p, confidence.min(cap), bbox));
        match (l1, l8) {
            (Some(a), Some(b)) => {
                let agree = a.center_physical.distance(b.0) <= AGREE_PX;
                let (pos, conf, bbox) = if agree {
                    // Corroborated: keep L1's subpixel position, but never let
                    // the bonus exceed the registered appearance's ceiling.
                    (a.position_logical, (a.confidence + CORROBORATION_BONUS).min(cap), a.bbox_physical)
                } else if a.confidence >= b.1 {
                    (a.position_logical, a.confidence, a.bbox_physical)
                } else {
                    (frame.physical_to_logical(b.0), b.1, b.2)
                };
                self.absorb(pos, conf, bbox);
                Ok(ProbabilisticPosition {
                    position: self.track.position(),
                    confidence: conf,
                    bbox_physical: Some(bbox),
                    measured_at: now,
                    source,
                })
            }
            (Some(a), None) => {
                let conf = a.confidence;
                let bbox = a.bbox_physical;
                self.absorb(a.position_logical, conf, bbox);
                Ok(ProbabilisticPosition {
                    position: self.track.position(),
                    confidence: conf,
                    bbox_physical: Some(bbox),
                    measured_at: now,
                    source,
                })
            }
            (None, Some(b)) => {
                let pos = frame.physical_to_logical(b.0);
                let (conf, bbox) = (b.1, b.2);
                self.absorb(pos, conf, bbox);
                Ok(ProbabilisticPosition {
                    position: self.track.position(),
                    confidence: conf,
                    bbox_physical: Some(bbox),
                    measured_at: now,
                    source,
                })
            }
            (None, None) => {
                self.lost_streak = self.lost_streak.saturating_add(1);
                if self.has_fix {
                    // Coasting: the prediction step already advanced the
                    // track; report the extrapolation at zero confidence —
                    // a clearly-labeled belief, not a measurement.
                    Ok(ProbabilisticPosition {
                        position: self.track.position(),
                        confidence: 0.0,
                        bbox_physical: None,
                        measured_at: now,
                        source,
                    })
                } else {
                    Err(EstimateError::TargetLost)
                }
            }
        }
    }
}

/// A bbox centered at `c` with the given *logical* template size scaled by
/// the frame — used as the L8 gate region before any measurement exists.
fn bbox_around(c: PhysicalPoint, w: f64, h: f64) -> BoundingBox {
    BoundingBox {
        x0: (c.x - w / 2.0) as i64,
        y0: (c.y - h / 2.0) as i64,
        x1: (c.x + w / 2.0) as i64,
        y1: (c.y + h / 2.0) as i64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_of_spans_the_configured_interval() {
        // Project convention 7: the public knobs must actually be read. The
        // old `r_of` hard-coded `1.0 + (1-c)*399.0`, so these assertions all
        // failed at the non-default settings below while the API kept
        // advertising the fields.
        let mut e = FusedEstimator::new();
        assert!((e.r_of(1.0) - DEFAULT_R_MIN_PX2).abs() < 1e-9);
        assert!((e.r_of(0.0) - DEFAULT_R_MAX_PX2).abs() < 1e-9);

        e.r_min_px2 = 4.0;
        e.r_max_px2 = 900.0;
        assert!((e.r_of(1.0) - 4.0).abs() < 1e-9, "tuned r_min ignored");
        assert!((e.r_of(0.0) - 900.0).abs() < 1e-9, "tuned r_max ignored");
        assert!((e.r_of(0.5) - 452.0).abs() < 1e-9, "midpoint {}", e.r_of(0.5));
    }

    #[test]
    fn r_of_is_monotonic_in_confidence() {
        // Higher confidence must never mean *more* assumed noise, or the
        // filter would weight its worst measurements most heavily.
        let e = FusedEstimator::new();
        let mut prev = f64::INFINITY;
        for i in 0..=100 {
            let r = e.r_of(i as f32 / 100.0);
            assert!(r <= prev + 1e-12, "r rose at confidence {}: {r} > {prev}", i as f32 / 100.0);
            prev = r;
        }
    }

    #[test]
    fn hostile_knob_settings_cannot_break_the_filter() {
        // The fields are pub, so they are untrusted input. None of these may
        // yield a negative, NaN, or zero variance.
        let mut e = FusedEstimator::new();
        for (lo, hi) in [
            (f64::NAN, 100.0),
            (-5.0, 100.0),
            (900.0, 4.0),           // swapped
            (0.0, 0.0),             // both zero: would give gain 1 forever
            (f64::INFINITY, 1.0),
            (1.0, f64::NEG_INFINITY),
        ] {
            e.r_min_px2 = lo;
            e.r_max_px2 = hi;
            for c in [0.0f32, 0.5, 1.0, f32::NAN] {
                let r = e.r_of(c);
                assert!(r.is_finite(), "r = {r} for ({lo}, {hi}) at confidence {c}");
                assert!(
                    r >= FusedEstimator::MIN_VARIANCE_PX2,
                    "r = {r} below the floor for ({lo}, {hi}) at confidence {c}"
                );
            }
        }
    }
}
