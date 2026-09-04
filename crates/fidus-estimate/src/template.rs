//! L1 template tracking: normalized cross-correlation over a region of
//! interest, coarse-to-fine with subpixel refinement.
//!
//! NCC on luma is robust to global brightness shifts and to arbitrary
//! background: only the target's own texture determines the score, so a
//! dynamic wallpaper degrades nothing as long as the target itself is
//! visible and unchanged.

use fidus_core::coord::PhysicalPoint;
use fidus_core::io::Frame;

use crate::image::RgbaImage;

/// One successful template match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemplateMatch {
    /// Matched template center in capture pixels (subpixel).
    pub center: PhysicalPoint,
    /// NCC score in `[-1, 1]`; higher is better. Above ~0.35 is a usable
    /// measurement for textured templates.
    pub score: f64,
}

/// A search window: center plus half-extent, in capture pixels.
#[derive(Clone, Copy, Debug)]
pub struct SearchRoi {
    /// ROI center in capture pixels.
    pub center: (f64, f64),
    /// Half-extent in capture pixels.
    pub half: f64,
}

/// A luma plane sampled on a fixed grid, with NCC statistics precomputed.
struct SampledTemplate {
    /// `(template pixel index, luma)` pairs on the sampling grid.
    samples: Vec<(usize, f32)>,
    /// Mean luma over the samples.
    mean: f32,
    /// `sqrt(Σ (t - mean)²)` over the samples.
    norm: f32,
    /// Template width/height in pixels.
    size: (u32, u32),
}

impl SampledTemplate {
    fn build(template: &RgbaImage, step: u32) -> Self {
        let (tw, th) = (template.width, template.height);
        let mut samples = Vec::new();
        let mut y = 0;
        while y < th {
            let mut x = 0;
            while x < tw {
                samples.push(((y * tw + x) as usize, template.luma_at(x, y)));
                x += step;
            }
            y += step;
        }
        let mean = samples.iter().map(|s| s.1).sum::<f32>() / samples.len() as f32;
        let norm = samples.iter().map(|s| (s.1 - mean).powi(2)).sum::<f32>().sqrt();
        SampledTemplate { samples, mean, norm, size: (tw, th) }
    }
}

/// Computes the tightly-packed luma plane of `frame`.
fn luma_plane(frame: &Frame) -> (u32, u32, Vec<f32>) {
    let mut p = Vec::with_capacity(frame.width as usize * frame.height as usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let [r, g, b, _] = frame.rgba_at(x, y);
            p.push(0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32);
        }
    }
    (frame.width, frame.height, p)
}

/// NCC of `tpl` placed with its top-left at `(ox, oy)` in the frame plane.
fn ncc(
    plane: &(u32, u32, Vec<f32>),
    tpl: &SampledTemplate,
    ox: i64,
    oy: i64,
) -> Option<f64> {
    let (fw, fh, px) = plane;
    let (tw, th) = tpl.size;
    if ox < 0 || oy < 0 || ox + tw as i64 > *fw as i64 || oy + th as i64 > *fh as i64 {
        return None;
    }

    // Pass 1: window mean over the sampled grid.
    let (mut fsum, mut fcount) = (0.0f32, 0.0f32);
    for &(ti, _) in &tpl.samples {
        let (tx, ty) = (ti % tw as usize, ti / tw as usize);
        let f = px[((oy + ty as i64) * *fw as i64 + ox + tx as i64) as usize];
        fsum += f;
        fcount += 1.0;
    }
    let fmean = fsum / fcount;

    // Pass 2: correlation numerator and window variance.
    let (mut num, mut den_f) = (0.0f32, 0.0f32);
    for &(ti, t) in &tpl.samples {
        let (tx, ty) = (ti % tw as usize, ti / tw as usize);
        let f = px[((oy + ty as i64) * *fw as i64 + ox + tx as i64) as usize];
        num += (f - fmean) * (t - tpl.mean);
        den_f += (f - fmean) * (f - fmean);
    }
    let den = (den_f.sqrt() * tpl.norm).max(1e-6);
    Some((num / den) as f64)
}

/// Finds `template` inside `roi` of `frame`.
///
/// Phase 1 scans the ROI on a coarse grid with subsampled pixels; phase 2
/// refines around the best candidate at full resolution; phase 3 fits a 2D
/// parabola for subpixel precision.
pub fn match_template(frame: &Frame, template: &RgbaImage, roi: SearchRoi) -> Option<TemplateMatch> {
    let plane = luma_plane(frame);
    let (fw, fh) = (plane.0, plane.1);
    let (tw, th) = (template.width, template.height);
    if tw == 0 || th == 0 || tw + 2 > fw || th + 2 > fh {
        return None;
    }

    let coarse_tpl = SampledTemplate::build(template, 3);
    let fine_tpl = SampledTemplate::build(template, 1);

    // Clamp the ROI to positions where the template fits inside the frame.
    let clamp = |v: f64, hi: f64| v.clamp(0.0, hi);
    let x_lo = clamp((roi.center.0 - roi.half).floor(), (fw - tw) as f64) as i64;
    let x_hi = clamp((roi.center.0 + roi.half).ceil(), (fw - tw) as f64) as i64;
    let y_lo = clamp((roi.center.1 - roi.half).floor(), (fh - th) as f64) as i64;
    let y_hi = clamp((roi.center.1 + roi.half).ceil(), (fh - th) as f64) as i64;

    // Phase 1: coarse scan.
    let step = 3i64;
    let mut best: Option<(i64, i64, f64)> = None;
    let mut oy = y_lo;
    while oy <= y_hi {
        let mut ox = x_lo;
        while ox <= x_hi {
            if let Some(score) = ncc(&plane, &coarse_tpl, ox, oy) {
                if best.is_none_or(|b| score > b.2) {
                    best = Some((ox, oy, score));
                }
            }
            ox += step;
        }
        oy += step;
    }
    let (mut bx, mut by, mut bs) = best?;

    // Phase 2: full-resolution refinement around the coarse peak.
    let r = 3i64;
    for oy in (by - r).max(0)..=(by + r) {
        for ox in (bx - r).max(0)..=(bx + r) {
            if let Some(score) = ncc(&plane, &fine_tpl, ox, oy) {
                if score > bs {
                    (bx, by, bs) = (ox, oy, score);
                }
            }
        }
    }

    // Phase 3: subpixel via 2D parabola over neighbor scores.
    let mut cx = bx as f64 + tw as f64 / 2.0;
    let mut cy = by as f64 + th as f64 / 2.0;
    if let (Some(l), Some(rr)) = (ncc(&plane, &fine_tpl, bx - 1, by), ncc(&plane, &fine_tpl, bx + 1, by)) {
        let denom = l - 2.0 * bs + rr;
        if denom.abs() > 1e-9 {
            cx += 0.5 * (l - rr) / denom;
        }
    }
    if let (Some(u), Some(d)) = (ncc(&plane, &fine_tpl, bx, by - 1), ncc(&plane, &fine_tpl, bx, by + 1)) {
        let denom = u - 2.0 * bs + d;
        if denom.abs() > 1e-9 {
            cy += 0.5 * (u - d) / denom;
        }
    }

    Some(TemplateMatch { center: PhysicalPoint::new(cx, cy), score: bs })
}
