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

use crate::session::{Loop, Session};
use crate::shm::ShmPool;

/// How long to wait for the initial configure before giving up.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long to drain events after a state change before attaching a buffer.
const ACK_DRAIN: u64 = 60;
/// Frame callbacks awaited per presentation (belt and braces).
const SETTLE_FRAMES: u64 = 2;

/// The projected overlay surface and its buffers.
pub(crate) struct Projector {
    surface: wl_surface::WlSurface,
    layer_surface: ZwlrLayerSurfaceV1,
    buffers: Option<MarkerBuffers>,
    destroyed: bool,
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

    l.wait_for(CONFIGURE_TIMEOUT, |st: &Session| st.configure_seen || st.closed)
        .map_err(|_| MarkerError::Timeout)?;
    if l.st.closed {
        return Err(MarkerError::Closed);
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
        return Err(MarkerError::Backend("could not determine usable-area hint".into()));
    }

    let projector = Projector { surface, layer_surface, buffers: None, destroyed: false };
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
pub(crate) fn destroy(l: &mut Loop, projector: &mut Projector) {
    if projector.destroyed {
        return;
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
    let _ = l.conn.flush();
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
    ensure_buffers(l, projector, style)?;

    let size = projector.buffers.as_ref().expect("allocated above").size;
    let surface = projector.surface.clone();

    // Anchor TOP|LEFT so margins are exact usable-area offsets (anchoring
    // trap, spec §4.1.4).
    projector.layer_surface.set_size(size, size);
    projector.layer_surface.set_anchor(Anchor::Top | Anchor::Left);
    projector
        .layer_surface
        .set_margin(pos.y.round() as i32, 0, 0, pos.x.round() as i32);
    surface.commit();
    l.drain(ACK_DRAIN);
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

    {
        let buffers = projector.buffers.as_ref().expect("buffers allocated");
        let buf = if marker_visible { &buffers.marker } else { &buffers.clear };
        surface.attach(Some(buf), 0, 0);
        surface.damage_buffer(0, 0, size as i32, size as i32);
    }
    surface.commit();

    // Presentation sync: request a frame callback *before* each commit so it
    // fires for the repaint that includes this buffer.
    for _ in 0..SETTLE_FRAMES {
        l.st.cb_target = l.st.cb_done + 1;
        surface.frame(&l.st.qh, ());
        surface.commit();
        let target = l.st.cb_target;
        l.wait_for(Duration::from_secs(2), |st| st.cb_done >= target || st.closed)
            .map_err(|_| MarkerError::Timeout)?;
        if l.st.closed {
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
    let size = style.size_logical.ceil().max(1.0) as u32;
    if let Some(b) = projector.buffers.as_ref() {
        if b.size == size && b.rgba == style.rgba {
            return Ok(());
        }
    }

    let shm: wl_shm::WlShm = l
        .st
        .shm
        .clone()
        .ok_or_else(|| MarkerError::Backend("wl_shm not bound".into()))?;
    let stride = size as usize * 4;
    let plane = stride * size as usize;
    let mut shm_pool =
        ShmPool::create(&shm, plane * 2, &l.st.qh).map_err(|e| MarkerError::Backend(e.to_string()))?;

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
