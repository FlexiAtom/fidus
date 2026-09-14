// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Marker projection via a bare `wl_surface` + `zwlr_layer_shell_v1` overlay.
//!
//! Constraints verified on Niri (spec §4.1 and the attached verification
//! document), all enforced here:
//!
//! 1. **Bare surface** — the `wl_surface` is created directly from
//!    `wl_compositor`; Qt/GTK surfaces are never reused (`Surface already has
//!    role`).
//! 2. **Protocol timing** — commit → configure → ack → *then* attach. Acks are
//!    sent from the configure handler itself, so the client is always
//!    compliant regardless of caller interleaving.
//! 3. **Click-through** — `LAYER_OVERLAY` + `KEYBOARD_INTERACTIVITY_NONE` +
//!    empty input region. The empty input region is the essential part.
//! 4. **Anchoring trap** — `anchor=0` makes wlroots center the surface and
//!    ignore margins; the surface is always anchored `TOP|LEFT` so margins
//!    become exact usable-area offsets.
//! 5. **Lifecycle** — the surface is destroyed the moment calibration data is
//!    extracted; `destroy` is idempotent and also runs on `Drop`.

use std::time::Duration;

use wayland_client::protocol::{wl_buffer, wl_compositor, wl_region, wl_shm, wl_surface};
use wayland_client::QueueHandle;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::KeyboardInteractivity;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;
use fidus_core::coord::LogicalPoint;
use fidus_core::io::{MarkerError, MarkerStyle};

use crate::session::{BufferState, Loop, Session};
use crate::shm::ShmPool;

/// How long to wait for the initial configure before giving up.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(3);
/// Frame callbacks awaited per presentation (belt and braces).
const SETTLE_FRAMES: u64 = 2;

/// The projected overlay surface and its buffers.
pub(crate) struct Projector {
    surface: wl_surface::WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
    buffers: Option<MarkerBuffers>,
    destroyed: bool,
    poisoned: bool,
}

struct MarkerBuffers {
    shm_pool: ShmPool,
    marker: wl_buffer::WlBuffer,
    clear: wl_buffer::WlBuffer,
    /// Buffer edge length in pixels (== logical size; buffer scale is 1).
    size: u32,
    rgba: [u8; 4],
}

/// Creates the overlay surface in "usable-area hint" mode: anchored to all
/// edges with size 0, so the compositor's configure announces the usable area.
///
/// The announced size is a *placement hint only* (see
/// [`fidus_core::io::CalibrationIo::usable_size_hint`]): it spreads markers
/// across the screen and never becomes fidus state — off-screen markers are
/// simply not detected and calibration retries with safer positions.
fn cleanup_open_failure(
    l: &mut Loop,
    layer_surface: &ZwlrLayerSurfaceV1,
    surface: &wl_surface::WlSurface,
    error: MarkerError,
) -> Result<(Projector, (f64, f64)), MarkerError> {
    layer_surface.destroy();
    surface.destroy();
    // The proxies were not usable, but they may already be visible to the
    // session after configure. Clear them before returning so a retry cannot
    // submit against a destroyed surface.
    if l.st.layer_surface.as_ref().is_some_and(|current| current == layer_surface) {
        l.st.layer_surface = None;
    }
    if l.st.overlay_surface.as_ref().is_some_and(|current| current == surface) {
        l.st.overlay_surface = None;
    }
    l.st.configure_seen = false;
    l.st.closed = false;
    l.conn
        .flush()
        .map_err(|flush| MarkerError::Backend(format!("{error}; cleanup flush failed: {flush}")))?;
    Err(error)
}

pub(crate) fn open(l: &mut Loop) -> Result<(Projector, (f64, f64)), MarkerError> {
    let compositor: wl_compositor::WlCompositor = l
        .st
        .compositor
        .clone()
        .ok_or_else(|| MarkerError::Backend("wl_compositor not bound".into()))?;
    let layer_shell: ZwlrLayerShellV1 = l
        .st
        .layer_shell
        .clone()
        .ok_or_else(|| MarkerError::Backend("zwlr_layer_shell_v1 not bound".into()))?;
    let output = l
        .st
        .outputs
        .first()
        .cloned()
        .ok_or_else(|| MarkerError::Backend("no wl_output bound".into()))?;

    let surface = compositor.create_surface(&l.st.qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(&output),
        Layer::Overlay,
        "fidus-calib".into(),
        &l.st.qh,
        (),
    );

    layer_surface.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
    layer_surface.set_size(0, 0);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer_surface.set_exclusive_zone(0);

    l.st.configure_seen = false;
    l.st.closed = false;
    surface.commit();

    if l
        .wait_for(CONFIGURE_TIMEOUT, |st: &Session| st.configure_seen || st.closed)
        .is_err()
    {
        return cleanup_open_failure(l, &layer_surface, &surface, MarkerError::Timeout);
    }
    if l.st.closed {
        return cleanup_open_failure(l, &layer_surface, &surface, MarkerError::Closed);
    }

    // Click-through: an empty input region is what actually routes pointer
    // events to the windows below (KEYBOARD_INTERACTIVITY_NONE alone does not).
    set_empty_input_region(&compositor, &surface, &l.st.qh);

    l.st.layer_surface = Some(layer_surface.clone());
    l.st.overlay_surface = Some(surface.clone());

    let (w, h) = l.st.configure_size;
    let hint = if w > 0 && h > 0 {
        (w as f64, h as f64)
    } else {
        // Fallback: derive the hint from the announced physical mode and
        // scale. Still only a placement hint.
        let (mw, mh) = l.st.output_mode.unwrap_or((0, 0));
        let scale = l.st.output_scale.unwrap_or(1).max(1) as f64;
        (mw as f64 / scale, mh as f64 / scale)
    };
    if hint.0 < 1.0 || hint.1 < 1.0 {
        return cleanup_open_failure(
            l,
            &layer_surface,
            &surface,
            MarkerError::Backend("could not determine usable-area hint".into()),
        );
    }

    let projector = Projector { surface, layer_surface, buffers: None, destroyed: false, poisoned: false };
    Ok((projector, hint))
}

