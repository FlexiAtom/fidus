// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Turning fidus' own quantities into platform placement, and platform pixels
//! back into a [`Frame`].
//!
//! Both directions are pure functions over values fidus produced itself, so
//! they are tested without a display server.

use fidus_core::coord::LogicalPoint;
use fidus_core::io::{CaptureError, Frame, MarkerError, MarkerShape, MarkerStyle, PixelFormat};

use crate::api::{MarkerSpec, RawFrame, WorkArea};

/// Largest marker edge accepted, in logical pixels. Beyond this we are
/// projecting a rectangle, not a calibration sentinel.
pub const MAX_MARKER_EDGE: f64 = 4096.0;

/// Plans one marker: quantise the logical position to integer logical pixels,
/// then offset it into screen coordinates.
///
/// # Why the quantisation lives here
///
/// The contract requires the value handed to the platform and the value the
/// solver expects to agree exactly: a half-pixel disagreement goes straight
/// into the affine residual. Rounding happens **once**, here, with the same
/// rule the calibrator uses (`f64::round`, spec's "量化到整数逻辑像素"); the
/// anchor calibrator already hands over integers, and L9-style fractional
/// margins would round identically on any platform.
///
/// A marker whose rounded edge is smaller than one pixel is refused rather than
/// silently inflated: the solver pairs the marker's *centre* with what it
/// detects, and inventing a 1 px marker where the caller asked for 0.4 px would
/// change that centre.
pub fn plan_marker(
    pos: LogicalPoint,
    style: MarkerStyle,
    area: WorkArea,
) -> Result<MarkerSpec, MarkerError> {
    if style.shape != MarkerShape::SolidSquare {
        return Err(MarkerError::Backend(
            "the Windows backend projects solid squares only; crosshair markers are not implemented"
                .into(),
        ));
    }
    let edge = style.size_logical;
    if !edge.is_finite() || edge <= 0.0 || edge > MAX_MARKER_EDGE {
        return Err(MarkerError::Backend(format!(
            "marker edge {edge} is outside 0 < edge <= {MAX_MARKER_EDGE} logical pixels"
        )));
    }
    let rounded = edge.round();
    if rounded < 1.0 {
        return Err(MarkerError::Backend(format!(
            "marker edge {edge} rounds to 0 pixels"
        )));
    }
    if !pos.x.is_finite() || !pos.y.is_finite() {
        return Err(MarkerError::Backend("marker position is not finite".into()));
    }
    // Screen coordinates = work-area origin + rounded logical position. The
    // origin is a *placement* offset (we are telling the system where to put
    // our window); it is never stored, never measured back.
    let x = f64::from(area.left) + pos.x.round();
    let y = f64::from(area.top) + pos.y.round();
    if x < f64::from(i32::MIN) || x > f64::from(i32::MAX) || y < f64::from(i32::MIN) || y > f64::from(i32::MAX) {
        return Err(MarkerError::Backend(
            "marker position is outside Win32 coordinate limits".into(),
        ));
    }
    Ok(MarkerSpec {
        x: x as i32,
        y: y as i32,
        size: rounded as i32,
        rgb: [style.rgba[0], style.rgba[1], style.rgba[2]],
    })
}

/// Validates a raw capture and adopts it as a [`Frame`].
///
/// Fails closed: a truncated or malformed buffer is an `Err`, never a frame of
/// fabricated pixels. A capture that could not be taken at all is already an
/// error at the platform layer, so a black frame can never be mistaken for a
/// successful measurement (contract §2.1).
pub fn raw_to_frame(raw: RawFrame) -> Result<Frame, CaptureError> {
    if raw.width == 0 || raw.height == 0 {
        return Err(CaptureError::Backend("capture has zero extent".into()));
    }
    let row = (raw.width as usize)
        .checked_mul(4)
        .ok_or_else(|| CaptureError::Backend("capture row size overflow".into()))?;
    if (raw.stride as usize) < row {
        return Err(CaptureError::Backend(format!(
            "capture stride {} is smaller than one row ({row} bytes)",
            raw.stride
        )));
    }
    let needed = (raw.height as usize)
        .checked_mul(raw.stride as usize)
        .ok_or_else(|| CaptureError::Backend("capture buffer size overflow".into()))?;
    if raw.data.len() < needed {
        return Err(CaptureError::Backend(format!(
            "capture buffer is truncated: {} < {needed} bytes",
            raw.data.len()
        )));
    }
    Ok(Frame {
        width: raw.width,
        height: raw.height,
        stride: raw.stride,
        format: PixelFormat::Xrgb8888,
        data: raw.data,
    })
}
