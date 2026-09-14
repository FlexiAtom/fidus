// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Protocol session: connection, registry binding, and a timeout-capable
//! event loop.
//!
//! Everything here talks to the most basic compositor primitives only
//! (`wl_compositor`, `wl_shm`, `wl_output`) plus the two WLR protocols fidus
//! needs (layer-shell for projection, screencopy for capture). No
//! window-coordinate protocol is ever bound — that is the zero-trust rule
//! applied to the backend layer.

use std::io;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_region, wl_shm, wl_surface,
};
use wayland_client::protocol::wl_shm_pool;
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use fidus_core::io::PixelFormat;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1;
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1;
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1;

use crate::BackendError;

/// All protocol state for one backend connection.
pub(crate) struct Session {
    pub qh: QueueHandle<Session>,
    pub compositor: Option<wl_compositor::WlCompositor>,
    pub shm: Option<wl_shm::WlShm>,
    pub layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    pub screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    pub outputs: Vec<wl_output::WlOutput>,
    /// Physical mode size of the first output, if announced (placement
    /// fallback hint only).
    pub output_mode: Option<(u32, u32)>,
    /// Scale factor announced for the first output (placement fallback hint
    /// only).
    pub output_scale: Option<u32>,

    // Overlay surface (marker projector).
    pub layer_surface: Option<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    pub overlay_surface: Option<wl_surface::WlSurface>,
    pub configure_seen: bool,
    /// Monotonic configure generation; each configure event advances it.
    pub configure_generation: u64,
    pub configure_size: (u32, u32),
    pub closed: bool,

    // Presentation sync (wl_surface.frame callbacks).
    pub cb_done: u64,
    pub cb_target: u64,

    // Screencopy state for the frame in flight. `Some(None)` marks an
    // announced-but-unsupported format.
    pub cp_format: Option<Option<PixelFormat>>,
    pub cp_size: Option<(u32, u32, u32)>,
    pub cp_y_invert: bool,
    pub cp_ready: bool,
    pub cp_failed: bool,
    /// Lifecycle of the screencopy buffer. `ready` is deliberately not reuse permission.
    pub cp_buffer_state: BufferState,
    pub cp_buffer: Option<wl_buffer::WlBuffer>,
    pub marker_buffer: Option<wl_buffer::WlBuffer>,
    pub clear_buffer: Option<wl_buffer::WlBuffer>,
    pub marker_buffer_state: BufferState,
    pub clear_buffer_state: BufferState,
}

/// State tracked for every reusable wl_buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum BufferState {
    Idle,
    Submitted,
    Ready,
    Failed,
    Released,
    Destroyed,
}

impl BufferState {
    pub(crate) fn may_reuse(self) -> bool {
        matches!(self, Self::Idle | Self::Released)
    }

}

impl Session {
    pub(crate) fn new(qh: QueueHandle<Session>) -> Self {
        Session {
            qh,
            compositor: None,
            shm: None,
            layer_shell: None,
            screencopy: None,
            outputs: Vec::new(),
            output_mode: None,
            output_scale: None,
            layer_surface: None,
            overlay_surface: None,
            configure_seen: false,
            configure_generation: 0,
            configure_size: (0, 0),
            closed: false,
            cb_done: 0,
            cb_target: 0,
            cp_format: None,
            cp_size: None,
            cp_y_invert: false,
            cp_ready: false,
            cp_failed: false,
            cp_buffer_state: BufferState::Idle,
            cp_buffer: None,
            marker_buffer: None,
            clear_buffer: None,
            marker_buffer_state: BufferState::Idle,
            clear_buffer_state: BufferState::Idle,
        }
    }

    pub(crate) fn reset_screencopy(&mut self) {
        self.cp_format = None;
        self.cp_size = None;
        self.cp_y_invert = false;
        self.cp_ready = false;
        self.cp_failed = false;
    }

}

/// Owns the connection, the event queue and the session state, and provides
/// timeout-capable dispatch waits.
pub(crate) struct Loop {
    pub conn: Connection,
    pub eq: EventQueue<Session>,
    pub st: Session,
}

impl Loop {
    /// Dispatches until `cond(&state)` holds or `timeout` elapses.
    pub(crate) fn wait_for<F>(&mut self, timeout: Duration, mut cond: F) -> Result<(), BackendError>
    where
        F: FnMut(&Session) -> bool,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if cond(&self.st) {
                return Ok(());
            }
            self.eq
                .dispatch_pending(&mut self.st)
                .map_err(|e| BackendError::Protocol(e.to_string()))?;
            if cond(&self.st) {
                return Ok(());
            }
            self.conn.flush().map_err(|e| BackendError::Io(e.to_string()))?;
            if cond(&self.st) {
                return Ok(());
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(BackendError::Timeout);
            }
            let ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
            let fd = self.conn.backend().poll_fd().as_raw_fd();
            let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }];
            let r = unsafe { libc::poll(fds.as_mut_ptr(), 1, ms) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(BackendError::Io(e.to_string()));
            }
            if r > 0 {
                // Read the socket into the event queue, then loop to dispatch.
                // Spurious wakeups surface as WouldBlock and simply retry.
                if let Some(guard) = self.conn.prepare_read() {
                    if let Err(e) = guard.read() {
                        match e {
                            wayland_backend::client::WaylandError::Io(e)
                                if e.kind() == io::ErrorKind::WouldBlock => {}
                            e => return Err(BackendError::Io(e.to_string())),
                        }
                    }
                }
            }
        }
    }

}