fn set_empty_input_region(
    compositor: &wl_compositor::WlCompositor,
    surface: &wl_surface::WlSurface,
    qh: &QueueHandle<Session>,
) {
    let region: wl_region::WlRegion = compositor.create_region(qh, ());
    // No wl_region.add calls: the region stays empty, so the surface receives
    // no pointer/touch input at all.
    surface.set_input_region(Some(&region));
    region.destroy();
}

/// Tears down all protocol objects. Idempotent; also invoked from `Drop`.
pub(crate) fn destroy(l: &mut Loop, projector: &mut Projector) -> Result<(), MarkerError> {
    if projector.destroyed {
        return Ok(());
    }
    projector.layer_surface.destroy();
    projector.surface.destroy();
    if let Some(b) = projector.buffers.take() {
        b.marker.destroy();
        b.clear.destroy();
        b.shm_pool.destroy(&l.conn);
    }
    projector.destroyed = true;
    l.st.layer_surface = None;
    l.st.overlay_surface = None;
    l.conn.flush().map_err(|e| MarkerError::Backend(e.to_string()))
}

/// Projects a marker with its logical top-left corner at `pos`.
pub(crate) fn show(
    l: &mut Loop,
    projector: &mut Projector,
    pos: LogicalPoint,
    style: &MarkerStyle,
) -> Result<(), MarkerError> {
    if projector.destroyed {
        return Err(MarkerError::AlreadyDestroyed);
    }
    if projector.poisoned {
        return Err(MarkerError::Backend("marker projector is poisoned after an unconfirmed buffer timeout".into()));
    }
    ensure_buffers(l, projector, style)?;

    let size = projector.buffers.as_ref().expect("allocated above").size;
    let surface = projector.surface.clone();

    // Anchor TOP|LEFT so margins are exact usable-area offsets (anchoring
    // trap, spec §4.1.4).
    projector.layer_surface.set_size(size, size);
    projector.layer_surface.set_anchor(Anchor::Top | Anchor::Left);
    let left_f = pos.x.round();
    let top_f = pos.y.round();
    if !left_f.is_finite() || !top_f.is_finite()
        || left_f < i32::MIN as f64 || left_f > i32::MAX as f64
        || top_f < i32::MIN as f64 || top_f > i32::MAX as f64
    {
        return Err(MarkerError::Backend("marker coordinate is outside protocol range".into()));
    }
    let left = left_f as i32;
    let top = top_f as i32;
    projector.layer_surface.set_margin(top, 0, 0, left);
    let generation = l.st.configure_generation;
    surface.commit();
    l.wait_for(Duration::from_secs(2), |st| st.closed || st.configure_generation > generation)
        .map_err(|_| MarkerError::Timeout)?;
    if l.st.closed {
        return Err(MarkerError::Closed);
    }

    attach_and_settle(l, projector, true)
}

/// Removes the projected marker and waits until the removal is presented.
pub(crate) fn clear(l: &mut Loop, projector: &mut Projector) -> Result<(), MarkerError> {
    if projector.destroyed {
        return Ok(()); // teardown already happened; nothing to clear
    }
    if projector.buffers.is_none() {
        return Ok(()); // never shown
    }
    attach_and_settle(l, projector, false)
}

