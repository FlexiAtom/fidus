//! Engine assembly: probe → gate → calibrator/estimator selection.

use fidus_calibrate::{CrosshairCalibrator, CrosshairConfig};
use fidus_core::calibration::{CalibrationMethod, CalibrationStatus};
use fidus_core::engine::{EngineParts, FallbackEngine, InitError};
use fidus_core::env::EnvironmentContext;
use fidus_core::gate::ProbeGate;
use fidus_core::io::IoFactory;
use fidus_estimate::NullEstimator;

/// Which backend to assemble.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendChoice {
    /// Layer-shell backend (default; Niri / Sway / Hyprland / KDE / partial
    /// GNOME).
    #[default]
    WaylandLayer,
}

/// Builder for a [`FallbackEngine`], with caller-supplied environment
/// knowledge and calibrator tuning.
#[derive(Debug, Default)]
pub struct FidusBuilder {
    caller_env: EnvironmentContext,
    crosshair: Option<CrosshairConfig>,
}

impl FidusBuilder {
    /// A builder with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds caller knowledge about the environment (e.g. dynamic wallpaper).
    /// Probe results remain authoritative for capabilities.
    pub fn with_environment(mut self, caller: EnvironmentContext) -> Self {
        self.caller_env = caller;
        self
    }

    /// Overrides the L9 Crosshair configuration.
    pub fn with_crosshair_config(mut self, config: CrosshairConfig) -> Self {
        self.crosshair = Some(config);
        self
    }

    /// Connects to the backend, probes the environment, answers gating and
    /// assembles the engine.
    ///
    /// The engine is returned even when the gate answers "unavailable" —
    /// callers inspect [`FallbackEngine::gate`] themselves (spec §5.4).
    pub fn build(self) -> Result<FallbackEngine, InitError> {
        self.build_with(BackendChoice::default())
    }

    /// Like [`build`](Self::build) with an explicit backend choice.
    pub fn build_with(self, choice: BackendChoice) -> Result<FallbackEngine, InitError> {
        match choice {
            #[cfg(feature = "wayland-layer")]
            BackendChoice::WaylandLayer => self.build_wayland_layer(),
            #[cfg(not(feature = "wayland-layer"))]
            BackendChoice::WaylandLayer => Err(InitError::ProbeFailed(
                "compiled without the `wayland-layer` feature".into(),
            )),
        }
    }

    #[cfg(feature = "wayland-layer")]
    fn build_wayland_layer(self) -> Result<FallbackEngine, InitError> {
        use fidus_core::gate::Gate;

        let mut backend = fidus_backend_wayland_layer::WaylandLayerBackend::connect()?;
        if !backend.primitives_available() {
            return Err(InitError::ProbeFailed(
                "compositor lacks compositor/shm/layer-shell/screencopy globals".into(),
            ));
        }
        let probe = backend.probe_environment();
        let env = EnvironmentContext::merge(probe, self.caller_env);
        let gate = ProbeGate::from_environment(env);

        // Calibrator selection follows the gate: Crosshair needs layer-shell
        // (spec §3.1). The engine itself stays usable for gate queries.
        let calibrator = match gate.query_calibrator_availability(CalibrationMethod::Crosshair) {
            CalibrationStatus::Available { .. } | CalibrationStatus::Degraded { .. } => {
                Some(Box::new(CrosshairCalibrator::new(
                    self.crosshair.unwrap_or_default(),
                )) as Box<dyn fidus_core::engine::Calibrator>)
            }
            _ => None,
        };

        Ok(FallbackEngine::new(EngineParts {
            gate: Box::new(gate),
            calibrator,
            estimator: Box::new(NullEstimator),
            io_factory: Box::new(backend) as Box<dyn IoFactory>,
        }))
    }
}
