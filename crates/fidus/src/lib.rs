// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! # fidus — Zero-Trust Coordinate positioning engine
//!
//! fidus locates the caller's own window on screen for environments where
//! platform window-coordinate APIs cannot be trusted (Wayland compositors are
//! the canonical case). The caller supplies that window's offscreen render;
//! fidus never wraps a native coordinate API: it projects its own markers
//! through basic compositor primitives, captures the screen, and solves its
//! own coordinate frame from those measurements. It does not enumerate or
//! locate arbitrary third-party system windows, or provide window identity.
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
//! //
//! //    The render has to be *locatable*. Registration refuses appearances
//! //    that template matching cannot pin down — a gradient or a flat fill
//! //    correlates with a shifted copy of itself, so NCC reports a confident
//! //    score at an arbitrary position. Note this is not about contrast: a
//! //    linear gradient has plenty of it and is still unlocatable.
//! let target = TargetDescription::new(my_own_render());
//!
//! //    If no more distinctive render exists, opt into a best-effort track
//! //    whose confidence is capped by the measured ambiguity:
//! //    `TargetDescription::new(img).tracking_ambiguous_appearance()`.
//! engine.register_target(target).expect("estimator accepts targets");
//!
//! // 4. Steady-state tracking (L1 Fingerprint, wired in P2-a).
//! let _ = engine.estimate();
//! # Ok(())
//! # }
//! #
//! # /// Stands in for the caller's own offscreen render. Deliberately
//! # /// textured: a flat placeholder would be refused at registration.
//! # fn my_own_render() -> fidus::prelude::RgbaImage {
//! #     let (w, h) = (64u32, 48u32);
//! #     let mut data = Vec::with_capacity((w * h * 4) as usize);
//! #     for y in 0..h {
//! #         for x in 0..w {
//! #             let v = ((x * 7) ^ (y * 13)) as u8;
//! #             data.extend_from_slice(&[v, v.wrapping_mul(3), 255 - v, 255]);
//! #         }
//! #     }
//! #     fidus::prelude::RgbaImage::from_raw(w, h, data)
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

/// The layer-shell backend, serving wlroots-like compositors (Niri / Sway /
/// Hyprland), KDE Plasma, and partial GNOME. Hosts the L9 Crosshair
/// calibrator.
#[cfg(feature = "wayland-layer")]
pub mod wayland {
    pub use fidus_backend_wayland_layer::WaylandLayerBackend;
}

/// The X11 backend (override-redirect windows + `GetImage`), serving real X
/// servers. Hosts the L0 Anchor calibrator.
#[cfg(feature = "x11")]
pub mod x11 {
    pub use fidus_backend_x11::X11Backend;
}

pub mod prelude {
    //! Everything a typical caller needs.

    pub use crate::builder::{BackendChoice, FidusBuilder};
    pub use fidus_core::engine::FidusEngine;
    pub use fidus_core::prelude::*;
    pub use fidus_calibrate::{AnchorCalibrator, AnchorConfig, CrosshairCalibrator, CrosshairConfig};
    pub use fidus_estimate::{FingerprintEstimator, FusedEstimator};
}

mod builder;

pub use builder::{BackendChoice, FidusBuilder};
