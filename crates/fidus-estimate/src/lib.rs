//! C-layer estimators for fidus.
//!
//! # Status (spec §8 priorities)
//!
//! This crate currently ships the trait plumbing only. The steady-state
//! estimators are the P2 roadmap item:
//!
//! * **L1/L6 Fingerprint** — template/fingerprint matching of the target
//!   against captures (the caller describes *what* to locate; fidus only
//!   ever measures pixels).
//! * **L8 EdgeSync** — low-frequency edge differencing of the target region,
//!   gated by the MotionGate (500 ms frame-difference window; dynamic
//!   content rate `R > max(baseline·3, 15%)` discards the frame).
//! * **L4 Relative displacement** — motion model for drags.
//! * **L7 fusion** — EKF over the whitelisted measurements only
//!   (spec §6.1).
//!
//! Until then, [`NullEstimator`] reports `NotImplementedYet` honestly instead
//! of pretending to track.

use fidus_core::engine::Estimator;
use fidus_core::estimate::{EstimateError, ProbabilisticPosition};
use fidus_core::frame::CoordinateFrame;
use fidus_core::io::CaptureIo;

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
