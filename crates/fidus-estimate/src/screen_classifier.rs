// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! ScreenClassifier — automatic dynamic-background detection (spec v0.4 L8
//! "动态壁纸检查", carried into v0.5.1 as the source of
//! `EnvironmentContext::is_dynamic_wallpaper`).
//!
//! Two captures ~200 ms apart, luma-differenced over the central region of
//! the screen (the outer band is excluded so panels / taskbars / clocks do
//! not vote), sampled on a coarse grid. A changed fraction above 15 % means
//! the background animates: the gate degrades the calibrators' expected
//! confidence and callers may pick a quieter moment.
//!
//! This is a *classification of fidus' own captures* — pure vision, nothing
//! read from the platform — and its verdict is environment knowledge, never
//! a coordinate. It therefore lives outside the probability pool and only
//! ever touches [`EnvironmentContext`](fidus_core::env::EnvironmentContext).
//!
//! What it cannot distinguish: a video playing in a window and an animated
//! wallpaper both read as "dynamic". That is the honest answer to the
//! question the gate asks ("will the background change between my baseline
//! and my capture?"), so no attempt is made to tell them apart.

use std::time::Duration;

use fidus_core::io::{CaptureError, CaptureIo, Frame};

/// Verdict of one classification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallpaperVerdict {
    /// Whether the background is considered animated.
    pub dynamic: bool,
    /// Fraction of sampled central pixels that changed between the two
    /// captures, in `[0, 1]`.
    pub changed_ratio: f64,
    /// Number of pixels sampled (diagnostics; `0` means the crop was empty).
    pub samples: usize,
}

/// Tuning knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenClassifier {
    /// Time between the two captures (spec: 200 ms).
    pub interval: Duration,
    /// Central crop kept for the comparison, as a fraction of each
    /// dimension (spec: 84 %, i.e. an 8 % band excluded on every edge).
    pub crop_fraction: f64,
    /// Luma difference (0–255) above which a sample counts as changed.
    pub change_threshold: f32,
    /// Changed fraction above which the background is dynamic (spec: 15 %).
    pub dynamic_ratio: f64,
    /// Sampling stride in pixels (every `stride`-th pixel on both axes).
    pub stride: u32,
    /// Consecutive capture pairs to examine; the verdict is the **worst**
    /// (largest change ratio) of them. Animation is bursty — a single
    /// 200 ms pair measured 8–17 % on a live full-screen test pattern,
    /// flickering around the threshold — and the gate's question is
    /// "can the background change between my baseline and my capture?",
    /// to which "yes, sometimes" means *yes*.
    pub windows: u32,
}

impl Default for ScreenClassifier {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(200),
            crop_fraction: 0.84,
            change_threshold: 10.0,
            dynamic_ratio: 0.15,
            stride: 4,
            windows: 3,
        }
    }
}

impl ScreenClassifier {
    /// Captures `windows + 1` frames through `io`, sleeping `interval`
    /// between captures with `sleep` (injectable so tests do not wait),
    /// compares each consecutive pair and reports the worst verdict.
    pub fn classify(
        &self,
        io: &mut dyn CaptureIo,
        mut sleep: impl FnMut(Duration),
    ) -> Result<WallpaperVerdict, CaptureError> {
        let mut prev = io.capture()?;
        let mut worst = WallpaperVerdict { dynamic: false, changed_ratio: 0.0, samples: 0 };
        for _ in 0..self.windows.max(1) {
            sleep(self.interval);
            let next = io.capture()?;
            let v = self.compare(&prev, &next);
            if v.changed_ratio >= worst.changed_ratio {
                worst = v;
            }
            prev = next;
        }
        Ok(worst)
    }

    /// Blocking convenience: [`classify`](Self::classify) with
    /// `std::thread::sleep`.
    pub fn classify_blocking(&self, io: &mut dyn CaptureIo) -> Result<WallpaperVerdict, CaptureError> {
        self.classify(io, std::thread::sleep)
    }

    /// Pure comparison of two frames (the classification core; exposed for
    /// tests and for callers that already hold two captures).
    ///
    /// Frames of different size cannot be compared and are reported as
    /// dynamic with `changed_ratio = 1.0` — a geometry change between two
    /// captures 200 ms apart *is* an unstable screen.
    pub fn compare(&self, a: &Frame, b: &Frame) -> WallpaperVerdict {
        // A malformed capture is not evidence of a static or dynamic screen.
        // Treating unreadable bytes as black would silently manufacture a
        // classification; fail closed by reporting the environment unstable.
        if !a.is_valid() || !b.is_valid() {
            return WallpaperVerdict { dynamic: true, changed_ratio: 1.0, samples: 0 };
        }
        if a.size() != b.size() {
            return WallpaperVerdict { dynamic: true, changed_ratio: 1.0, samples: 0 };
        }
        let (w, h) = a.size();
        let crop = self.crop_fraction.clamp(0.0, 1.0);
        let band_x = ((w as f64) * (1.0 - crop) / 2.0).round() as u32;
        let band_y = ((h as f64) * (1.0 - crop) / 2.0).round() as u32;
        let (x0, x1) = (band_x, w.saturating_sub(band_x));
        let (y0, y1) = (band_y, h.saturating_sub(band_y));
        let stride = self.stride.max(1);

        let mut samples = 0usize;
        let mut changed = 0usize;
        let mut y = y0;
        while y < y1 {
            let mut x = x0;
            while x < x1 {
                let la = luma(a.rgba_at(x, y));
                let lb = luma(b.rgba_at(x, y));
                samples += 1;
                if (la - lb).abs() > self.change_threshold {
                    changed += 1;
                }
                x += stride;
            }
            y += stride;
        }
        let changed_ratio = if samples == 0 { 0.0 } else { changed as f64 / samples as f64 };
        WallpaperVerdict { dynamic: changed_ratio > self.dynamic_ratio, changed_ratio, samples }
    }
}

