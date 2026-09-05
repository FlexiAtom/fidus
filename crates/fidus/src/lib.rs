//! # fidus — Zero-Trust Coordinate positioning engine
//!
//! fidus locates things on screen for environments where platform
//! window-coordinate APIs cannot be trusted (Wayland compositors are the
//! canonical case). It never wraps a native coordinate API: it projects its
//! own markers through basic compositor primitives, captures the screen, and
//! solves its own coordinate frame from those measurements. *What* to locate
//! is the caller's business; fidus provides the map.
//!
//! ## Usage
//!
//! ```no_run
//! # fn main() -> Result<(), fidus::InitError> {
//! use fidus::prelude::*;
//!
//! let mut engine = FidusBuilder::new().build()?;
//!
//! // 1. Query before calibrating (Gate, spec §5).
//! match engine.gate().query_calibrator_availability(CalibrationMethod::Crosshair) {
//!     CalibrationStatus::Available { .. } | CalibrationStatus::Degraded { .. } => {}
//!     status => {
//!         eprintln!("fidus unavailable here: {status:?}");
//!         return Ok(());
//!     }
//! }
//!
//! // 2. Calibrate once; the overlay is destroyed when this returns.
//! let frame = engine.calibrate().expect("calibration failed");
//! println!(
//!     "scale {:.3}, residuals {:.3} px",
//!     frame.map().linear_scale(),
//!     frame.quality().rms_residual_px
//! );
//!
//! // 3. Register what to track: the caller's own offscreen render
//! //    (pure pixels — fidus never accepts a platform coordinate).
//! let target = TargetDescription {
//!     template_logical: RgbaImage::from_raw(4, 4, vec![0; 4 * 4 * 4]),
//!     initial_center: None,
//! };
//! engine.register_target(target).expect("estimator accepts targets");
//!
//! // 4. Steady-state tracking (L1 Fingerprint, wired in P2-a).
//! let _ = engine.estimate();
//! # Ok(())
//! # }
//! ```
//!
//! ## Zero-trust guarantee (spec §6)
//!
//! There is no public API on any fidus crate that accepts a coordinate
//! returned by a platform API. The only environmental input is
//! [`fidus_core::env::EnvironmentContext`], which describes capabilities and
//! conditions but carries no coordinate value. The probability pool only
//! ever receives visual and interaction measurements.

#![warn(missing_docs)]

pub use fidus_core::engine::InitError;

/// Backend selection: the layer-shell backend, serving wlroots-like
/// compositors (Niri / Sway / Hyprland), KDE Plasma, and partial GNOME.
#[cfg(feature = "wayland-layer")]
pub mod wayland {
    pub use fidus_backend_wayland_layer::WaylandLayerBackend;
}

pub mod prelude {
    //! Everything a typical caller needs.

    pub use crate::builder::{BackendChoice, FidusBuilder};
    pub use fidus_core::engine::FallbackEngine;
    pub use fidus_core::prelude::*;
    pub use fidus_calibrate::CrosshairCalibrator;
    pub use fidus_calibrate::CrosshairConfig;
}

mod builder;

pub use builder::{BackendChoice, FidusBuilder};
