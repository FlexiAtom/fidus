//! The calibrated coordinate frame (spec §4.4 "交给 C 层做增量追踪").

use std::time::SystemTime;

use crate::calibration::CalibrationMethod;
use crate::coord::{AffineTransform, LogicalPoint, PhysicalPoint, SolveError, SolvedMap};

/// Quality metrics of a solved [`CoordinateFrame`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalibrationQuality {
    /// Root-mean-square residual of the final affine fit, in pixels.
    pub rms_residual_px: f64,
    /// Largest residual of the final affine fit, in pixels.
    pub max_residual_px: f64,
    /// Largest error of the verification round (markers at fresh positions,
    /// predicted vs. detected), in pixels.
    pub verification_max_err_px: f64,
    /// Largest disagreement between independent calibration passes, in pixels.
    pub consistency_max_err_px: f64,
    /// Number of marker correspondences used per pass.
    pub sample_count: usize,
    /// Number of independent passes that had to agree.
    pub independent_passes: usize,
}

/// fidus' own coordinate system, produced by a successful calibration and
/// handed to the C layer (estimator) for incremental tracking.
///
/// The frame is built entirely from quantities fidus produced itself:
/// marker margins it chose and marker centroids it detected in its own
/// captures. Per the Q9 lesson (spec §4.4), everything is extracted *before*
/// the overlay surface is destroyed; the frame never references live state.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordinateFrame {
    map: AffineTransform,
    inverse: AffineTransform,
    capture_size: (u32, u32),
    method: CalibrationMethod,
    quality: CalibrationQuality,
    calibrated_at: SystemTime,
}

impl CoordinateFrame {
    /// Builds a frame from a map that was **solved from measurements**.
    ///
    /// Fails if the map is not invertible.
    ///
    /// # Why the parameter is [`SolvedMap`], not [`AffineTransform`]
    ///
    /// Principle 1 is enforced structurally rather than by review: the only
    /// source of a `SolvedMap` is
    /// [`AffineTransform::from_correspondences`], so every frame in existence
    /// traces back to detected marker centroids in fidus' own captures. There
    /// is no expressible way to declare a frame — not even the identity map,
    /// which would silently claim "capture pixels are logical pixels" and go
    /// unnoticed on any 1× single-output desktop.
    ///
    /// *Failure mode this closes*: the previous signature accepted any
    /// `AffineTransform`, and with `pub` coefficients that meant a platform
    /// rect could be spelled `AffineTransform { c: x, f: y, .. }` and handed
    /// in as a calibration result — the exact pool contamination spec §6.2
    /// says the compiler should prevent.
    pub fn new(
        solved: SolvedMap,
        capture_size: (u32, u32),
        method: CalibrationMethod,
        quality: CalibrationQuality,
        calibrated_at: SystemTime,
    ) -> Result<Self, SolveError> {
        let map = solved.map();
        let inverse = map.inverse()?;
        Ok(Self { map, inverse, capture_size, method, quality, calibrated_at })
    }

    /// Converts a logical (usable-area) point to capture-pixel space.
    pub fn logical_to_physical(&self, p: LogicalPoint) -> PhysicalPoint {
        self.map.apply(p)
    }

    /// Converts a capture-pixel point to logical (usable-area) space.
    pub fn physical_to_logical(&self, p: PhysicalPoint) -> LogicalPoint {
        self.inverse.apply_physical(p)
    }

    /// Dimensions of the captures the frame was solved from, in pixels.
    pub fn capture_size(&self) -> (u32, u32) {
        self.capture_size
    }

    /// The calibration method that produced this frame.
    pub fn method(&self) -> CalibrationMethod {
        self.method
    }

    /// Quality metrics of the calibration.
    pub fn quality(&self) -> &CalibrationQuality {
        &self.quality
    }

    /// When the calibration completed.
    pub fn calibrated_at(&self) -> SystemTime {
        self.calibrated_at
    }

    /// The underlying logical→physical map.
    pub fn map(&self) -> &AffineTransform {
        &self.map
    }
}
