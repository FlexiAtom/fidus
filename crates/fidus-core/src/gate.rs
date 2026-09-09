//! Gate — the "can fidus be used here?" programmable interface (spec §5).

use crate::calibration::{CalibrationMethod, CalibrationStatus, UnsupportedReason};
use crate::env::{EnvironmentContext, PermissionState, PermissionType};

/// Capability probe, a first-class part of fidus' public API (spec §3.3).
///
/// Callers *must* query before calibrating: `query_calibrator_availability`
/// answers which methods can run and, when they cannot, why — so the caller
/// can drive UI (permission prompts, fallbacks) instead of fidus.
pub trait Gate: Send {
    /// The environment this gate describes.
    fn environment(&self) -> &EnvironmentContext;

    /// Answers whether `method` can currently be calibrated with.
    fn query_calibrator_availability(&self, method: CalibrationMethod) -> CalibrationStatus;
}

/// The default, platform-agnostic [`Gate`]: a pure function of
/// [`EnvironmentContext`].
///
/// Backends probe the live environment to build the context; policy decisions
/// then need no live connection, which keeps the gate cheap and testable.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeGate {
    env: EnvironmentContext,
}

impl ProbeGate {
    /// Builds a gate over a probed environment context.
    pub fn from_environment(env: EnvironmentContext) -> Self {
        Self { env }
    }
}

impl Gate for ProbeGate {
    fn environment(&self) -> &EnvironmentContext {
        &self.env
    }

    fn query_calibrator_availability(&self, method: CalibrationMethod) -> CalibrationStatus {
        match method {
            CalibrationMethod::Crosshair => self.crosshair_status(),
            CalibrationMethod::GradientField => CalibrationStatus::NotSupported {
                method,
                reason: UnsupportedReason::FeatureDisabled { feature: "gradient-field" },
            },
            CalibrationMethod::Anchor => self.anchor_status(),
        }
    }
}

impl ProbeGate {
    fn crosshair_status(&self) -> CalibrationStatus {
        let method = CalibrationMethod::Crosshair;
        if !self.env.has_layer_shell {
            return CalibrationStatus::NotSupported {
                method,
                reason: UnsupportedReason::MissingProtocol { protocol: "zwlr_layer_shell_v1" },
            };
        }
        if self.env.multi_monitor_count == 0 {
            return CalibrationStatus::NotSupported { method, reason: UnsupportedReason::NoDisplay };
        }
        match self.env.screen_capture_permission {
            PermissionState::Granted | PermissionState::Unknown => {}
            PermissionState::Requesting | PermissionState::Revoked | PermissionState::RequiresRestart => {
                return CalibrationStatus::PermissionRequired {
                    method,
                    permission: PermissionType::ScreenCapture,
                };
            }
        }
        if self.env.multi_monitor_count > 1 {
            // Spec §10 open question 4: multi-monitor CoordinateFrame is not
            // settled; v0.1 calibrates the primary output honestly.
            CalibrationStatus::Degraded { method, estimated_confidence: 0.75 }
        } else {
            CalibrationStatus::Available { method }
        }
    }

    /// L0 Anchor is the universal fallback: it asks for nothing but the two
    /// primitives every backend adapts (project, capture) plus the ability
    /// to show four markers at once. Platform identity never enters the
    /// decision — a backend that provides the primitives qualifies.
    fn anchor_status(&self) -> CalibrationStatus {
        let method = CalibrationMethod::Anchor;
        if !self.env.multi_marker_projection {
            return CalibrationStatus::NotSupported {
                method,
                reason: UnsupportedReason::MissingPrimitive { primitive: "multi-marker projection" },
            };
        }
        if self.env.multi_monitor_count == 0 {
            return CalibrationStatus::NotSupported { method, reason: UnsupportedReason::NoDisplay };
        }
        match self.env.screen_capture_permission {
            PermissionState::Granted | PermissionState::Unknown => {}
            PermissionState::Requesting | PermissionState::Revoked | PermissionState::RequiresRestart => {
                return CalibrationStatus::PermissionRequired {
                    method,
                    permission: PermissionType::ScreenCapture,
                };
            }
        }
        // Color segmentation without a topmost overlay guarantee is weaker
        // than L9's difference-based isolation; dynamic wallpaper is the
        // known adversary (baseline differencing still applies, but the
        // background may change between baseline and capture).
        let dynamic = self.env.is_dynamic_wallpaper == Some(true);
        if self.env.multi_monitor_count > 1 {
            CalibrationStatus::Degraded { method, estimated_confidence: if dynamic { 0.5 } else { 0.7 } }
        } else if dynamic {
            CalibrationStatus::Degraded { method, estimated_confidence: 0.6 }
        } else {
            CalibrationStatus::Available { method }
        }
    }
}
