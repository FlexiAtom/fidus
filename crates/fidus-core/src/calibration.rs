//! Calibration methods, statuses and errors (spec §5.1).

use crate::env::PermissionType;
use crate::io::{CaptureError, MarkerError};
use crate::coord::SolveError;

/// A calibration strategy identified by its spec level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CalibrationMethod {
    /// L9 Crosshair — flagship calibrator, requires layer-shell (wlroots-like
    /// compositors: Niri / Sway / Hyprland, KDE, partial GNOME).
    Crosshair,
    /// L10 GradientField — experimental, feature-gated (`gradient-field`).
    GradientField,
    /// L0 Anchor — universal fallback based on corner markers, for
    /// environments without layer-shell (X11, Windows).
    Anchor,
}

impl CalibrationMethod {
    /// All methods, in spec order.
    pub const ALL: [CalibrationMethod; 3] =
        [CalibrationMethod::Crosshair, CalibrationMethod::GradientField, CalibrationMethod::Anchor];

    /// Human-readable name, e.g. `"L9 Crosshair"`.
    pub fn name(self) -> &'static str {
        match self {
            CalibrationMethod::Crosshair => "L9 Crosshair",
            CalibrationMethod::GradientField => "L10 GradientField",
            CalibrationMethod::Anchor => "L0 Anchor",
        }
    }
}

/// Why a calibration method is not usable in the current environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedReason {
    /// A required Wayland/global protocol is absent.
    MissingProtocol {
        /// Wire name of the missing protocol/interface, e.g.
        /// `"zwlr_layer_shell_v1"`.
        protocol: &'static str,
    },
    /// The method is specified but not implemented yet (honest roadmapping,
    /// spec §10).
    NotImplementedYet {
        /// Short note about when it is planned.
        note: &'static str,
    },
    /// The method exists but its compile-time feature is disabled.
    FeatureDisabled {
        /// Name of the cargo feature that enables it.
        feature: &'static str,
    },
    /// The method only supports a single output and the environment has more.
    MultiOutputUnsupported {
        /// Number of outputs detected.
        outputs: usize,
    },
    /// No usable display connection.
    NoDisplay,
}

impl UnsupportedReason {
    /// Short human-readable summary for UI passthrough.
    pub fn summary(&self) -> String {
        match self {
            UnsupportedReason::MissingProtocol { protocol } => {
                format!("missing protocol {protocol}")
            }
            UnsupportedReason::NotImplementedYet { note } => format!("not implemented yet: {note}"),
            UnsupportedReason::FeatureDisabled { feature } => {
                format!("disabled, enable cargo feature `{feature}`")
            }
            UnsupportedReason::MultiOutputUnsupported { outputs } => {
                format!("{outputs} outputs detected, only single-output is supported")
            }
            UnsupportedReason::NoDisplay => "no usable display".to_string(),
        }
    }
}

/// Availability of a calibration method, as answered by [`crate::gate::Gate`].
#[derive(Clone, Debug, PartialEq)]
pub enum CalibrationStatus {
    /// Usable, no precision degradation expected.
    Available {
        /// The method this status refers to.
        method: CalibrationMethod,
    },
    /// Platform / environment does not support it.
    NotSupported {
        /// The method this status refers to.
        method: CalibrationMethod,
        /// Why it is not supported.
        reason: UnsupportedReason,
    },
    /// A permission must be granted first (e.g. screen recording on macOS).
    PermissionRequired {
        /// The method this status refers to.
        method: CalibrationMethod,
        /// The permission to request.
        permission: PermissionType,
    },
    /// Usable but environmentally constrained (e.g. multiple outputs, dynamic
    /// wallpaper); calibration can proceed with reduced expected confidence.
    Degraded {
        /// The method this status refers to.
        method: CalibrationMethod,
        /// Rough expected confidence of the resulting frame, in `[0, 1]`.
        estimated_confidence: f32,
    },
}

impl CalibrationStatus {
    /// The method this status refers to.
    pub fn method(&self) -> CalibrationMethod {
        match self {
            CalibrationStatus::Available { method }
            | CalibrationStatus::NotSupported { method, .. }
            | CalibrationStatus::PermissionRequired { method, .. }
            | CalibrationStatus::Degraded { method, .. } => *method,
        }
    }

    /// `true` when calibration may proceed ([`Available`](Self::Available) or
    /// [`Degraded`](Self::Degraded)).
    pub fn is_usable(&self) -> bool {
        matches!(
            self,
            CalibrationStatus::Available { .. } | CalibrationStatus::Degraded { .. }
        )
    }
}

/// Errors raised while running a calibration.
#[derive(Debug, thiserror::Error)]
pub enum CalibrationError {
    /// The calibrated output's usable area is too small to place markers in.
    #[error("usable area ({size}) is too small to calibrate in")]
    UsableAreaTooSmall {
        /// Human-readable usable-area size.
        size: String,
    },
    /// The method is specified but its implementation is not available.
    #[error("calibration method not implemented yet: {note}")]
    NotImplementedYet {
        /// Roadmap note.
        note: &'static str,
    },
    /// The projector surface was closed by the compositor mid-calibration.
    #[error("calibration overlay was closed by the compositor")]
    ProjectorClosed,
    /// A marker could not be located in the capture after all retries.
    #[error("marker detection failed after {attempts} attempts at {position}")]
    DetectionFailed {
        /// Number of attempts made.
        attempts: usize,
        /// Logical position of the last attempt.
        position: String,
    },
    /// Two independent calibration passes disagree beyond tolerance.
    #[error("independent calibration passes disagree: {detail}")]
    Inconsistent {
        /// Human-readable description of the disagreement.
        detail: String,
    },
    /// Residual or verification error exceeded the configured tolerance.
    #[error("calibration accuracy below threshold: {measured:.3} px > {tolerance:.3} px ({stage})")]
    AccuracyBelowThreshold {
        /// Measured error in pixels.
        measured: f64,
        /// Configured tolerance in pixels.
        tolerance: f64,
        /// Which stage failed (`residual`, `verification`, `consistency`,
        /// `transform sanity`).
        stage: &'static str,
    },
    /// Affine solve failed.
    #[error(transparent)]
    Solve(#[from] SolveError),
    /// Screen capture failed.
    #[error(transparent)]
    Capture(#[from] CaptureError),
    /// Marker projection failed.
    #[error(transparent)]
    Marker(#[from] MarkerError),
    /// The backend has no calibrator installed for this method.
    #[error("no calibrator installed for {0}")]
    NoCalibrator(&'static str),
}
