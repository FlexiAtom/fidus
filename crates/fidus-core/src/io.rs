// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Calibration I/O session: the only two primitives fidus ever uses —
//! projecting known markers and capturing the screen — behind one trait.
//!
//! This is the concrete realization of principle 3 (*bootstrap from
//! primitives*): a calibration/estimation strategy never talks to a platform
//! directly; it talks to a [`CalibrationIo`] session that a backend provides.
//! Because [`CalibrationIo`] carries no coordinate inputs, native coordinates
//! cannot enter fidus through it.

/// Pixel formats fidus understands for captured frames and marker buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// `WL_SHM_FORMAT_ARGB8888` (premultiplied 0xAARRGGBB, little-endian).
    Argb8888,
    /// `WL_SHM_FORMAT_XRGB8888` (0xXRRGGBB, little-endian).
    Xrgb8888,
    /// `WL_SHM_FORMAT_ABGR8888` (premultiplied 0xAABBGGRR, little-endian).
    Abgr8888,
    /// `WL_SHM_FORMAT_XBGR8888` (0xXBBGGRR, little-endian).
    Xbgr8888,
}

impl PixelFormat {
    /// Number of bytes per pixel.
    pub fn bpp(self) -> usize {
        4
    }

    /// Reads the pixel at byte offset `i` as `[r, g, b, a]`.
    ///
    /// Returns `None` when the requested four-byte pixel is outside `data`;
    /// `x` channels read as fully opaque.
    pub fn read_rgba(self, data: &[u8], i: usize) -> Option<[u8; 4]> {
        let end = i.checked_add(4)?;
        let b: [u8; 4] = data.get(i..end)?.try_into().ok()?;
        match self {
            // 0xAARRGGBB little-endian → bytes B G R A
            PixelFormat::Argb8888 => Some([b[2], b[1], b[0], b[3]]),
            PixelFormat::Xrgb8888 => Some([b[2], b[1], b[0], 255]),
            // 0xAABBGGRR little-endian → bytes R G B A
            PixelFormat::Abgr8888 => Some([b[0], b[1], b[2], b[3]]),
            PixelFormat::Xbgr8888 => Some([b[0], b[1], b[2], 255]),
        }
    }

    /// Writes an `[r, g, b, a]` pixel at byte offset `i`.
    pub fn write_rgba(self, data: &mut [u8], i: usize, rgba: [u8; 4]) -> bool {
        let (r, g, b, a) = (rgba[0], rgba[1], rgba[2], rgba[3]);
        let bytes: [u8; 4] = match self {
            PixelFormat::Argb8888 => [b, g, r, a],
            PixelFormat::Xrgb8888 => [b, g, r, 255],
            PixelFormat::Abgr8888 => [r, g, b, a],
            PixelFormat::Xbgr8888 => [r, g, b, 255],
        };
        let Some(end) = i.checked_add(4) else {
            return false;
        };
        let Some(dst) = data.get_mut(i..end) else {
            return false;
        };
        dst.copy_from_slice(&bytes);
        true
    }
}

/// One captured screen frame.
///
/// Rows are stored top-down (backends normalize Y-inversion away); `stride`
/// is the row pitch in bytes and may exceed `width * 4`.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row pitch in bytes.
    pub stride: u32,
    /// Pixel format of `data`.
    pub format: PixelFormat,
    /// Raw pixel bytes.
    pub data: Vec<u8>,
}

impl Frame {
    /// Reads the pixel at `(x, y)` as `[r, g, b, a]`.
    ///
    /// Returns black-transparent for out-of-bounds coordinates instead of
    /// panicking; detection code runs on untrusted geometry.
    pub fn rgba_at(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let row = (y as usize).checked_mul(self.stride as usize);
        let pixel = (x as usize).checked_mul(self.format.bpp());
        let i = row.and_then(|r| pixel.and_then(|p| r.checked_add(p)));
        match i {
            Some(i) if i.checked_add(self.format.bpp()).is_some_and(|end| end <= self.data.len()) => {
                self.format.read_rgba(&self.data, i).unwrap_or([0, 0, 0, 0])
            }
            // A malformed public Frame is an invalid measurement, not a reason
            // to bring down a detector. Treat its unreadable pixels as absent.
            _ => [0, 0, 0, 0],
        }
    }

    /// Whether the announced geometry and backing bytes describe complete rows.
    pub fn is_valid(&self) -> bool {
        let Some(row) = (self.width as usize).checked_mul(self.format.bpp()) else {
            return false;
        };
        self.width != 0
            && self.height != 0
            && self.stride as usize >= row
            && (self.height as usize)
                .checked_mul(self.stride as usize)
                .is_some_and(|n| n <= self.data.len())
    }

    /// Logical dimensions `(width, height)`.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::{Frame, PixelFormat};

    #[test]
    fn malformed_frame_reads_as_transparent_without_panicking() {
        let frame = Frame { width: u32::MAX, height: 2, stride: 4, format: PixelFormat::Xrgb8888, data: vec![0; 4] };
        assert_eq!(frame.rgba_at(0, 1), [0, 0, 0, 0]);
        assert!(!frame.is_valid());
    }

    #[test]
    fn empty_frame_is_invalid() {
        let frame = Frame { width: 0, height: 0, stride: 0, format: PixelFormat::Xrgb8888, data: Vec::new() };
        assert!(!frame.is_valid());
    }
}

