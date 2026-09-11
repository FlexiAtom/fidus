//! L1 template tracking: normalized cross-correlation over a region of
//! interest, coarse-to-fine with subpixel refinement.
//!
//! NCC on luma is robust to global brightness shifts and to arbitrary
//! background: only the target's own texture determines the score, so a
//! dynamic wallpaper degrades nothing as long as the target itself is
//! visible and unchanged.

use fidus_core::coord::PhysicalPoint;
use fidus_core::io::Frame;
use fidus_core::target::RgbaImage;

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

/// Luma variance floor, in (0–255 luma)² per sample.
///
/// A window flatter than this carries no matchable structure: its NCC
/// denominator is pure quantization noise, and dividing by it manufactures
/// scores near ±1 out of nothing.
///
/// *Failure mode this constant replaced*: the denominator used to be
/// `.max(1e-6)`, so a flat window (variance ~1e-8) produced |score| ≈ 1 — a
/// full-confidence phantom match fed straight into the Kalman filter. A
/// guard must reject, not fabricate (project convention 4). 0.25 ≈ (0.5
/// luma LSB)², i.e. strictly below anything a real 8-bit image can show.
const MIN_VARIANCE_PER_SAMPLE: f64 = 0.25;

/// Downsampling factor of the coarse pyramid level.
const COARSE_STEP: u32 = 3;

/// Coarse candidates carried into the full-resolution refinement.
///
/// More than one because the blurred landscape's summit can sit a step away
/// from the true peak when the target is partly occluded or the background
/// is busy; four is enough for the peak to rank in practice while keeping
/// phase 2 bounded at `TOP_K · (2·step+1)²` fine evaluations.
const TOP_K: usize = 4;

/// A luma plane sampled on a fixed grid, with NCC statistics precomputed.
struct SampledTemplate {
    /// `(x, y, luma)` triples on the sampling grid. The coordinates are
    /// stored rather than recovered with `%` / `/` per sample: the inner
    /// NCC loop runs millions of times per frame.
    samples: Vec<(u32, u32, f32)>,
    /// Mean luma over the samples.
    mean: f64,
    /// `sqrt(Σ (t - mean)²)` over the samples.
    norm: f64,
    /// Template width/height in pixels.
    size: (u32, u32),
}

impl SampledTemplate {
    /// Samples `template` on a `step` grid.
    ///
    /// When `step > 1` each sample is the **mean of its `step`×`step`
    /// block**, not the single pixel at the corner. This is the low-pass
    /// half of a proper image pyramid, and it must match the frame side
    /// (see [`box_downsample`]).
    ///
    /// *Failure mode without it*: point-sampling both sides aliases sharp
    /// content. The NCC peak of a detailed template is narrower than one
    /// pixel, so on a step-3 lattice every coarse position scores like
    /// noise, the true peak does not even rank, and no amount of local
    /// refinement can recover it — the match lands wherever the noise
    /// happened to be highest. Blurring first makes the coarse landscape a
    /// smooth hill whose summit is within a step of the true peak.
    fn build(template: &RgbaImage, step: u32) -> Self {
        let (tw, th) = (template.width, template.height);
        let step = step.max(1);
        let mut samples = Vec::new();
        let mut y = 0;
        while y < th {
            let mut x = 0;
            while x < tw {
                // Coordinates are expressed in the *downsampled* grid so the
                // samples index a `box_downsample`d frame plane directly.
                samples.push((x / step, y / step, block_mean_template(template, x, y, step)));
                x += step;
            }
            y += step;
        }
        let size = (tw.div_ceil(step), th.div_ceil(step));
        // f64 accumulation: a 200x200 template sums to ~1e7, where f32's
        // ~7 significant digits visibly degrade the centered differences.
        let n = samples.len().max(1) as f64;
        let mean = samples.iter().map(|s| s.2 as f64).sum::<f64>() / n;
        let norm = samples.iter().map(|s| (s.2 as f64 - mean).powi(2)).sum::<f64>().sqrt();
        SampledTemplate { samples, mean, norm, size }
    }

    /// Whether the template itself carries enough structure to match with.
    /// A flat template can only ever produce meaningless scores.
    fn is_matchable(&self) -> bool {
        let n = self.samples.len() as f64;
        n >= 4.0 && self.norm * self.norm >= MIN_VARIANCE_PER_SAMPLE * n
    }
}

