//! fidus-calibrate — B-layer calibration strategies (spec §4).
//!
//! Available strategies:
//!
//! * [`CrosshairCalibrator`] (L9, flagship): projects markers at known
//!   layer-shell margins and solves the logical→physical affine map by
//!   least squares, with verification rounds and independent-pass
//!   consistency checks.
//! * [`AnchorCalibrator`] (L0, universal fallback): four color sentinels at
//!   the usable-area corners, shuffled color→corner mapping, two-pass
//!   consistency plus the rectangle constraint.
//! * L10 GradientField: **rejected**, see spec §4.2. The log-spiral grid and
//!   the FFT-fundamental lock it was built on are mutually exclusive, and the
//!   phase it would solve from is ambiguous modulo the grating period. The
//!   `gradient-field` feature remains only so the flag does not break builds
//!   that already set it; enabling it adds nothing.
//!
//! Every strategy works strictly through [`fidus_core::io::CalibrationIo`]:
//! project a marker, capture the screen, measure. No native coordinates are
//! involved anywhere.

#![warn(missing_docs)]

pub mod anchor;
pub mod crosshair;
pub mod detect;
pub mod rng;

pub use anchor::{AnchorCalibrator, AnchorConfig, SENTINEL_COLORS};
pub use crosshair::{CrosshairCalibrator, CrosshairConfig};
pub use detect::{DetectConfig, DetectError, Detection};
pub use rng::Rng;
