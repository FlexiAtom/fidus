// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! The tracking-target contract (C layer, spec §3.2 / §6.1).
//!
//! The caller tells fidus *what* to locate by handing over its own offscreen
//! render. Both types are pure visual inputs: nothing here reads or wraps a
//! platform coordinate, so runtime target registration cannot violate the
//! zero-trust rule.

use crate::coord::LogicalPoint;
use crate::io::Frame;

/// A tightly-packed 8-bit RGBA image, row-major, 4 bytes per pixel.
///
/// Used for the caller's target render (and reusable wherever fidus needs a
/// plain pixel buffer outside the capture [`Frame`] strides).
#[derive(Clone, Debug, PartialEq)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl RgbaImage {
    /// Width in pixels.
    pub fn width(&self) -> u32 { self.width }

    /// Height in pixels.
    pub fn height(&self) -> u32 { self.height }

    /// Number of stored pixel bytes.
    pub fn byte_len(&self) -> usize { self.data.len() }

    /// Reports whether dimensions and storage describe one tightly-packed RGBA image.
    pub fn is_valid(&self) -> bool {
        (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .is_some_and(|expected| expected == self.data.len() && self.width != 0 && self.height != 0)
    }

    /// Creates an image from raw tightly-packed bytes.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Self {
        // Keep this constructor total for public/untrusted image input. An
        // invalid buffer has no pixels; accessors and matchers then reject it
        // instead of panicking on a forged length or overflowing arithmetic.
        let expected = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4));
        if expected != Some(data.len()) {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        }
        RgbaImage { width, height, data }
    }

    /// Reads the pixel at `(x, y)`; out-of-bounds reads are black-transparent.
    pub fn rgba(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0, 0, 0, 0];
        }
        let i = (y as usize)
            .checked_mul(self.width as usize)
            .and_then(|n| n.checked_add(x as usize))
            .and_then(|n| n.checked_mul(4));
        match i {
            Some(i) if i.checked_add(4).is_some_and(|end| end <= self.data.len()) => {
                [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
            }
            _ => [0, 0, 0, 0],
        }
    }

    /// Rec.709 luma of a pixel, in `[0, 255]`.
    pub fn luma_at(&self, x: u32, y: u32) -> f32 {
        let [r, g, b, _] = self.rgba(x, y);
        0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
    }

    /// Copies a [`Frame`] (arbitrary stride/format) into a tightly-packed
    /// image.
    pub fn from_frame(frame: &Frame) -> Self {
        if !frame.is_valid() {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        }
        let Some(capacity) = (frame.width as usize)
            .checked_mul(frame.height as usize)
            .and_then(|n| n.checked_mul(4)) else {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        };
        let mut data = Vec::with_capacity(capacity);
        for y in 0..frame.height {
            for x in 0..frame.width {
                data.extend_from_slice(&frame.rgba_at(x, y));
            }
        }
        RgbaImage { width: frame.width, height: frame.height, data }
    }

    /// Bilinear resample to a new pixel size.
    pub fn resample(&self, new_width: u32, new_height: u32) -> Self {
        let Some(source_len) = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4)) else {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        };
        if new_width == 0 || new_height == 0 || self.width == 0 || self.height == 0 || self.data.len() != source_len {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        }
        let Some(out_len) = (new_width as usize)
            .checked_mul(new_height as usize)
            .and_then(|n| n.checked_mul(4)) else {
            return RgbaImage { width: 0, height: 0, data: Vec::new() };
        };
        let mut out = vec![0u8; out_len];
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
                    let Some(pixel) = (y as usize)
                        .checked_mul(new_width as usize)
                        .and_then(|n| n.checked_add(x as usize))
                        .and_then(|n| n.checked_mul(4))
                        .and_then(|n| n.checked_add(c)) else {
                        return RgbaImage { width: 0, height: 0, data: Vec::new() };
                    };
                    out[pixel] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        RgbaImage { width: new_width, height: new_height, data: out }
    }
}

