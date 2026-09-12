// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! fidus backend for X11 displays (real X servers, and XWayland screens
//! where the X server actually composes a root image).
//!
//! Basic X primitives only (spec §7): projection uses one tiny
//! **override-redirect** window per marker (unmanaged by any WM, placed at
//! exact root coordinates), capture uses `GetImage` on the root window.
//! Nothing here reads window-manager geometry — override-redirect windows
//! are *our own* windows at coordinates *we* chose, so the zero-trust rule
//! holds: the only coordinates fidus touches are its own.
//!
//! Because it creates one window per marker it provides **multi-marker
//! projection**, which is what the L0 Anchor calibrator needs.
//!
//! ## Capture is probed, not assumed
//!
//! `GetImage(root)` is not guaranteed to work: **rootless XWayland** (the
//! common case under Wayland compositors) has no root picture and answers
//! `BadMatch`. The backend probes one capture at connect time and reports
//! the outcome as the screen-capture permission state, so the gate refuses
//! honestly instead of failing mid-calibration. On such screens the
//! calibrated frame would describe the XWayland coordinate space anyway —
//! callers under a Wayland compositor should use the layer-shell backend.

#![warn(missing_docs)]

use fidus_core::coord::LogicalPoint;
use fidus_core::engine::InitError;
use fidus_core::env::{CompositorKind, EnvironmentContext, PermissionState};
use fidus_core::io::{
    CalibrationIo, CaptureError, CaptureIo, Frame, IoFactory, MarkerError, MarkerStyle, PixelFormat,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateWindowAux, ImageFormat, ImageOrder, Screen, Visualtype, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// The connected backend: one X connection, the root window, and the
/// projector's marker windows.
pub struct X11Backend {
    conn: RustConnection,
    screen_index: usize,
    root_size: (u32, u32),
    server_image_order: ImageOrder,
    /// Outcome of the connect-time capture probe.
    capture_probe: Result<(), String>,
    /// Live marker windows.
    markers: Vec<u32>,
}

impl X11Backend {
    /// Connects to the display named by the environment (`$DISPLAY`) and
    /// probes the capture primitive once.
    pub fn connect() -> Result<Self, InitError> {
        let (conn, screen_index) =
            x11rb::connect(None).map_err(|e| InitError::NoDisplay(e.to_string()))?;
        let screen = conn
            .setup()
            .roots
            .get(screen_index)
            .ok_or_else(|| InitError::NoDisplay("screen index out of range".into()))?;
        let mut backend = X11Backend {
            server_image_order: conn.setup().image_byte_order,
            screen_index,
            root_size: (screen.width_in_pixels as u32, screen.height_in_pixels as u32),
            capture_probe: Ok(()),
            markers: Vec::new(),
            conn,
        };
        // A 1×1 GetImage is enough to learn whether the root is readable.
        backend.capture_probe = backend.get_root_image(Some((1, 1))).map(|_| ()).map_err(|e| e.to_string());
        Ok(backend)
    }

    fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen_index]
    }

    fn root_visual(&self) -> Option<Visualtype> {
        self.screen()
            .allowed_depths
            .iter()
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.visual_id == self.screen().root_visual)
            .copied()
    }

    /// Whether the screen exposes the primitives this backend needs: a
    /// TrueColor-ish root visual, a non-empty root, and a readable root
    /// image.
    pub fn primitives_available(&self) -> bool {
        self.root_visual().is_some() && self.root_size.0 > 0 && self.capture_probe.is_ok()
    }

    /// Why the capture probe failed, if it did (diagnostics).
    pub fn capture_probe_error(&self) -> Option<&str> {
        self.capture_probe.as_ref().err().map(String::as_str)
    }

    /// Probes the environment into an [`EnvironmentContext`].
    pub fn probe_environment(&self) -> EnvironmentContext {
        EnvironmentContext {
            has_layer_shell: false,
            is_dynamic_wallpaper: None,
            multi_monitor_count: self.conn.setup().roots.len(),
            compositor_type: CompositorKind::X11,
            // Rootless XWayland: GetImage(root) → BadMatch. Reported as a
            // capture problem so the gate answers `PermissionRequired`-class
            // refusal rather than letting calibration fail mid-way.
            screen_capture_permission: if self.capture_probe.is_ok() {
                PermissionState::Granted
            } else {
                PermissionState::Revoked
            },
            wayland_input_region_supported: false,
            multi_marker_projection: true,
        }
    }

    /// Captures the root window once (diagnostics).
    pub fn capture_once(&mut self) -> Result<Frame, CaptureError> {
        self.get_root_image(None)
    }

    /// Computes the root-visual pixel value for an RGB color.
    fn pixel_of(&self, rgba: [u8; 4]) -> Result<u32, MarkerError> {
        let visual = self
            .root_visual()
            .ok_or_else(|| MarkerError::Backend("screen has no TrueColor visual".into()))?;
        let shift = |mask: u32| mask.trailing_zeros();
        Ok(((rgba[0] as u32) << shift(visual.red_mask))
            | ((rgba[1] as u32) << shift(visual.green_mask))
            | ((rgba[2] as u32) << shift(visual.blue_mask)))
    }

    fn destroy_markers(&mut self) -> Result<(), MarkerError> {
        if self.markers.is_empty() {
            return Ok(());
        }
        for window in self.markers.drain(..) {
            self.conn
                .destroy_window(window)
                .map_err(|e| MarkerError::Backend(e.to_string()))?;
        }
        // Round-trip: the removal must be visible before the caller's next
        // capture (the baseline for the following measurement).
        self.sync().map_err(MarkerError::Backend)
    }

    /// Drains the connection: replying to any request means the server has
    /// processed everything queued before it.
    fn sync(&self) -> Result<(), String> {
        self.conn
            .get_input_focus()
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())
            .map(|_| ())
    }

    /// Projects `marks` as override-redirect windows at exact root
    /// coordinates. Replaces any previously projected set; an empty slice
    /// simply removes the previous one.
    fn project(&mut self, marks: &[(LogicalPoint, MarkerStyle)]) -> Result<(), MarkerError> {
        self.destroy_markers()?;
        let screen = self.screen();
        let (root, depth) = (screen.root, screen.root_depth);
        for (pos, style) in marks {
            let size = style.size_logical.ceil().max(1.0) as u16;
            let x = pos.x.round().clamp(0.0, i16::MAX as f64) as i16;
            let y = pos.y.round().clamp(0.0, i16::MAX as f64) as i16;
            let pixel = self.pixel_of(style.rgba)?;
            let window = self
                .conn
                .generate_id()
                .map_err(|e| MarkerError::Backend(e.to_string()))?;
            self.conn
                .create_window(
                    depth,
                    window,
                    root,
                    x,
                    y,
                    size,
                    size,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    x11rb::COPY_FROM_PARENT,
                    &CreateWindowAux::new()
                        .background_pixel(pixel)
                        // Unmanaged by the WM: exact coordinates, no
                        // decorations, no focus stealing.
                        .override_redirect(1),
                )
                .map_err(|e| MarkerError::Backend(e.to_string()))?;
            self.conn
                .map_window(window)
                .map_err(|e| MarkerError::Backend(e.to_string()))?;
            self.markers.push(window);
        }
        // Round-trip so the windows exist server-side before the caller
        // captures (GetImage reads the server's state).
        self.sync().map_err(MarkerError::Backend)
    }

    /// Grabs the root window (or a `(w, h)` top-left sub-rectangle) as a
    /// top-down [`Frame`] (capture primitive).
    fn get_root_image(&self, size: Option<(u32, u32)>) -> Result<Frame, CaptureError> {
        let screen = self.screen();
        let (w, h) = size.unwrap_or(self.root_size);
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, screen.root, 0, 0, w as u16, h as u16, !0)
            .map_err(|e| CaptureError::Backend(e.to_string()))?
            .reply()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        if reply.depth < 24 {
            return Err(CaptureError::UnsupportedFormat(reply.depth as i32));
        }

        // ZPixmap at depth ≥ 24 packs 4 bytes per pixel. The X server hands
        // the data out in *its* image byte order; normalize to LSB order
        // ([B, G, R, X] per pixel — our Xrgb8888 convention).
        let mut data = reply.data;
        if matches!(self.server_image_order, ImageOrder::MSB_FIRST) {
            for px in data.chunks_exact_mut(4) {
                px.reverse();
            }
        }
        let needed = w as usize * h as usize * 4;
        if data.len() < needed {
            return Err(CaptureError::Backend("truncated GetImage reply".into()));
        }
        // Scanlines are padded to 32-bit units, which is exact for 4 bpp.
        data.truncate(needed);
        Ok(Frame { width: w, height: h, stride: w * 4, format: PixelFormat::Xrgb8888, data })
    }
}