/// Mean template luma over the `step`×`step` block at `(x0, y0)`, clipped
/// at the template edges.
fn block_mean_template(t: &RgbaImage, x0: u32, y0: u32, step: u32) -> f32 {
    if step == 1 {
        return t.luma_at(x0, y0);
    }
    let (mut sum, mut n) = (0.0f32, 0.0f32);
    for y in y0..(y0 + step).min(t.height) {
        for x in x0..(x0 + step).min(t.width) {
            sum += t.luma_at(x, y);
            n += 1.0;
        }
    }
    if n == 0.0 { 0.0 } else { sum / n }
}

/// Box-downsamples a luma plane by `step`, the frame-side counterpart of
/// [`SampledTemplate::build`]'s block averaging. Both sides must be blurred
/// the same way or their coarse scores describe different images.
fn box_downsample(plane: &(u32, u32, Vec<f32>), step: u32) -> (u32, u32, Vec<f32>) {
    let (w, h, px) = plane;
    if step <= 1 {
        return plane.clone();
    }
    let (dw, dh) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = Vec::with_capacity((dw * dh) as usize);
    for by in 0..dh {
        for bx in 0..dw {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for y in by * step..((by + 1) * step).min(*h) {
                for x in bx * step..((bx + 1) * step).min(*w) {
                    sum += px[(y * w + x) as usize];
                    n += 1.0;
                }
            }
            out.push(if n == 0.0 { 0.0 } else { sum / n });
        }
    }
    (dw, dh, out)
}

/// Computes the tightly-packed luma plane of `frame`.
pub(crate) fn luma_plane(frame: &Frame) -> (u32, u32, Vec<f32>) {
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
    let mut fsum = 0.0f64;
    for &(tx, ty, _) in &tpl.samples {
        let f = px[((oy + ty as i64) * *fw as i64 + ox + tx as i64) as usize];
        fsum += f as f64;
    }
    let n = tpl.samples.len() as f64;
    let fmean = fsum / n;

    // Pass 2: correlation numerator and window variance.
    let (mut num, mut den_f) = (0.0f64, 0.0f64);
    for &(tx, ty, t) in &tpl.samples {
        let f = px[((oy + ty as i64) * *fw as i64 + ox + tx as i64) as usize] as f64;
        num += (f - fmean) * (t as f64 - tpl.mean);
        den_f += (f - fmean) * (f - fmean);
    }

    // Reject flat windows instead of dividing by an epsilon. See
    // MIN_VARIANCE_PER_SAMPLE: this is where phantom full-score matches on
    // solid-color regions used to be born.
    if den_f < MIN_VARIANCE_PER_SAMPLE * n {
        return None;
    }
    Some(num / (den_f.sqrt() * tpl.norm))
}

