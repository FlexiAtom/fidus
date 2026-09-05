//! Estimator outputs and errors (C layer, spec §3.2 / §6).

use std::time::Instant;

use crate::coord::{BoundingBox, LogicalPoint};
use crate::io::CaptureError;

/// Which measurement source produced a position estimate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementSource {
    /// A single named layer (e.g. `"L1 Fingerprint"`, `"L8 EdgeSync"`).
    Single(&'static str),
    /// Fused from several layers (L7).
    Fusion,
}

/// A position estimate with attached confidence.
///
/// `position` lives in the calibrated logical space of the active
/// [`crate::frame::CoordinateFrame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbabilisticPosition {
    /// Estimated position in calibrated logical coordinates.
    pub position: LogicalPoint,
    /// Confidence in `[0, 1]`. Values below 0.4 sustained over time are the
    /// spec's trigger for recalibration (spec §4.5).
    pub confidence: f32,
    /// Measured bounding box of the target in physical pixels, when the
    /// source measurement provides one.
    pub bbox_physical: Option<BoundingBox>,
    /// When the underlying measurement was taken.
    pub measured_at: Instant,
    /// Which layer produced this estimate.
    pub source: MeasurementSource,
}

impl ProbabilisticPosition {
    /// Convenience constructor for a position-only estimate.
    pub fn new(position: LogicalPoint, confidence: f32, source: MeasurementSource) -> Self {
        Self { position, confidence, bbox_physical: None, measured_at: Instant::now(), source }
    }
}

/// Errors from [`crate::engine::FallbackEngine::estimate`].
#[derive(Debug, thiserror::Error)]
pub enum EstimateError {
    /// No calibrated coordinate frame exists yet; call `calibrate()` first.
    #[error("not calibrated: call calibrate() first")]
    NotCalibrated,
    /// The configured estimator does not provide a target to track.
    #[error("no tracking target has been registered")]
    NoTarget,
    /// The estimator implementation is not available in this build (P2
    /// roadmap item).
    #[error("estimator not implemented yet: {note}")]
    NotImplementedYet {
        /// Roadmap note.
        note: &'static str,
    },
    /// Capturing the screen for a fresh measurement failed.
    #[error(transparent)]
    Capture(#[from] CaptureError),
    /// The target could not be located and no prior position exists to
    /// report (first search failed). Honesty rule: the pool is never fed a
    /// fabricated position (spec §4.5).
    #[error("tracking target not found in capture")]
    TargetLost,
}