// ---------------------------------------------------------------------------
// Dispatch impls
// ---------------------------------------------------------------------------

// Global objects that only *produce* requests (compositor, shm, managers)
// never emit events relevant to fidus; they still need no-op impls so the
// registry can bind them.
macro_rules! noop_dispatch {
    ($($ty:ty),* $(,)?) => {
        $(
            impl Dispatch<$ty, ()> for Session {
                fn event(
                    _state: &mut Self,
                    _: &$ty,
                    _: <$ty as wayland_client::Proxy>::Event,
                    _: &(),
                    _: &Connection,
                    _: &QueueHandle<Self>,
                ) {
                }
            }
        )*
    };
}

noop_dispatch!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
);

impl Dispatch<wl_registry::WlRegistry, ()> for Session {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_output" => {
                    if state.outputs.len() < 8 {
                        state.outputs.push(registry.bind(name, version.min(4), qh, ()));
                    }
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()));
                }
                "zwlr_screencopy_manager_v1" => {
                    state.screencopy = Some(registry.bind(name, version.min(3), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Session {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // Mode size is a physical hardware fact used only as a placement
            // fallback hint; it never becomes fidus state.
            wl_output::Event::Mode { flags, width, height, .. } => {
                if let Ok(f) = flags.into_result() {
                    if f.contains(wl_output::Mode::Current) {
                        state.output_mode = Some((width.max(0) as u32, height.max(0) as u32));
                    }
                }
            }
            wl_output::Event::Scale { factor } => {
                state.output_scale = Some(factor.max(1) as u32)
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Session {
    fn event(
        _state: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, ()> for Session {
    fn event(
        _state: &mut Self,
        _: &wl_region::WlRegion,
        _: wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Session {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_callback::Event::Done { .. }) {
            state.cb_done += 1;
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Session {
    fn event(
        state: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if !matches!(event, wl_buffer::Event::Release) {
            return;
        }
        if state.cp_buffer.as_ref().is_some_and(|b| b == buffer) {
            state.cp_buffer_state = BufferState::Released;
        }
        if state.marker_buffer.as_ref().is_some_and(|b| b == buffer) {
            state.marker_buffer_state = BufferState::Released;
        }
        if state.clear_buffer.as_ref().is_some_and(|b| b == buffer) {
            state.clear_buffer_state = BufferState::Released;
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Session {
    fn event(
        state: &mut Self,
        proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // Ack immediately: keeps the client compliant with "ack before
            // attach" no matter how the caller interleaves commits.
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                proxy.ack_configure(serial);
                state.configure_seen = true;
                state.configure_generation = state.configure_generation.saturating_add(1);
                state.configure_size = (width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for Session {
    fn event(
        state: &mut Self,
        _: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event as Ev;
        match event {
            Ev::Buffer { format, width, height, stride } => {
                state.cp_format = Some(shm_format_to_pixel_format(format));
                state.cp_size = Some((width, height, stride));
            }
            Ev::Flags { flags } => {
                if let Ok(f) = flags.into_result() {
                    state.cp_y_invert = f.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
                }
            }
            Ev::Ready { .. } => {
                state.cp_ready = true;
                if state.cp_buffer_state == BufferState::Submitted {
                    state.cp_buffer_state = BufferState::Ready;
                }
            }
            Ev::Failed => {
                state.cp_failed = true;
                if state.cp_buffer_state == BufferState::Submitted {
                    state.cp_buffer_state = BufferState::Failed;
                }
            }
            _ => {}
        }
    }
}

fn shm_format_to_pixel_format(
    format: WEnum<wl_shm::Format>,
) -> Option<PixelFormat> {
    match format.into_result() {
        Ok(wl_shm::Format::Argb8888) => Some(PixelFormat::Argb8888),
        Ok(wl_shm::Format::Xrgb8888) => Some(PixelFormat::Xrgb8888),
        Ok(wl_shm::Format::Abgr8888) => Some(PixelFormat::Abgr8888),
        Ok(wl_shm::Format::Xbgr8888) => Some(PixelFormat::Xbgr8888),
        _ => None,
    }
}

/// Maps an announced shm format to a fidus [`PixelFormat`]; `None` for
/// formats fidus cannot read.
#[cfg(test)]
mod tests {
    use super::BufferState;

    #[test]
    fn ready_is_not_reusable_without_release() {
        assert!(!BufferState::Ready.may_reuse());
        assert!(!BufferState::Ready.may_reuse());
    }

    #[test]
    fn failed_and_submitted_are_not_reusable() {
        assert!(!BufferState::Submitted.may_reuse());
        assert!(!BufferState::Failed.may_reuse());
        assert!(!BufferState::Failed.may_reuse());
        assert!(!BufferState::Destroyed.may_reuse());
    }

    #[test]
    fn release_is_the_reuse_boundary() {
        assert!(BufferState::Idle.may_reuse());
        assert!(BufferState::Released.may_reuse());
        assert!(!BufferState::Submitted.may_reuse());
    }
}
