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
    /// Measurement variance at confidence 1.0, px².
    pub r_min_px2: f64,
    /// Measurement variance at confidence 0.0, px².
    pub r_max_px2: f64,
    /// Extra search margin around the predicted region, in capture pixels.
    pub search_margin_px: f64,
    /// Search-window widening per consecutive miss, in capture pixels.
    pub widen_per_miss_px: f64,
}

impl FusedEstimator {
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
            r_min_px2: 1.0,
            r_max_px2: 400.0,
            search_margin_px: 48.0,
            widen_per_miss_px: 64.0,
        }
    }

    /// Replaces the clock. Production code never needs this; deterministic
    /// callers (tests, replay harnesses) do.
    pub fn set_clock(&mut self, clock: Box<dyn Fn() -> Instant + Send>) {
        self.clock = clock;
    }

    /// Measurement variance derived from confidence.
    fn r_of(confidence: f32) -> f64 {
        let c = confidence.clamp(0.0, 1.0) as f64;
        1.0 + (1.0 - c) * 399.0
    }

    /// Half-extent of the search window around a predicted center.
    fn search_half(&self, tpl: &fidus_core::target::RgbaImage) -> f64 {
        let base = tpl.width.max(tpl.height) as f64 * 1.5;
        base + self.search_margin_px + self.widen_per_miss_px * self.lost_streak as f64
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
            self.track.update(position_logical, Self::r_of(confidence));
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
                .unwrap_or_else(|| bbox_around(pred, tpl.width as f64, tpl.height as f64));
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
                    let (tw, th) = (tpl.width.max(tpl.height) as f64, tpl.height as f64);
                    (
                        PhysicalPoint::new(fw as f64 / 2.0, fh as f64 / 2.0),
                        fw.max(fh) as f64 + tw.max(th),
                    )
                }
            };
            l1_match(&img, frame, &tpl, SearchRoi { center: (center.x, center.y), half })
        };

        // 4.–6. Fuse and assimilate.
        match (l1, l8) {
            (Some(a), Some(b)) => {
                let agree = a.center_physical.distance(b.0) <= AGREE_PX;
                let (pos, conf, bbox) = if agree {
                    // Corroborated: keep L1's subpixel position, boost the
                    // confidence.
                    (a.position_logical, (a.confidence + CORROBORATION_BONUS).min(1.0), a.bbox_physical)
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
