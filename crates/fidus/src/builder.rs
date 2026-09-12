// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Engine assembly: probe → gate → calibrator/estimator selection.

use fidus_calibrate::{AnchorCalibrator, AnchorConfig, CrosshairCalibrator, CrosshairConfig};
use fidus_core::calibration::CalibrationMethod;
use fidus_core::engine::{Calibrator, EngineParts, FidusEngine, InitError};
use fidus_core::env::EnvironmentContext;
use fidus_core::gate::{Gate, ProbeGate};
use fidus_core::io::IoFactory;
use fidus_estimate::{FusedEstimator, ScreenClassifier};

/// Which backend to assemble.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BackendChoice {
    /// Try the compiled-in backends in preference order and keep the first
    /// whose primitives are all available: layer-shell first (its
    /// calibrator is the flagship), then X11.
    #[default]
    Auto,
    /// Layer-shell backend (Niri / Sway / Hyprland / KDE / partial GNOME).
    WaylandLayer,
    /// X11 backend (real X servers; XWayland only when its root is
    /// capturable).
    X11,
}

/// Builder for a [`FidusEngine`], with caller-supplied environment
/// knowledge and calibrator tuning.
#[derive(Debug)]
pub struct FidusBuilder {
    caller_env: EnvironmentContext,
    crosshair: Option<CrosshairConfig>,
    anchor: Option<AnchorConfig>,
    preferred: Option<CalibrationMethod>,
    /// Automatic dynamic-wallpaper classification at build time (two
    /// captures ~200 ms apart). `None` disables it.
    classifier: Option<ScreenClassifier>,
}

impl Default for FidusBuilder {
    fn default() -> Self {
        Self {
            caller_env: EnvironmentContext::default(),
            crosshair: None,
            anchor: None,
            preferred: None,
            classifier: Some(ScreenClassifier::default()),
        }
    }
}

impl FidusBuilder {
    /// A builder with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds caller knowledge about the environment (e.g. dynamic wallpaper).
    /// Probe results remain authoritative for capabilities; a caller-supplied
    /// `is_dynamic_wallpaper` skips the automatic classification.
    pub fn with_environment(mut self, caller: EnvironmentContext) -> Self {
        self.caller_env = caller;
        self
    }

