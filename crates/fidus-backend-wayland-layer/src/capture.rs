//! Screen capture via `zwlr_screencopy_manager_v1` — the generic capture
//! primitive (spec §1.1 原则二：通用截屏). It copies composited output pixels
//! into an `wl_shm` buffer; it reads no window geometry of any kind.

use std::time::Duration;

use wayland_client::protocol::{wl_buffer, wl_shm};
use fidus_core::io::{CaptureError, Frame, PixelFormat};

use crate::session::Loop;
use crate::shm::ShmPool;

/// How long to wait for screencopy events before giving up.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(2);

/// Reusable capture buffer: one pool, one buffer, sized to the last frame.
/// Reuse is safe once the compositor has sent `ready` for the previous copy.
pub(crate) struct CopyBuffer {
    shm_pool: ShmPool,
    buffer: wl_buffer::WlBuffer,
    /// (format, width, height, stride) the buffer was created for.
    params: (PixelFormat, u32, u32, u32),
}

/// Captures the first bound output into a top-down [`Frame`].
pub(crate) fn capture(l: &mut Loop, cache: &mut Option<CopyBuffer>) -> Result<Frame, CaptureError> {
    let manager = l
        .st
        .screencopy
        .clone()
        .ok_or_else(|| CaptureError::Backend("screencopy manager not bound".into()))?;
    let output = l
        .st
        .outputs
        .first()
        .cloned()
        .ok_or_else(|| CaptureError::Backend("no output bound".into()))?;

    l.st.reset_screencopy();
    let frame = manager.capture_output(0, &output, &l.st.qh, ());

    // The compositor announces the buffer geometry it will copy into.
    l.wait_for(CAPTURE_TIMEOUT, |st| st.cp_size.is_some())
        .map_err(|_| CaptureError::Timeout)?;
    let (width, height, stride) = l.st.cp_size.expect("checked above");
    let format = l
        .st
        .cp_format
        .expect("format event precedes size availability")
        .ok_or(CaptureError::UnsupportedFormat(-1))?;

    let byte_size = stride as usize * height as usize;
    if byte_size == 0 || width == 0 || height == 0 {
        return Err(CaptureError::Failed("compositor announced empty frame".into()));
    }

    // Rebuild the pool only when the announced geometry changed.
    let reuse = cache
        .as_ref()
        .is_some_and(|c| c.params == (format, width, height, stride));
    if !reuse {
        let shm = l
            .st
            .shm
            .clone()
            .ok_or_else(|| CaptureError::Backend("wl_shm not bound".into()))?;
        let shm_pool = ShmPool::create(&shm, byte_size, &l.st.qh)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        let buffer = shm_pool.pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            shm_format_of(format),
            &l.st.qh,
            (),
        );
        *cache = Some(CopyBuffer { shm_pool, buffer, params: (format, width, height, stride) });
    }
    let cached = cache.as_mut().expect("just created");

    frame.copy(&cached.buffer);
    l.wait_for(CAPTURE_TIMEOUT, |st| st.cp_ready || st.cp_failed)
        .map_err(|_| CaptureError::Timeout)?;
    if l.st.cp_failed {
        return Err(CaptureError::Failed("compositor refused the copy".into()));
    }

    let src = cached.shm_pool.mmap.as_slice().to_vec();
    frame.destroy();
    let data = if l.st.cp_y_invert {
        flip_rows(&src, height, stride)
    } else {
        src
    };

    Ok(Frame { width, height, stride, format, data })
}

/// Flips rows bottom-up when the compositor announces `Y_INVERT`.
fn flip_rows(src: &[u8], height: u32, stride: u32) -> Vec<u8> {
    let row = stride as usize;
    let mut out = vec![0u8; src.len()];
    for y in 0..height as usize {
        let s = (height as usize - 1 - y) * row;
        let d = y * row;
        out[d..d + row].copy_from_slice(&src[s..s + row]);
    }
    out
}

fn shm_format_of(format: PixelFormat) -> wl_shm::Format {
    match format {
        PixelFormat::Argb8888 => wl_shm::Format::Argb8888,
        PixelFormat::Xrgb8888 => wl_shm::Format::Xrgb8888,
        PixelFormat::Abgr8888 => wl_shm::Format::Abgr8888,
        PixelFormat::Xbgr8888 => wl_shm::Format::Xbgr8888,
    }
}

#[cfg(test)]
mod tests {
    use super::flip_rows;

    #[test]
    fn flip_rows_reverses_row_order() {
        // 3 rows of 4 bytes.
        let src: Vec<u8> = (0..3usize)
            .flat_map(|row| (0..4usize).map(move |i| (row * 10 + i) as u8))
            .collect();
        let flipped = flip_rows(&src, 3, 4);
        assert_eq!(&flipped[0..4], &src[8..12], "top row becomes the old bottom row");
        assert_eq!(&flipped[8..12], &src[0..4]);
        assert_eq!(flipped.len(), src.len());
    }

    #[test]
    #[should_panic]
    fn flip_rows_rejects_height_beyond_buffer() {
        // Regression guard: the call site must pass the *row count*, not the
        // width — a width/height swap panics here instead of corrupting data.
        flip_rows(&[0u8; 4 * 3], 4, 4);
    }
}