fn luma([r, g, b, _]: [u8; 4]) -> f32 {
    0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidus_core::io::PixelFormat;

    const W: u32 = 400;
    const H: u32 = 300;

    fn textured(seed: u32) -> Frame {
        let mut data = vec![0u8; (W * H * 4) as usize];
        for y in 0..H {
            for x in 0..W {
                let v = ((x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ seed) % 200) as u8;
                PixelFormat::Xrgb8888.write_rgba(&mut data, ((y * W + x) * 4) as usize, [v, v, v, 255]);
            }
        }
        Frame { width: W, height: H, stride: W * 4, format: PixelFormat::Xrgb8888, data }
    }

    fn paint(frame: &mut Frame, x0: u32, y0: u32, x1: u32, y1: u32, v: u8) {
        for y in y0..y1.min(H) {
            for x in x0..x1.min(W) {
                PixelFormat::Xrgb8888.write_rgba(&mut frame.data, ((y * W + x) * 4) as usize, [v, v, v, 255]);
            }
        }
    }

    #[test]
    fn identical_frames_are_static() {
        let c = ScreenClassifier::default();
        let v = c.compare(&textured(1), &textured(1));
        assert!(!v.dynamic);
        assert_eq!(v.changed_ratio, 0.0);
        assert!(v.samples > 1000);
    }

    #[test]
    fn full_screen_animation_is_dynamic() {
        let c = ScreenClassifier::default();
        let v = c.compare(&textured(1), &textured(0xFF));
        assert!(v.dynamic, "ratio = {}", v.changed_ratio);
        assert!(v.changed_ratio > 0.5);
    }

    #[test]
    fn cursor_sized_change_is_static() {
        let c = ScreenClassifier::default();
        let a = textured(1);
        let mut b = textured(1);
        paint(&mut b, 200, 150, 224, 174, 255); // a 24×24 cursor moved in
        let v = c.compare(&a, &b);
        assert!(!v.dynamic, "ratio = {}", v.changed_ratio);
    }

    #[test]
    fn animated_taskbar_band_is_excluded_by_the_crop() {
        // The bottom 6 % of the screen animates (a clock / a busy panel);
        // that band lies inside the excluded 8 % border, so the verdict
        // must stay static.
        let c = ScreenClassifier::default();
        let a = textured(1);
        let mut b = textured(1);
        paint(&mut b, 0, (H as f64 * 0.94) as u32, W, H, 255);
        let v = c.compare(&a, &b);
        assert!(!v.dynamic, "ratio = {}", v.changed_ratio);
        assert_eq!(v.changed_ratio, 0.0);
    }

    #[test]
    fn size_change_between_captures_is_dynamic() {
        let c = ScreenClassifier::default();
        let a = textured(1);
        let b = Frame { width: W / 2, height: H, stride: W / 2 * 4, format: PixelFormat::Xrgb8888, data: vec![0; (W / 2 * H * 4) as usize] };
        let v = c.compare(&a, &b);
        assert!(v.dynamic);
        assert_eq!(v.samples, 0);
    }

    #[test]
    fn classify_captures_twice_and_sleeps_the_interval() {
        struct Flip(u32);
        impl CaptureIo for Flip {
            fn capture(&mut self) -> Result<Frame, CaptureError> {
                self.0 += 1;
                Ok(textured(self.0 * 0x55)) // clearly different textures
            }
        }
        let c = ScreenClassifier::default();
        let mut io = Flip(0);
        let mut slept = Vec::new();
        let v = c.classify(&mut io, |d| slept.push(d)).expect("captures");
        assert_eq!(io.0, c.windows + 1);
        assert_eq!(slept, vec![Duration::from_millis(200); c.windows as usize]);
        assert!(v.dynamic);
    }

    #[test]
    fn bursty_animation_is_caught_by_the_worst_window() {
        // Static, static, then one animated pair: the verdict must be the
        // worst pair, not the last or the mean.
        struct Burst(u32);
        impl CaptureIo for Burst {
            fn capture(&mut self) -> Result<Frame, CaptureError> {
                self.0 += 1;
                Ok(textured(if self.0 == 4 { 0xAA } else { 1 }))
            }
        }
        let c = ScreenClassifier::default();
        let v = c.classify(&mut Burst(0), |_| {}).expect("captures");
        assert!(v.dynamic, "ratio = {}", v.changed_ratio);
    }
}