/// How to handle a target whose appearance cannot be reliably located.
///
/// # Why an opt-in and not a flag that silences the check
///
/// Some appearances are genuinely untrackable: a linear gradient correlates
/// 1.000 with a shifted copy of itself, so template matching returns a
/// confident score at an *arbitrary* position. The default is to refuse such
/// a target outright, because a fabricated position entering the pool is the
/// exact failure spec §6.1 exists to prevent.
///
/// But refusal is not always the caller's best option: a consumer that has
/// no better render available may prefer a *degraded, honestly-labelled*
/// track over none at all. That is what [`Self::TrackWithReducedConfidence`]
/// is for — and note what it does **not** do. It never skips the check and it
/// never lets a fabricated position look trustworthy: measurements from an
/// ambiguous template are emitted with their confidence scaled down by how
/// ambiguous the template actually measured, so L7 fusion and the Kalman
/// filter weight them accordingly, and a caller watching `confidence` sees
/// the degradation. Escaping the refusal costs credibility, not honesty.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UntrackablePolicy {
    /// Refuse registration with `EstimateError::UntrackableTarget`.
    ///
    /// The default: an untrackable appearance is a caller bug that is far
    /// cheaper to fix at registration than to diagnose later from a track
    /// that wanders for reasons nothing reports.
    #[default]
    Refuse,
    /// Accept the target, but permanently scale down the confidence of every
    /// measurement derived from it.
    ///
    /// The caller is asserting "I know this render is ambiguous and I want a
    /// best-effort track anyway". fidus keeps its side of spec §6.1 by making
    /// the resulting uncertainty explicit rather than hiding it: nothing in
    /// the pool is fabricated, it is merely *labelled as weak*.
    ///
    /// *Failure mode*: a caller that ignores `confidence` and treats every
    /// position as exact gets a wandering target. Contained only by
    /// documentation — a caller that opts in has explicitly taken this on.
    TrackWithReducedConfidence,
}

/// What the estimator should track.
///
/// The template is the caller's **own offscreen render** of the target — a
/// pure visual input, exactly the kind of measurement the probability pool
/// accepts (spec §6.1). Nothing here reads or wraps a platform coordinate.
#[derive(Clone, Debug)]
pub struct TargetDescription {
    /// Target appearance in **logical pixels** (the caller renders its window
    /// at logical size; the estimator resamples to the capture scale using
    /// the calibrated frame).
    pub template_logical: RgbaImage,
    /// Where the caller believes it placed the target (center, logical
    /// coordinates of the calibrated frame). `None` means "search the whole
    /// screen on the first estimate".
    ///
    /// This is the caller's own belief about its own drawing — it is not, and
    /// cannot be, a value read from a platform window API.
    pub initial_center: Option<LogicalPoint>,
    /// What to do when the appearance cannot be reliably located.
    ///
    /// Defaults to [`UntrackablePolicy::Refuse`]; opting out has to be
    /// written down at the call site, which is the point.
    pub untrackable_policy: UntrackablePolicy,
}

impl TargetDescription {
    /// A target tracked under the default policy (refuse if untrackable).
    pub fn new(template_logical: RgbaImage) -> Self {
        TargetDescription {
            template_logical,
            initial_center: None,
            untrackable_policy: UntrackablePolicy::Refuse,
        }
    }

    /// Sets the caller's belief about where the target currently is.
    pub fn with_initial_center(mut self, center: LogicalPoint) -> Self {
        self.initial_center = Some(center);
        self
    }

    /// Opts into best-effort tracking of an ambiguous appearance, accepting
    /// reduced confidence on every resulting measurement.
    ///
    /// Read [`UntrackablePolicy::TrackWithReducedConfidence`] before using
    /// this: prefer supplying a more distinctive render when one exists.
    pub fn tracking_ambiguous_appearance(mut self) -> Self {
        self.untrackable_policy = UntrackablePolicy::TrackWithReducedConfidence;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::RgbaImage;

    #[test]
    fn malformed_raw_image_is_empty_instead_of_panicking() {
        let image = RgbaImage::from_raw(u32::MAX, 2, vec![1, 2, 3]);
        assert_eq!((image.width(), image.height()), (0, 0));
        assert!(!image.is_valid());
    }
}