    /// Tunes — or with `None` disables — the build-time dynamic-wallpaper
    /// classification (adds ~200 ms and two captures to `build`).
    pub fn with_screen_classifier(mut self, classifier: Option<ScreenClassifier>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Overrides the L9 Crosshair configuration.
    pub fn with_crosshair_config(mut self, config: CrosshairConfig) -> Self {
        self.crosshair = Some(config);
        self
    }

    /// Overrides the L0 Anchor configuration.
    pub fn with_anchor_config(mut self, config: AnchorConfig) -> Self {
        self.anchor = Some(config);
        self
    }

    /// Prefers a calibration method. The gate still decides: a preferred
    /// method that is unavailable falls through to the default order
    /// (Crosshair, then Anchor).
    pub fn prefer_method(mut self, method: CalibrationMethod) -> Self {
        self.preferred = Some(method);
        self
    }

    /// Connects to a backend, probes the environment, answers gating and
    /// assembles the engine.
    ///
    /// The engine is returned even when the gate answers "unavailable" for
    /// every calibrator — callers inspect [`FidusEngine::gate`]
    /// themselves (spec §5.4). Only a missing backend is an error.
    pub fn build(self) -> Result<FidusEngine, InitError> {
        self.build_with(BackendChoice::default())
    }

    /// Like [`build`](Self::build) with an explicit backend choice.
    pub fn build_with(self, choice: BackendChoice) -> Result<FidusEngine, InitError> {
        match choice {
            BackendChoice::Auto => self.build_auto(),
            BackendChoice::WaylandLayer => self.build_wayland_layer(),
            BackendChoice::X11 => self.build_x11(),
        }
    }

    fn build_auto(self) -> Result<FidusEngine, InitError> {
        let mut failures: Vec<String> = Vec::new();
        #[cfg(feature = "wayland-layer")]
        match Self::connect_wayland_layer() {
            Ok(backend) => return self.assemble(backend),
            Err(e) => failures.push(format!("wayland-layer: {e}")),
        }
        #[cfg(feature = "x11")]
        match Self::connect_x11() {
            Ok(backend) => return self.assemble(backend),
            Err(e) => failures.push(format!("x11: {e}")),
        }
        if failures.is_empty() {
            failures.push("no backend compiled in (enable `wayland-layer` and/or `x11`)".into());
        }
        Err(InitError::ProbeFailed(failures.join("; ")))
    }

    fn build_wayland_layer(self) -> Result<FidusEngine, InitError> {
        #[cfg(feature = "wayland-layer")]
        {
            let backend = Self::connect_wayland_layer()?;
            self.assemble(backend)
        }
        #[cfg(not(feature = "wayland-layer"))]
        Err(InitError::ProbeFailed("compiled without the `wayland-layer` feature".into()))
    }

    fn build_x11(self) -> Result<FidusEngine, InitError> {
        #[cfg(feature = "x11")]
        {
            let backend = Self::connect_x11()?;
            self.assemble(backend)
        }
        #[cfg(not(feature = "x11"))]
        Err(InitError::ProbeFailed("compiled without the `x11` feature".into()))
    }

    #[cfg(feature = "wayland-layer")]
    fn connect_wayland_layer() -> Result<ProbedBackend, InitError> {
        let mut backend = fidus_backend_wayland_layer::WaylandLayerBackend::connect()?;
        if !backend.primitives_available() {
            return Err(InitError::ProbeFailed(
                "compositor lacks compositor/shm/layer-shell/screencopy globals".into(),
            ));
        }
        let env = backend.probe_environment();
        Ok(ProbedBackend { env, io_factory: Box::new(backend) })
    }

    #[cfg(feature = "x11")]
    fn connect_x11() -> Result<ProbedBackend, InitError> {
        let backend = fidus_backend_x11::X11Backend::connect()?;
        if !backend.primitives_available() {
            return Err(InitError::ProbeFailed(match backend.capture_probe_error() {
                Some(e) => format!("root window is not capturable (rootless XWayland?): {e}"),
                None => "X screen lacks a usable visual".into(),
            }));
        }
        let env = backend.probe_environment();
        Ok(ProbedBackend { env, io_factory: Box::new(backend) })
    }

    /// Gate → calibrator selection → engine.
    fn assemble(self, mut backend: ProbedBackend) -> Result<FidusEngine, InitError> {
        let mut env = EnvironmentContext::merge(backend.env, self.caller_env);

        // Automatic dynamic-wallpaper knowledge (v0.4 L8 ScreenClassifier):
        // only when nobody knows better, and only if a capture-only session
        // opens — a failure here is not fatal, the field just stays `None`.
        if env.is_dynamic_wallpaper.is_none() {
            if let Some(classifier) = self.classifier {
                if let Ok(mut io) = backend.io_factory.open_capture() {
                    if let Ok(v) = classifier.classify_blocking(io.as_mut()) {
                        env.is_dynamic_wallpaper = Some(v.dynamic);
                    }
                }
            }
        }
        let gate = ProbeGate::from_environment(env);

        // Calibrator selection follows the gate, in preference order. The
        // engine stays usable for gate queries even when nothing qualifies.
        let order = [CalibrationMethod::Crosshair, CalibrationMethod::Anchor];
        let candidates = self.preferred.into_iter().chain(order);
        let mut calibrator: Option<Box<dyn Calibrator>> = None;
        for method in candidates {
            if !gate.query_calibrator_availability(method).is_usable() {
                continue;
            }
            calibrator = match method {
                CalibrationMethod::Crosshair => Some(Box::new(CrosshairCalibrator::new(
                    self.crosshair.clone().unwrap_or_default(),
                ))),
                CalibrationMethod::Anchor => Some(Box::new(AnchorCalibrator::new(
                    self.anchor.clone().unwrap_or_default(),
                ))),
                CalibrationMethod::GradientField => None,
            };
            if calibrator.is_some() {
                break;
            }
        }

        Ok(FidusEngine::new(EngineParts {
            gate: Box::new(gate),
            calibrator,
            // L7 fusion: L1 template matching + L8 gated differencing into a
            // constant-velocity track (P2-c).
            estimator: Box::new(FusedEstimator::new()),
            io_factory: backend.io_factory,
        }))
    }
}

/// A connected backend together with its probed environment.
struct ProbedBackend {
    env: EnvironmentContext,
    io_factory: Box<dyn IoFactory>,
}