/// Visual style of a projected calibration marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkerStyle {
    /// Marker color as `[r, g, b, a]`. High-saturation, rarely-used colors
    /// are recommended; frame-difference detection makes the exact choice
    /// non-critical.
    pub rgba: [u8; 4],
    /// Marker edge length in logical pixels.
    pub size_logical: f64,
    /// Marker shape.
    pub shape: MarkerShape,
}

impl MarkerStyle {
    /// Default style: an opaque magenta solid square, 28 logical pixels.
    pub const DEFAULT: Self = Self {
        rgba: [255, 0, 255, 255],
        size_logical: 28.0,
        shape: MarkerShape::SolidSquare,
    };
}

/// Shape of a projected calibration marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerShape {
    /// Solid filled square. Its centroid corresponds to the center of the
    /// projected rectangle, which is what the L9 solver uses.
    SolidSquare,
    /// Crosshair lines with a hollow center (planned refinement).
    Crosshair,
}

/// Errors from the capture primitive.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// Backend-specific failure.
    #[error("capture backend error: {0}")]
    Backend(String),
    /// The compositor reported a failed capture.
    #[error("compositor reported capture failure: {0}")]
    Failed(String),
    /// A required protocol event did not arrive in time.
    #[error("capture timed out")]
    Timeout,
    /// The announced buffer format is not supported by fidus.
    #[error("unsupported capture buffer format {0:?}")]
    UnsupportedFormat(i32),
}

/// Errors from the marker-projection primitive.
#[derive(Debug, thiserror::Error)]
pub enum MarkerError {
    /// Backend-specific failure.
    #[error("marker backend error: {0}")]
    Backend(String),
    /// A required protocol event did not arrive in time.
    #[error("marker projection timed out")]
    Timeout,
    /// The overlay surface was closed by the compositor.
    #[error("overlay surface closed by compositor")]
    Closed,
    /// The projector was already destroyed (teardown is idempotent).
    #[error("projector already destroyed")]
    AlreadyDestroyed,
    /// This backend cannot project several markers at once (single-surface
    /// compositors). L0 Anchor needs multi-marker projection; L9 does not.
    #[error("backend cannot project multiple markers simultaneously")]
    MultiMarkersUnsupported,
}

/// Capture-only sessions: the single primitive steady-state estimation
/// needs.
///
/// Deliberately narrower than [`CalibrationIo`]: an estimator receives one
/// of these and *cannot* project markers — "calibrate, then get out"
/// (spec appendix A.5) enforced at the type level.
pub trait CaptureIo: Send {
    /// Captures the observed output as a top-down pixel frame.
    fn capture(&mut self) -> Result<Frame, CaptureError>;
}

/// One live calibration/estimation session.
///
/// Bundles the two primitives fidus relies on — screen capture and marker
/// projection — plus the lifecycle rule from spec §4.4: the projector must be
/// destroyed once coordinates have been extracted. `destroy_projector` is
/// called explicitly by the calibrator and again on `Drop`; both are safe.
pub trait CalibrationIo: CaptureIo {
    /// Returns the usable-area size of the calibrated output in logical
    /// pixels, as announced by the compositor for fidus' own overlay surface.
    ///
    /// This is a *placement hint only*: it is used to spread markers across
    /// the screen and is never stored in a [`crate::frame::CoordinateFrame`]
    /// nor fed to the probability pool. Wrong hints self-correct — markers
    /// that land off-screen are simply not detected and calibration retries
    /// with safer positions.
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError>;

    /// Projects a marker whose logical top-left corner lands exactly at
    /// `pos` (usable-area coordinates). Returns after the marker is
    /// guaranteed to be present on screen.
    fn show_marker(&mut self, pos: crate::coord::LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError>;

    /// Projects several markers simultaneously (L0 Anchor's four corner
    /// sentinels, spec §4.3). Same top-left semantics as
    /// [`show_marker`](Self::show_marker); replaces any previously projected
    /// set.
    ///
    /// The default refuses, and a backend that keeps the default must report
    /// `multi_marker_projection: false` in its probed
    /// [`EnvironmentContext`](crate::env::EnvironmentContext) so the gate
    /// never offers L0 on it. Single-surface projectors (the layer-shell
    /// backend) keep the default; window-per-marker backends (X11
    /// override-redirect windows, and later Windows/macOS) override it.
    fn show_markers(
        &mut self,
        _marks: &[(crate::coord::LogicalPoint, MarkerStyle)],
    ) -> Result<(), MarkerError> {
        Err(MarkerError::MultiMarkersUnsupported)
    }

    /// Removes the currently projected marker and waits until the removal is
    /// visible on screen.
    fn clear_marker(&mut self) -> Result<(), MarkerError>;

    /// Tears the overlay surface down immediately (spec §4.4: "destroy_layer_
    /// surface — 必须显式销毁，不留残留"). Idempotent.
    fn destroy_projector(&mut self) -> Result<(), MarkerError>;
}

/// Opens [`CalibrationIo`] / [`CaptureIo`] sessions on demand.
///
/// Implemented by platform backends. The returned sessions borrow the
/// backend; backends serialize sessions internally.
pub trait IoFactory: Send {
    /// Opens a full calibration session (capture + projection).
    fn open(&mut self) -> Result<Box<dyn CalibrationIo + '_>, crate::engine::InitError>;

    /// Opens a capture-only session for steady-state estimation: no overlay
    /// surface is ever created (spec §4.4 teardown discipline).
    fn open_capture(&mut self) -> Result<Box<dyn CaptureIo + '_>, crate::engine::InitError>;
}