/// Finds `template` inside `roi` of `frame`.
///
/// Phase 1 scans the ROI on a genuine image pyramid (both frame and
/// template box-blurred and downsampled by [`COARSE_STEP`]); phase 2 refines
/// the top few candidates at full resolution; phase 3 fits a 1D parabola per
/// axis for subpixel precision.
pub fn match_template(frame: &Frame, template: &RgbaImage, roi: SearchRoi) -> Option<TemplateMatch> {
    let plane = luma_plane(frame);
    let (fw, fh) = (plane.0, plane.1);
    let (tw, th) = (template.width, template.height);
    if tw == 0 || th == 0 || tw + 2 > fw || th + 2 > fh {
        return None;
    }

    let fine_tpl = SampledTemplate::build(template, 1);
    // A featureless template cannot be located; say so rather than return
    // an arbitrary position with a meaningless score.
    if !fine_tpl.is_matchable() {
        return None;
    }

    // Clamp the ROI to positions where the template fits inside the frame.
    let clamp = |v: f64, hi: f64| v.clamp(0.0, hi);
    let x_lo = clamp((roi.center.0 - roi.half).floor(), (fw - tw) as f64) as i64;
    let x_hi = clamp((roi.center.0 + roi.half).ceil(), (fw - tw) as f64) as i64;
    let y_lo = clamp((roi.center.1 - roi.half).floor(), (fh - th) as f64) as i64;
    let y_hi = clamp((roi.center.1 + roi.half).ceil(), (fh - th) as f64) as i64;

    // Phase 1: coarse scan on the pyramid level.
    //
    // Both sides are box-blurred by the same factor, so the coarse NCC
    // landscape is a smooth hill rather than an aliased needle field and
    // its summit lies within one coarse step of the true peak. Point
    // sampling here (the P2-c behavior) made the coarse scores pure noise
    // for detailed templates: the true peak did not rank at all, so phase 2
    // refined the wrong basin and the reported score stayed plausible.
    let step = COARSE_STEP as i64;
    let coarse_tpl = SampledTemplate::build(template, COARSE_STEP);
    let mut candidates: Vec<(i64, i64)> = Vec::new();
    if coarse_tpl.is_matchable() {
        let coarse_plane = box_downsample(&plane, COARSE_STEP);
        let mut scored: Vec<(i64, i64, f64)> = Vec::new();
        let mut oy = y_lo;
        while oy <= y_hi {
            let mut ox = x_lo;
            while ox <= x_hi {
                if let Some(s) = ncc(&coarse_plane, &coarse_tpl, ox / step, oy / step) {
                    scored.push((ox, oy, s));
                }
                ox += step;
            }
            oy += step;
        }
        scored.sort_by(|a, b| b.2.total_cmp(&a.2));
        scored.truncate(TOP_K);
        candidates.extend(scored.into_iter().map(|(x, y, _)| (x, y)));
    }
    // Always refine around the caller's prediction as well, whatever the
    // coarse pass thought: verifying the current belief at full fidelity is
    // the cheapest way to keep a good track locked.
    candidates.push((
        (roi.center.0.round() as i64).clamp(x_lo, x_hi),
        (roi.center.1.round() as i64).clamp(y_lo, y_hi),
    ));

    // Phase 2: full-resolution refinement around each candidate.
    //
    // Every comparison from here on uses *fine* scores only. Mixing the two
    // silently disables the refinement: coarse and fine NCC normalize over
    // different sample sets, coarse scores run systematically higher, so
    // `fine_score > coarse_best` was almost never true and phase 2 returned
    // the lattice point unchanged (the other half of the P2-c defect).
    let r = step;
    let mut best: Option<(i64, i64, f64)> = None;
    for (cx0, cy0) in candidates {
        for oy in (cy0 - r).max(0)..=(cy0 + r) {
            for ox in (cx0 - r).max(0)..=(cx0 + r) {
                if let Some(score) = ncc(&plane, &fine_tpl, ox, oy) {
                    if best.is_none_or(|b| score > b.2) {
                        best = Some((ox, oy, score));
                    }
                }
            }
        }
    }
    let (bx, by, bs) = best?;

    // Phase 3: subpixel via 2D parabola over neighbour scores.
    let mut cx = bx as f64 + tw as f64 / 2.0;
    let mut cy = by as f64 + th as f64 / 2.0;
    if let (Some(l), Some(rr)) = (ncc(&plane, &fine_tpl, bx - 1, by), ncc(&plane, &fine_tpl, bx + 1, by)) {
        cx += parabola_offset(l, bs, rr);
    }
    if let (Some(u), Some(d)) = (ncc(&plane, &fine_tpl, bx, by - 1), ncc(&plane, &fine_tpl, bx, by + 1)) {
        cy += parabola_offset(u, bs, d);
    }

    Some(TemplateMatch { center: PhysicalPoint::new(cx, cy), score: bs })
}