fn attach_and_settle(
    l: &mut Loop,
    projector: &mut Projector,
    marker_visible: bool,
) -> Result<(), MarkerError> {
    let surface = projector.surface.clone();
    let size = projector.buffers.as_ref().expect("buffers allocated").size;

    for _ in 0..SETTLE_FRAMES {
        let buffers = projector.buffers.as_ref().expect("buffers allocated");
        let buf = if marker_visible { &buffers.marker } else { &buffers.clear };
        let state = if marker_visible { l.st.marker_buffer_state } else { l.st.clear_buffer_state };
        if !matches!(state, BufferState::Idle | BufferState::Released) {
            return Err(MarkerError::Backend("marker buffer is still in use".into()));
        }
        if marker_visible {
            l.st.marker_buffer = Some(buf.clone());
            l.st.marker_buffer_state = BufferState::Submitted;
        } else {
            l.st.clear_buffer = Some(buf.clone());
            l.st.clear_buffer_state = BufferState::Submitted;
        }

        // frame 请求必须与 attach 同一批次先于 commit 发出；否则后续裸提交
        // 按 Wayland 语义等价于 attach(null)，会分离 buffer，marker 在
        // show() 返回前就已消失。
        surface.attach(Some(buf), 0, 0);
        surface.damage_buffer(0, 0, size as i32, size as i32);
        surface.frame(&l.st.qh, ());
        surface.commit();

        let target = l.st.cb_done + 1;
        l.st.cb_target = target;
        l.wait_for(Duration::from_secs(2), |st| st.cb_done >= target || st.closed)
            .map_err(|_| {
                projector.poisoned = true;
                MarkerError::Timeout
            })?;
        if l.st.closed {
            projector.poisoned = true;
            return Err(MarkerError::Closed);
        }
        // A frame callback only reports presentation. The attached wl_buffer
        // remains compositor-owned until its independent release event.
        l.wait_for(Duration::from_secs(2), |st| {
            let released = if marker_visible {
                st.marker_buffer_state == BufferState::Released
            } else {
                st.clear_buffer_state == BufferState::Released
            };
            released || st.closed
        })
        .map_err(|_| {
            projector.poisoned = true;
            MarkerError::Timeout
        })?;
        if l.st.closed {
            projector.poisoned = true;
            return Err(MarkerError::Closed);
        }
    }
    Ok(())
}

fn ensure_buffers(
    l: &mut Loop,
    projector: &mut Projector,
    style: &MarkerStyle,
) -> Result<(), MarkerError> {
    if !style.size_logical.is_finite() || style.size_logical <= 0.0 {
        return Err(MarkerError::Backend("marker size must be finite and positive".into()));
    }
    let size_f = style.size_logical.ceil();
    if size_f > i32::MAX as f64 || size_f > u32::MAX as f64 {
        return Err(MarkerError::Backend("marker size exceeds protocol limits".into()));
    }
    let size = size_f as u32;
    if let Some(b) = projector.buffers.as_ref() {
        if b.size == size && b.rgba == style.rgba {
            return Ok(());
        }
        if !matches!(l.st.marker_buffer_state, BufferState::Idle | BufferState::Released)
            || !matches!(l.st.clear_buffer_state, BufferState::Idle | BufferState::Released)
        {
            return Err(MarkerError::Backend(
                "cannot replace marker buffers before wl_buffer.release".into(),
            ));
        }
    }

    let shm: wl_shm::WlShm = l
        .st
        .shm
        .clone()
        .ok_or_else(|| MarkerError::Backend("wl_shm not bound".into()))?;
    let stride = (size as usize)
        .checked_mul(4)
        .ok_or_else(|| MarkerError::Backend("marker stride overflow".into()))?;
    let plane = stride
        .checked_mul(size as usize)
        .ok_or_else(|| MarkerError::Backend("marker plane overflow".into()))?;
    let pool_size = plane
        .checked_mul(2)
        .ok_or_else(|| MarkerError::Backend("marker pool overflow".into()))?;
    if stride > i32::MAX as usize || plane > i32::MAX as usize {
        return Err(MarkerError::Backend("marker geometry exceeds protocol limits".into()));
    }
    let mut shm_pool =
        ShmPool::create(&shm, pool_size, &l.st.qh).map_err(|e| MarkerError::Backend(e.to_string()))?;

    // Solid marker half; the transparent half stays zeroed.
    let mmap = shm_pool.mmap.as_mut();
    for i in 0..plane / 4 {
        mmap[i * 4..i * 4 + 4].copy_from_slice(&argb_bytes(style.rgba));
    }

    let marker = shm_pool.pool.create_buffer(
        0,
        size as i32,
        size as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        &l.st.qh,
        (),
    );
    let clear = shm_pool.pool.create_buffer(
        plane as i32,
        size as i32,
        size as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        &l.st.qh,
        (),
    );

    if let Some(old) = projector
        .buffers
        .replace(MarkerBuffers { shm_pool, marker, clear, size, rgba: style.rgba })
    {
        old.marker.destroy();
        old.clear.destroy();
        old.shm_pool.destroy(&l.conn);
    }
    Ok(())
}

/// Converts `[r, g, b, a]` into the little-endian byte order of ARGB8888.
fn argb_bytes(rgba: [u8; 4]) -> [u8; 4] {
    [rgba[2], rgba[1], rgba[0], rgba[3]]
}
