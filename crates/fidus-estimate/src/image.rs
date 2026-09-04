//! Minimal tightly-packed RGBA image with the transforms fidus needs.

use fidus_core::io::Frame;

/// A tightly-packed 8-bit RGBA image, row-major, 4 bytes per pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel bytes, `width * height * 4` long.
    pub data: Vec<u8>,
}

impl RgbaImage {
    /// Creates an image from raw tightly-packed bytes.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width as usize * height as usize * 4);
        RgbaImage { width, height, data }
    }

    /// Reads the pixel at `(x, y)`; out-of-bounds reads are black-transparent.
    pub fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let i = (y * self.width + x) as usize * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// Rec.709 luma of a pixel, in `[0, 255]`.
    pub fn luma_at(&self, x: u32, y: u32) -> f32 {
        let [r, g, b, _] = self.rgba(x, y);
        0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
    }

    /// Copies a [`Frame`] (arbitrary stride/format) into a tightly-packed
    /// image.
    pub fn from_frame(frame: &Frame) -> Self {
        let mut data = Vec::with_capacity(frame.width as usize * frame.height as usize * 4);
        for y in 0..frame.height {
            for x in 0..frame.width {
                data.extend_from_slice(&frame.rgba_at(x, y));
            }
        }
        RgbaImage { width: frame.width, height: frame.height, data }
    }

    /// Bilinear resample to a new pixel size.
    pub fn resample(&self, new_width: u32, new_height: u32) -> Self {
        if new_width == 0 || new_height == 0 {
            return RgbaImage { width: new_width, height: new_height, data: Vec::new() };
        }
        let mut out = vec![0u8; new_width as usize * new_height as usize * 4];
        let sx = self.width as f32 / new_width as f32;
        let sy = self.height as f32 / new_height as f32;
        for y in 0..new_height {
            let fy = (y as f32 + 0.5) * sy - 0.5;
            let y0 = fy.floor().clamp(0.0, self.height as f32 - 1.0) as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let wy = (fy - y0 as f32).clamp(0.0, 1.0);
            for x in 0..new_width {
                let fx = (x as f32 + 0.5) * sx - 0.5;
                let x0 = fx.floor().clamp(0.0, self.width as f32 - 1.0) as u32;
                let x1 = (x0 + 1).min(self.width - 1);
                let wx = (fx - x0 as f32).clamp(0.0, 1.0);
                for c in 0..4 {
                    let p00 = self.rgba(x0, y0)[c] as f32;
                    let p01 = self.rgba(x1, y0)[c] as f32;
                    let p10 = self.rgba(x0, y1)[c] as f32;
                    let p11 = self.rgba(x1, y1)[c] as f32;
                    let v = p00 * (1.0 - wx) * (1.0 - wy)
                        + p01 * wx * (1.0 - wy)
                        + p10 * (1.0 - wx) * wy
                        + p11 * wx * wy;
                    out[(y * new_width + x) as usize * 4 + c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        RgbaImage { width: new_width, height: new_height, data: out }
    }
}
