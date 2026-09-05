//! The three-layer traits and the [`FallbackEngine`] shell (spec §3, §6).

use crate::calibration::{CalibrationError, CalibrationMethod};
use crate::env::EnvironmentContext;
use crate::estimate::{EstimateError, ProbabilisticPosition};
use crate::target::TargetDescription;
use crate::frame::CoordinateFrame;
use crate::gate::Gate;
use crate::io::{CalibrationIo, CaptureIo, IoFactory};

/// B layer — calibrator (spec §3.1).
///
/// Establishes a [`CoordinateFrame`] once, at initialization or periodic
/// recalibration, and is then discarded. Implementations must project
/// markers and detect them strictly through the provided [`CalibrationIo`]
/// session, and must leave the session's projector destroyed when they
/// return (spec §4.4 teardown rule).
pub trait Calibrator: Send {
    /// Which method this calibrator implements.
    fn method(&self) -> CalibrationMethod;

    /// Runs the calibration and returns the solved frame.
    fn calibrate(
        &mut self,
        io: &mut dyn CalibrationIo,
    ) -> Result<CoordinateFrame, CalibrationError>;
}

/// C layer — estimator (spec §3.2).
///
/// Performs incremental tracking in steady state, using only visual and
/// interaction measurements (spec §6.1 whitelist) taken through a
/// capture-only [`CaptureIo`] session — it structurally cannot project
/// markers — in the coordinates of the active frame.
pub trait Estimator: Send {
    /// Produces the next position estimate for the tracked target.
    fn estimate(
        &mut self,
        io: &mut dyn CaptureIo,
        frame: &CoordinateFrame,
    ) -> Result<ProbabilisticPosition, EstimateError>;

    /// Registers the target to track (spec §3.2: the caller describes *what*
    /// to locate; fidus only ever measures pixels). Replaces any previously
    /// registered target.
    ///
    /// The default implementation refuses: estimators without tracking
    /// support reject targets honestly instead of silently ignoring them.
    fn register_target(&mut self, _target: TargetDescription) -> Result<(), EstimateError> {
        Err(EstimateError::NotImplementedYet {
            note: "this estimator does not accept tracking targets",
        })
    }
}

/// Backend-level error surfaced when opening sessions or probing.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// The backend could not connect to the display server.
    #[error("no usable display: {0}")]
    NoDisplay(String),
    /// The environment lacks the primitives this backend needs.
    #[error("environment lacks required primitives: {0}")]
    ProbeFailed(String),
    /// Anything else.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Assembled parts for [`FallbackEngine::new`]; built by platform glue such
/// as the `fidus` umbrella crate.
pub struct EngineParts {
    /// Gate answering availability queries.
    pub gate: Box<dyn Gate>,
    /// The calibration strategy to use. `None` until a backend+strategy pair
    /// is available.
    pub calibrator: Option<Box<dyn Calibrator>>,
    /// The steady-state estimator.
    pub estimator: Box<dyn Estimator>,
    /// Factory providing capture/projection sessions.
    pub io_factory: Box<dyn IoFactory>,
}

/// The engine shell from spec §6.2.
///
/// The zero-trust rule is structural here: [`EnvironmentContext`] carries no
/// coordinates, [`CalibrationIo`] accepts no coordinates, and no method of
/// this type accepts a coordinate — there is simply no pathway for a native
/// coordinate to reach the probability pool.
pub struct FallbackEngine {
    gate: Box<dyn Gate>,
    calibrator: Option<Box<dyn Calibrator>>,
    estimator: Box<dyn Estimator>,
    io_factory: Box<dyn IoFactory>,
    frame: Option<CoordinateFrame>,
}

impl FallbackEngine {
    /// Assembles an engine from its parts.
    pub fn new(parts: EngineParts) -> Self {
        Self {
            gate: parts.gate,
            calibrator: parts.calibrator,
            estimator: parts.estimator,
            io_factory: parts.io_factory,
            frame: None,
        }
    }

    /// Availability queries (spec §5.4): callers must check before
    /// calibrating.
    pub fn gate(&self) -> &dyn Gate {
        self.gate.as_ref()
    }

    /// The environment the gate answers about.
    pub fn environment(&self) -> &EnvironmentContext {
        self.gate.environment()
    }

    /// Runs calibration and stores the resulting frame.
    ///
    /// The overlay surface is guaranteed to be destroyed when this returns,
    /// on success or failure alike (spec §4.4).
    pub fn calibrate(&mut self) -> Result<&CoordinateFrame, CalibrationError> {
        let frame = {
            let calibrator = self
                .calibrator
                .as_mut()
                .ok_or(CalibrationError::NoCalibrator("no calibrator configured"))?;
            let mut io = self.io_factory.open().map_err(|e| CalibrationError::Capture(
                crate::io::CaptureError::Backend(e.to_string()),
            ))?;
            calibrator.calibrate(io.as_mut())?
        };
        self.frame = Some(frame);
        Ok(self.frame.as_ref().expect("frame just stored"))
    }

    /// The active coordinate frame, if calibrated.
    pub fn frame(&self) -> Option<&CoordinateFrame> {
        self.frame.as_ref()
    }

    /// Forgets the active frame (e.g. after a hotplug event, spec §4.5).
    pub fn invalidate_frame(&mut self) {
        self.frame = None;
    }

    /// Produces the next position estimate for the tracked target.
    ///
    /// Runs on a capture-only session: no overlay surface exists during
    /// steady-state estimation (spec §4.4).
    pub fn estimate(&mut self) -> Result<ProbabilisticPosition, EstimateError> {
        let frame = self.frame.as_ref().ok_or(EstimateError::NotCalibrated)?;
        let mut io = self.io_factory.open_capture().map_err(|e| {
            EstimateError::Capture(crate::io::CaptureError::Backend(e.to_string()))
        })?;
        self.estimator.estimate(io.as_mut(), frame)
    }

    /// Registers the target the estimator should track (runtime
    /// registration; spec §3.2 / §6.1).
    ///
    /// Replaces any previously registered target. Returns
    /// [`EstimateError::NotImplementedYet`] when the assembled estimator
    /// does not support tracking.
    pub fn register_target(&mut self, target: TargetDescription) -> Result<(), EstimateError> {
        self.estimator.register_target(target)
    }
}
