//! fidus-calibrate — B-layer calibration strategies (spec §4).
//!
//! Available strategies:
//!
//! * [`CrosshairCalibrator`] (L9, flagship): projects markers at known
//!   layer-shell margins and solves the logical→physical affine map by
//!   least squares, with verification rounds and independent-pass
//!   consistency checks.
//! * L0 Anchor: planned for the P2 phase (X11/Windows fallback).
//! * L10 GradientField: experimental, feature-gated behind
//!   `gradient-field` and **not implemented** — it stays a research item
//!   per spec §4.2.
//!
//! Every strategy works strictly through [`fidus_core::io::CalibrationIo`]:
//! project a marker, capture the screen, measure. No native coordinates are
//! involved anywhere.

#![warn(missing_docs)]
pub mod crosshair;
pub mod detect;
pub mod rng;

pub use crosshair::{CrosshairCalibrator, CrosshairConfig};
pub use detect::{DetectConfig, DetectError, Detection};
