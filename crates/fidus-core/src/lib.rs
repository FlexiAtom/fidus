//! # fidus-core — the Zero-Trust Coordinate core
//!
//! fidus is a positioning engine for environments where platform window-coordinate
//! APIs cannot be trusted (Wayland compositors being the canonical case). It builds
//! its own coordinate system anchored to physical screen geometry instead of
//! relying on any native coordinate value.
//!
//! ## The five principles (spec v0.5.1, Appendix A)
//!
//! 1. **Trust no platform API.** No coordinate returned by any platform API ever
//!    becomes part of fidus' state. This is enforced at the type level: no public
//!    API of this crate accepts a native coordinate as input.
//! 2. **Build your own map.** Calibration projects known markers through basic
//!    compositor primitives and solves a mapping from the captured pixels.
//! 3. **Bootstrap from primitives.** Only the most basic, universal compositor
//!    primitives are used: `wl_surface` / `wl_shm` / layer-shell, basic X11
//!    drawing, Windows GDI, and generic screen capture.
//! 4. **Keep the pool pure.** The probability pool only receives visual and
//!    interaction measurements — never native coordinates, which are a different
//!    quantity entirely (boolean "success" vs. probabilistic confidence).
//! 5. **Calibrate, then get out.** The calibration overlay surface is destroyed
//!    immediately after the coordinate frame is solved; nothing stays resident.
//!
//! ## Crate layout (spec §7)
//!
//! * `fidus-core` (this crate): type system, the three-layer traits
//!   ([`Calibrator`]/[`Estimator`]/[`Gate`]), the calibration I/O session
//!   abstraction, and the [`FallbackEngine`] shell.
//! * `fidus-backend-*`: adapters for the basic compositor primitives only
//!   (projection + capture). They never wrap "read window coordinates" APIs.
//! * `fidus-calibrate`: L0/L9/L10 calibration strategies.
//! * `fidus-estimate`: steady-state incremental estimators (L1/L8 + constant-velocity Kalman).
//! * `fidus-extras` (planned): business-specific items (L3 Beacon, L5 TUIScan).
//!
//! ## Coordinate semantics
//!
//! A successful calibration yields a [`CoordinateFrame`]: an affine map between
//!
//! * *logical* space — layer-shell usable-area coordinates of the calibrated
//!   output (the same space layer-shell margins are expressed in), and
//! * *physical* space — pixel coordinates inside fidus' own screen captures.
//!
//! Both sides are fidus' own quantities: the logical side is anchored to marker
//! margins fidus itself chose, the physical side to pixels fidus itself captured.
//! No platform-provided coordinate participates anywhere.

#![warn(missing_docs)]

pub mod calibration;
pub mod coord;
pub mod engine;
pub mod env;
pub mod estimate;
pub mod frame;
pub mod gate;
pub mod io;
pub mod target;

/// Convenient re-exports of the primary public items.
pub mod prelude {
    pub use crate::calibration::{CalibrationError, CalibrationMethod, CalibrationStatus, UnsupportedReason};
    pub use crate::coord::{AffineTransform, BoundingBox, LogicalPoint, PhysicalPoint, Residuals, SolveError};
    pub use crate::engine::{EngineParts, FallbackEngine, InitError};
    pub use crate::env::{CompositorKind, EnvironmentContext, PermissionState, PermissionType};
    pub use crate::estimate::{EstimateError, MeasurementSource, ProbabilisticPosition};
    pub use crate::frame::{CalibrationQuality, CoordinateFrame};
    pub use crate::gate::{Gate, ProbeGate};
    pub use crate::io::{
        CalibrationIo, CaptureError, CaptureIo, Frame, IoFactory, MarkerError, MarkerShape, MarkerStyle,
        PixelFormat,
    };
    pub use crate::target::{RgbaImage, TargetDescription, UntrackablePolicy};
}
