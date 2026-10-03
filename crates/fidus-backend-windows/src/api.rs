// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Platform-neutral half of the Windows backend: the primitive interface, the
//! connect-time probe facts, and their mapping to
//! [`EnvironmentContext`](fidus_core::env::EnvironmentContext).
//!
//! This module (and [`crate::plan`] / [`crate::projector`]) compiles on **any**
//! host, including the Linux CI gate. Only [`crate::sys`] and
//! [`crate::backend`] are Windows-only. The split is deliberate: everything
//! that can be wrong *without* a display server — marker quantisation, frame
//! validation, command ordering, teardown idempotence, capability reporting —
//! stays under test on every platform.

use fidus_core::env::{CompositorKind, EnvironmentContext, PermissionState};

/// A rectangle in virtual-screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    /// Left edge, in virtual-screen pixels.
    pub left: i32,
    /// Top edge, in virtual-screen pixels.
    pub top: i32,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
}

impl WorkArea {
    /// Creates a work area.
    pub const fn new(left: i32, top: i32, width: i32, height: i32) -> Self {
        Self { left, top, width, height }
    }

    /// Whether the rectangle covers any pixels at all.
    pub fn is_usable(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Size in pixels, clamped to non-negative.
    pub fn size(&self) -> (u32, u32) {
        (self.width.max(0) as u32, self.height.max(0) as u32)
    }
}

/// One marker to project: top-left corner in **screen** coordinates, plus edge
/// length and opaque colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerSpec {
    /// Screen x of the marker's left edge.
    pub x: i32,
    /// Screen y of the marker's top edge.
    pub y: i32,
    /// Edge length in pixels.
    pub size: i32,
    /// Opaque colour as `[r, g, b]`.
    pub rgb: [u8; 3],
}

/// Opaque handle of a projected marker window.
///
/// The platform half decides what the integer means (on Windows it is the
/// window handle, carried as an integer so that the platform-neutral
/// [`crate::projector`] — and only it — can pass it back). It is never
/// dereferenced outside [`crate::sys`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkerId(pub u64);

/// A captured frame exactly as the platform hands it over.
///
/// Rows are **top-down** and pixels are 32-bit XRGB in `B, G, R, X` byte order
/// (what `BitBlt` into a top-down DIB section produces, and what
/// `PixelFormat::Xrgb8888` reads as `[r, g, b, 255]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFrame {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row pitch in bytes.
    pub stride: u32,
    /// Pixel bytes.
    pub data: Vec<u8>,
}

/// A platform call failed. Carries the platform's own message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PlatformError(pub String);

impl PlatformError {
    /// Creates an error from anything printable.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// How much DPI awareness the process managed to obtain.
///
/// The backend asks for per-monitor v2 so that its coordinates *are* physical
/// pixels; a fallback is reported honestly rather than assumed away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiAwareness {
    /// `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)` succeeded.
    PerMonitorV2,
    /// Only the legacy system-wide `SetProcessDPIAware()` succeeded.
    System,
    /// Neither succeeded; the process is DPI-unaware and the system may
    /// virtualise coordinates.
    None,
}

impl DpiAwareness {
    /// Whether placement coordinates can be trusted to be physical pixels.
    pub fn is_physical(self) -> bool {
        matches!(self, DpiAwareness::PerMonitorV2 | DpiAwareness::System)
    }
}

/// What the connect-time probe learned about this host.
///
/// Every field is either a capability, a *placement hint* (work-area geometry),
/// or a diagnostic. None of it carries a window coordinate, so none of it can
/// reach a [`fidus_core::frame::CoordinateFrame`].
#[derive(Clone, Debug)]
pub struct ProbeFacts {
    /// DPI awareness actually obtained.
    pub dpi_awareness: DpiAwareness,
    /// Number of display monitors reported.
    pub monitors: usize,
    /// Usable area of the primary display (the calibrated output).
    pub work_area: WorkArea,
    /// Outcome of a real 1×1 capture performed at connect time.
    pub capture_probe: Result<(), String>,
}

