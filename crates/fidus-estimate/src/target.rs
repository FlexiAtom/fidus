//! Target description: the caller tells fidus *what* to locate, never where
//! a platform thinks it is.

use fidus_core::coord::LogicalPoint;

use crate::image::RgbaImage;

/// What the estimator should track.
///
/// The template is the caller's **own offscreen render** of the target — a
/// pure visual input, exactly the kind of measurement the probability pool
/// accepts (spec §6.1). Nothing here reads or wraps a platform coordinate.
#[derive(Clone, Debug)]
pub struct TargetDescription {
    /// Target appearance in **logical pixels** (the caller renders its window
    /// at logical size; the estimator resamples to the capture scale using
    /// the calibrated frame).
    pub template_logical: RgbaImage,
    /// Where the caller believes it placed the target (center, logical
    /// coordinates of the calibrated frame). `None` means "search the whole
    /// screen on the first estimate".
    ///
    /// This is the caller's own belief about its own drawing — it is not, and
    /// cannot be, a value read from a platform window API.
    pub initial_center: Option<LogicalPoint>,
}
