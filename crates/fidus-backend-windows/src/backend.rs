// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! The assembled backend: connect → probe → sessions → the two primitives.
//!
//! Nothing here translates platform coordinates: [`crate::plan`] turns fidus'
//! own logical positions into placement, and captures come back as pixels.

use std::sync::Arc;

use fidus_core::coord::LogicalPoint;
use fidus_core::engine::InitError;
use fidus_core::env::EnvironmentContext;
use fidus_core::io::{
    CalibrationIo, CaptureError, CaptureIo, Frame, IoFactory, MarkerError, MarkerStyle,
};

use crate::api::{environment_context, Platform, WorkArea};
use crate::plan::{plan_marker, raw_to_frame};
use crate::projector::Projector;
use crate::sys::Win32;

/// The connected Windows backend.
///
/// Provides one window per marker, which is what the L0 Anchor calibrator
/// needs; L9 Crosshair is honestly unavailable (no layer shell on Windows) and
/// the gate never offers it.
pub struct WindowsBackend {
    platform: Arc<dyn Platform>,
    /// Started on first projection. A capture-only session never creates a
    /// window — and never starts a thread (spec §4.4).
    projector: Option<Projector>,
}

impl WindowsBackend {
    /// Connects to the Windows desktop and probes the capture primitive once.
    ///
    /// No marker window exists yet at this point.
    pub fn connect() -> Result<Self, InitError> {
        let host = Win32::connect().map_err(|e| InitError::NoDisplay(e.to_string()))?;
        Ok(Self { platform: Arc::new(host), projector: None })
    }

    /// The raw primitive interface (diagnostics and the live probe).
    pub fn platform(&self) -> &dyn Platform {
        &*self.platform
    }

    /// Whether both primitives this backend needs are available: a non-empty
    /// work area and a screen the capture probe could read.
    pub fn primitives_available(&self) -> bool {
        let facts = self.platform.facts();
        facts.work_area.is_usable() && facts.capture_ok()
    }

    /// Why the connect-time capture probe failed, if it did (diagnostics).
    pub fn capture_probe_error(&self) -> Option<&str> {
        self.platform.facts().capture_error()
    }

    /// Probes the environment into an [`EnvironmentContext`].
    pub fn probe_environment(&self) -> EnvironmentContext {
        environment_context(self.platform.facts())
    }

    /// Captures the work area once (diagnostics).
    pub fn capture_once(&mut self) -> Result<Frame, CaptureError> {
        self.capture()
    }

    /// Diagnostics: does a mouse hit at this screen point land on one of our
    /// marker windows?
    ///
    /// Always `false` when nothing is projected. Used by the live probe to
    /// check the contract's "the marker must not intercept input" rule with a
    /// real hit test instead of an assumption.
    pub fn point_intercepted_by_marker(&self, x: i32, y: i32) -> bool {
        self.platform.point_intercepted_by_marker(x, y)
    }

    /// Diagnostics: marker windows still alive (must be 0 after teardown).
    pub fn live_marker_windows(&self) -> usize {
        self.platform.live_marker_windows()
    }

    /// The usable area of the calibrated output, as a placement hint.
    fn work_area(&self) -> WorkArea {
        self.platform.facts().work_area
    }

    fn capture(&self) -> Result<Frame, CaptureError> {
        let raw = self
            .platform
            .capture_work_area()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        raw_to_frame(raw)
    }

    fn check_primitives(&self) -> Result<(), InitError> {
        if !self.work_area().is_usable() {
            return Err(InitError::ProbeFailed("the primary work area is empty".into()));
        }
        match self.platform.facts().capture_error() {
            Some(e) => Err(InitError::ProbeFailed(format!(
                "the screen is not capturable (no interactive desktop, secure desktop, or capture blocked): {e}"
            ))),
            None => Ok(()),
        }
    }

    /// The projector, started on first use.
    fn projector(&mut self) -> Result<&Projector, MarkerError> {
        if self.projector.is_none() {
            let projector = Projector::start(Arc::clone(&self.platform))
                .map_err(|e| MarkerError::Backend(e.to_string()))?;
            self.projector = Some(projector);
        }
        self.projector
            .as_ref()
            .ok_or_else(|| MarkerError::Backend("projector could not be started".into()))
    }
}

/// Capture-only session (steady-state estimation path, spec §4.4).
pub struct CaptureSession<'a> {
    backend: &'a WindowsBackend,
}

impl CaptureIo for CaptureSession<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.backend.capture()
    }
}

/// Full calibration session: marker projection plus capture.
///
/// Dropping it destroys whatever is still projected, so the overlay cannot
/// outlive the session even if a calibrator exits early (spec §4.4).
pub struct CalibrationSession<'a> {
    backend: &'a mut WindowsBackend,
}

impl Drop for CalibrationSession<'_> {
    fn drop(&mut self) {
        let _ = self.destroy_projector();
    }
}

impl CaptureIo for CalibrationSession<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.backend.capture()
    }
}

impl CalibrationIo for CalibrationSession<'_> {
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        let area = self.backend.work_area();
        if !area.is_usable() {
            return Err(MarkerError::Backend("the primary work area is empty".into()));
        }
        let (width, height) = area.size();
        Ok((f64::from(width), f64::from(height)))
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        self.show_markers(&[(pos, style)])
    }

    fn show_markers(&mut self, marks: &[(LogicalPoint, MarkerStyle)]) -> Result<(), MarkerError> {
        let area = self.backend.work_area();
        let specs = marks
            .iter()
            .map(|(pos, style)| plan_marker(*pos, *style, area))
            .collect::<Result<Vec<_>, MarkerError>>()?;
        self.backend
            .projector()?
            .show(specs)
            .map_err(|e| MarkerError::Backend(e.to_string()))
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        self.destroy_projector()
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        match self.backend.projector.as_ref() {
            Some(projector) => {
                projector.clear().map_err(|e| MarkerError::Backend(e.to_string()))
            }
            // Idempotent: with no projector there is nothing to tear down, and
            // that is a success, not an `AlreadyDestroyed` error.
            None => Ok(()),
        }
    }
}

impl IoFactory for WindowsBackend {
    fn open(&mut self) -> Result<Box<dyn CalibrationIo + '_>, InitError> {
        self.check_primitives()?;
        Ok(Box::new(CalibrationSession { backend: self }))
    }

    fn open_capture(&mut self) -> Result<Box<dyn CaptureIo + '_>, InitError> {
        self.check_primitives()?;
        Ok(Box::new(CaptureSession { backend: self }))
    }
}