impl ProbeFacts {
    /// Whether the capture primitive worked when probed.
    pub fn capture_ok(&self) -> bool {
        self.capture_probe.is_ok()
    }

    /// The probe failure message, if any.
    pub fn capture_error(&self) -> Option<&str> {
        self.capture_probe.as_ref().err().map(String::as_str)
    }
}

/// Builds the environment description the gate reasons about.
///
/// Capabilities, not identity: `has_layer_shell` is `false` because Windows has
/// no layer shell (so L9 Crosshair is honestly unavailable), while
/// `multi_marker_projection` is `true` because this backend creates one window
/// per marker (so L0 Anchor qualifies). The gate then picks L0 for a *capability*
/// reason, which is the rule the contract asks for.
///
/// `wayland_input_region_supported` is reported `false`: it names a Wayland
/// primitive. Windows click-through is achieved a different way (layered +
/// `WS_EX_TRANSPARENT`, measured) and must not be described with a Wayland field
/// name; L9 requires a layer shell and is unavailable here regardless.
pub fn environment_context(facts: &ProbeFacts) -> EnvironmentContext {
    EnvironmentContext {
        has_layer_shell: false,
        is_dynamic_wallpaper: None,
        multi_monitor_count: facts.monitors,
        compositor_type: CompositorKind::Windows,
        screen_capture_permission: if facts.capture_ok() {
            PermissionState::Granted
        } else {
            PermissionState::Revoked
        },
        wayland_input_region_supported: false,
        multi_marker_projection: true,
    }
}

/// The primitives this backend needs from a Windows host.
///
/// Implemented for real by [`crate::sys::Win32`] and, in tests, by fakes — which
/// is how the "presentation must be synchronised", "teardown is idempotent" and
/// "failure must not be reported as a black frame" rules get regression tests
/// without a display server.
pub trait Platform: Send + Sync + 'static {
    /// What the connect-time probe learned.
    fn facts(&self) -> &ProbeFacts;

    /// Captures the primary work area, top-down, `B, G, R, X` byte order.
    fn capture_work_area(&self) -> Result<RawFrame, PlatformError>;

    /// Creates a marker window at `spec`'s coordinates with `spec`'s colour.
    ///
    /// The window starts hidden; [`present_marker`](Self::present_marker) makes
    /// it visible.
    fn create_marker(&self, spec: &MarkerSpec) -> Result<MarkerId, PlatformError>;

    /// Shows a marker created by [`create_marker`](Self::create_marker).
    fn present_marker(&self, id: MarkerId) -> Result<(), PlatformError>;

    /// Destroys a marker window.
    fn destroy_marker(&self, id: MarkerId) -> Result<(), PlatformError>;

    /// Blocks until the platform has presented everything submitted so far.
    ///
    /// This is the contract's "return only once the marker is really on
    /// screen" primitive (spec §4.4 checks), and the Windows counterpart of a
    /// Wayland frame callback. Measured: without it, a plain `SRCCOPY` capture
    /// saw a just-shown marker in 8/20 trials.
    fn sync_presentation(&self) -> Result<(), PlatformError>;

    /// Number of marker windows this platform still has alive (diagnostics).
    ///
    /// Used by the live probe to prove teardown ("no residual window"), the
    /// Windows analogue of `niri msg layers` / `xwininfo -root -tree`.
    fn live_marker_windows(&self) -> usize;

    /// Diagnostics: does a mouse hit at this screen point land on one of our
    /// marker windows?
    ///
    /// This is the automatable half of the contract's "the marker must not
    /// intercept input" check (spec §11.4). It is not part of any measurement
    /// path: the answer never enters a capture, a frame or the probability pool.
    fn point_intercepted_by_marker(&self, x: i32, y: i32) -> bool;
}
