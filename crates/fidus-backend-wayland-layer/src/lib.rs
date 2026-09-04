//! fidus backend for Wayland compositors with layer-shell support
//! (wlroots-like: Niri / Sway / Hyprland, KDE Plasma, partial GNOME).
//!
//! This backend adapts **basic compositor primitives only** (spec §7):
//!
//! * projection: bare `wl_surface` + `zwlr_layer_shell_v1` + `wl_shm`
//! * capture: `zwlr_screencopy_manager_v1`
//!
//! It never binds any protocol that reports window geometry — there is no
//! native-coordinate facade here, matching the zero-trust rule (spec §1/§2).
//! The connection is independent of any toolkit the host application uses.

#![warn(missing_docs)]

mod capture;
mod marker;
mod session;
mod shm;

use fidus_core::env::{EnvironmentContext, PermissionState};
use fidus_core::engine::InitError;
use fidus_core::io::{
    CalibrationIo, CaptureError, Frame, IoFactory, MarkerError, MarkerStyle,
};
use fidus_core::coord::LogicalPoint;

use session::Loop;

/// The connected backend. One instance owns one Wayland connection; sessions
/// ([`CalibrationIo`]) borrow it for their lifetime.
pub struct WaylandLayerBackend {
    l: Loop,
}

impl WaylandLayerBackend {
    /// Connects to the Wayland display named by the environment and binds the
    /// required globals.
    pub fn connect() -> Result<Self, BackendError> {
        let conn = wayland_client::Connection::connect_to_env()
            .map_err(|e| BackendError::Connect(e.to_string()))?;
        let mut eq = conn.new_event_queue();
        let qh = eq.handle();
        let mut st = session::Session::new(qh.clone());
        conn.display().get_registry(&qh, ());
        eq.roundtrip(&mut st)
            .map_err(|e| BackendError::Protocol(e.to_string()))?;
        eq.roundtrip(&mut st)
            .map_err(|e| BackendError::Protocol(e.to_string()))?;
        Ok(WaylandLayerBackend { l: Loop { conn, eq, st } })
    }

    /// Whether the compositor exposes all primitives this backend needs.
    pub fn primitives_available(&self) -> bool {
        let st = &self.l.st;
        st.compositor.is_some()
            && st.shm.is_some()
            && st.layer_shell.is_some()
            && st.screencopy.is_some()
            && !st.outputs.is_empty()
    }

    /// Probes the live environment into an [`EnvironmentContext`].
    pub fn probe_environment(&mut self) -> EnvironmentContext {
        let st = &self.l.st;
        EnvironmentContext {
            has_layer_shell: st.layer_shell.is_some(),
            is_dynamic_wallpaper: None,
            multi_monitor_count: st.outputs.len(),
            compositor_type: fidus_core::env::CompositorKind::detect_from_env(),
            screen_capture_permission: if st.screencopy.is_some() {
                PermissionState::Granted
            } else {
                PermissionState::Unknown
            },
            wayland_input_region_supported: true,
        }
    }

    /// Captures the primary output once (diagnostics / backend smoke tests).
    pub fn capture_once(&mut self) -> Result<Frame, CaptureError> {
        let mut cache = None;
        capture::capture(&mut self.l, &mut cache)
    }
}

/// Errors at the backend boundary.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Could not connect to the Wayland display.
    #[error("failed to connect to Wayland display: {0}")]
    Connect(String),
    /// A required global is missing on this compositor.
    #[error("required global {0:?} is missing")]
    MissingGlobal(&'static str),
    /// Protocol-level failure.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A required event did not arrive in time.
    #[error("timed out waiting for the compositor")]
    Timeout,
    /// Transport-level failure.
    #[error("i/o error: {0}")]
    Io(String),
}

impl From<BackendError> for InitError {
    fn from(e: BackendError) -> Self {
        match e {
            BackendError::Connect(m) => InitError::NoDisplay(m),
            BackendError::MissingGlobal(g) => {
                InitError::ProbeFailed(format!("missing global {g}"))
            }
            other => InitError::Backend(other.to_string()),
        }
    }
}

impl From<BackendError> for CaptureError {
    fn from(e: BackendError) -> Self {
        match e {
            BackendError::Timeout => CaptureError::Timeout,
            other => CaptureError::Backend(other.to_string()),
        }
    }
}

impl From<BackendError> for MarkerError {
    fn from(e: BackendError) -> Self {
        match e {
            BackendError::Timeout => MarkerError::Timeout,
            other => MarkerError::Backend(other.to_string()),
        }
    }
}

/// One live calibration session: holds the overlay surface and the capture
/// buffer cache. Destroying the session drops the projector as well, so the
/// spec §4.4 teardown rule holds even on early returns.
pub struct BackendSession<'a> {
    l: &'a mut Loop,
    projector: Option<marker::Projector>,
    copy_cache: Option<capture::CopyBuffer>,
    hint: (f64, f64),
}

impl<'a> BackendSession<'a> {
    /// Opens a session: creates the overlay surface in hint mode and waits
    /// for the compositor to announce the usable area.
    pub fn open(backend: &'a mut WaylandLayerBackend) -> Result<Self, InitError> {
        if !backend.primitives_available() {
            return Err(InitError::ProbeFailed(
                "compositor lacks compositor/shm/layer-shell/screencopy or has no output".into(),
            ));
        }
        let (projector, hint) =
            marker::open(&mut backend.l).map_err(InitError::from_marker)?;
        Ok(BackendSession { l: &mut backend.l, projector: Some(projector), copy_cache: None, hint })
    }
}

impl CalibrationIo for BackendSession<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        capture::capture(self.l, &mut self.copy_cache)
    }

    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        Ok(self.hint)
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        let projector = self
            .projector
            .as_mut()
            .ok_or(MarkerError::AlreadyDestroyed)?;
        marker::show(self.l, projector, pos, &style)
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        match self.projector.as_mut() {
            Some(p) => marker::clear(self.l, p),
            None => Ok(()),
        }
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        match self.projector.as_mut() {
            Some(p) => {
                marker::destroy(self.l, p);
                self.projector = None;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

impl Drop for BackendSession<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.projector.as_mut() {
            marker::destroy(self.l, p);
        }
    }
}

impl IoFactory for WaylandLayerBackend {
    fn open(&mut self) -> Result<Box<dyn CalibrationIo + '_>, InitError> {
        Ok(Box::new(BackendSession::open(self)?))
    }
}

trait InitErrorFromMarker {
    fn from_marker(e: MarkerError) -> InitError;
}

impl InitErrorFromMarker for InitError {
    fn from_marker(e: MarkerError) -> InitError {
        InitError::Backend(e.to_string())
    }
}