/// Subpixel peak offset from three consecutive scores, `center` being the
/// middle one. Returns 0 when the samples do not describe a peak.
///
/// The concavity test (`denom < 0`) and the ±0.5 clamp are both load-bearing:
/// with only `denom.abs() > eps`, a *local minimum* (denom > 0) yields an
/// offset pointing away from the peak, and a near-degenerate denominator
/// yields |offset| >> 0.5 — a subpixel refinement that moves the answer by
/// several pixels in the wrong direction. Neither can be detected downstream,
/// because the score reported alongside it stays high.
fn parabola_offset(left: f64, center: f64, right: f64) -> f64 {
    if !(center >= left && center >= right) {
        return 0.0; // not a peak (typically the window edge)
    }
    let denom = left - 2.0 * center + right;
    if denom > -1e-9 {
        return 0.0; // flat or convex: no meaningful vertex
    }
    (0.5 * (left - right) / denom).clamp(-0.5, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidus_core::io::PixelFormat;

    const W: u32 = 200;
    const H: u32 = 150;

    /// Aperiodic pseudo-random luma. A periodic pattern (a checkerboard,
    /// say) makes the NCC peak genuinely ambiguous — several shifts match
    /// equally well — so such a test would measure the pattern rather than
    /// the matcher.
    fn hash_luma(x: u32, y: u32) -> u8 {
        let mut h = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA6B);
        h ^= h >> 13;
        h = h.wrapping_mul(0xC2B2_AE35);
        h ^= h >> 16;
        (h & 0xFF) as u8
    }

    /// A textured frame with a distinctive patch drawn at `(px, py)`.
    fn frame_with_patch(px: u32, py: u32, pw: u32, ph: u32) -> Frame {
        let mut data = vec![0u8; (W * H * 4) as usize];
        for y in 0..H {
            for x in 0..W {
                // Low-contrast background texture.
                let v = (100 + ((x / 8 + y / 8) % 3) * 4) as u8;
                PixelFormat::Xrgb8888.write_rgba(&mut data, ((y * W + x) * 4) as usize, [v, v, v, 255]);
            }
        }
        for y in py..(py + ph).min(H) {
            for x in px..(px + pw).min(W) {
                let v = hash_luma(x - px, y - py);
                PixelFormat::Xrgb8888.write_rgba(&mut data, ((y * W + x) * 4) as usize, [v, v, v, 255]);
            }
        }
        Frame { width: W, height: H, stride: W * 4, format: PixelFormat::Xrgb8888, data }
    }

    /// The exact content `frame_with_patch` draws, so the true peak scores 1.
    fn patch_template(pw: u32, ph: u32) -> RgbaImage {
        let mut data = Vec::with_capacity((pw * ph * 4) as usize);
        for y in 0..ph {
            for x in 0..pw {
                let v = hash_luma(x, y);
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        RgbaImage::from_raw(pw, ph, data)
    }

    fn solid(w: u32, h: u32, v: u8) -> RgbaImage {
        RgbaImage::from_raw(w, h, vec![v; (w * h * 4) as usize])
    }

    #[test]
    fn flat_window_yields_no_match_instead_of_a_perfect_one() {
        // The P2-c defect: `den.max(1e-6)` turned quantization noise in a
        // solid-color window into |score| ~ 1, i.e. a full-confidence
        // phantom measurement entering the Kalman filter.
        let mut data = vec![0u8; (W * H * 4) as usize];
        for i in 0..(W * H) as usize {
            PixelFormat::Xrgb8888.write_rgba(&mut data, i * 4, [128, 128, 128, 255]);
        }
        let flat = Frame { width: W, height: H, stride: W * 4, format: PixelFormat::Xrgb8888, data };
        let roi = SearchRoi { center: (100.0, 75.0), half: 40.0 };
        assert!(
            match_template(&flat, &patch_template(16, 16), roi).is_none(),
            "a flat frame must not produce a match"
        );
    }

    #[test]
    fn flat_template_is_refused() {
        let f = frame_with_patch(60, 40, 16, 16);
        let roi = SearchRoi { center: (68.0, 48.0), half: 40.0 };
        assert!(match_template(&f, &solid(16, 16, 200), roi).is_none());
    }

    #[test]
    fn refinement_reaches_the_exact_peak_off_the_coarse_lattice() {
        // Phase 2 used to compare fine scores against a coarse score and so
        // almost never fired, leaving the answer on the step-3 lattice.
        // Place the patch so its top-left is NOT a multiple of 3 away from
        // the ROI's lower bound, then demand exactness.
        let (px, py) = (61u32, 44u32);
        let f = frame_with_patch(px, py, 16, 16);
        let tpl = patch_template(16, 16);
        let roi = SearchRoi { center: (px as f64 + 8.0 + 5.0, py as f64 + 8.0 + 4.0), half: 25.0 };
        let m = match_template(&f, &tpl, roi).expect("patch is findable");
        let (want_x, want_y) = (px as f64 + 8.0, py as f64 + 8.0);
        assert!(
            (m.center.x - want_x).abs() < 0.6 && (m.center.y - want_y).abs() < 0.6,
            "match at {:?}, expected ({want_x}, {want_y}); score {}",
            m.center,
            m.score
        );
        assert!(m.score > 0.9, "exact patch should score near 1, got {}", m.score);
    }

    #[test]
    fn parabola_offset_refuses_non_peaks() {
        // A local minimum: the vertex points away from the true peak.
        assert_eq!(parabola_offset(0.9, 0.1, 0.8), 0.0);
        // Monotone ramp (the peak is outside the window).
        assert_eq!(parabola_offset(0.2, 0.5, 0.9), 0.0);
        // Flat.
        assert_eq!(parabola_offset(0.5, 0.5, 0.5), 0.0);
        // A genuine peak, biased right, stays within half a pixel.
        let d = parabola_offset(0.6, 0.95, 0.8);
        assert!(d > 0.0 && d <= 0.5, "d = {d}");
    }

    #[test]
    fn parabola_offset_is_always_bounded() {
        // Near-degenerate denominators used to produce |offset| >> 0.5.
        let d = parabola_offset(0.5 - 1e-12, 0.5, 0.5 - 2e-12);
        assert!(d.abs() <= 0.5, "d = {d}");
    }
}