impl Drop for X11Backend {
    fn drop(&mut self) {
        // Teardown discipline: no marker window outlives the backend.
        let _ = self.destroy_markers();
    }
}

/// Capture-only session (steady-state estimation path).
pub struct CaptureSession<'a> {
    backend: &'a mut X11Backend,
}

impl CaptureIo for CaptureSession<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.backend.get_root_image(None)
    }
}

/// Full calibration session: marker projection + capture. Destroys any
/// projected markers when dropped (spec §4.4).
pub struct CalibrationSession<'a> {
    backend: &'a mut X11Backend,
}

impl Drop for CalibrationSession<'_> {
    fn drop(&mut self) {
        let _ = self.backend.destroy_markers();
    }
}

impl CaptureIo for CalibrationSession<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.backend.get_root_image(None)
    }
}

impl CalibrationIo for CalibrationSession<'_> {
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        let (w, h) = self.backend.root_size;
        Ok((w as f64, h as f64))
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        self.backend.project(&[(pos, style)])
    }

    fn show_markers(
        &mut self,
        marks: &[(LogicalPoint, MarkerStyle)],
    ) -> Result<(), MarkerError> {
        self.backend.project(marks)
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        self.backend.destroy_markers()
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        self.backend.destroy_markers()
    }
}

impl IoFactory for X11Backend {
    fn open(&mut self) -> Result<Box<dyn CalibrationIo + '_>, InitError> {
        self.check_primitives()?;
        Ok(Box::new(CalibrationSession { backend: self }))
    }

    fn open_capture(&mut self) -> Result<Box<dyn CaptureIo + '_>, InitError> {
        self.check_primitives()?;
        Ok(Box::new(CaptureSession { backend: self }))
    }
}

impl X11Backend {
    fn check_primitives(&self) -> Result<(), InitError> {
        if self.root_visual().is_none() || self.root_size.0 == 0 {
            return Err(InitError::ProbeFailed("X screen lacks a usable visual".into()));
        }
        if let Err(e) = &self.capture_probe {
            return Err(InitError::ProbeFailed(format!(
                "root window is not capturable (rootless XWayland?): {e}"
            )));
        }
        Ok(())
    }
}
