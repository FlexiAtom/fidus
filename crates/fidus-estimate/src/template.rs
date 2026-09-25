// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

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
/// Bound caller-controlled full-screen/coarse scan work in no-display mode.
const MAX_SEARCH_POSITIONS: u64 = 16_000_000;

/// Downsampling factor of the coarse pyramid level.
const COARSE_STEP: u32 = 3;

/// Shift radii (capture pixels) at which template self-similarity is probed
/// along the **diagonals**, and the two points the decay rule compares.
///
/// The smallest radius must exceed the subpixel refinement range (±1 px)
/// so that "the peak is one pixel wide" is not mistaken for ambiguity; the
/// largest is the scale over which a tracking loop must stay locked between
/// frames.
///
/// Along the two axes these four samples are not the coverage any more: every
/// lag from `SELF_SIMILARITY_RADII[0]` to half the template's own extent is
/// probed densely, see [`dense_axis_worst`]. They survive here because the
/// diagonal probe and the near/far pair of the decay rule are what they were
/// always measuring, and both are unchanged.
const SELF_SIMILARITY_RADII: [i64; 4] = [2, 4, 8, 16];

/// Self-similarity above which a template is refused outright: at this level
/// the NCC surface has no distinguishable peak, so the reported position is
/// arbitrary within the ambiguous region.
///
/// 0.98 is deliberately just below the pathological cases (a linear gradient
/// scores exactly 1.000 at *every* radius — it is translation-invariant along
/// its axis) and comfortably above the worst legitimate template measured
/// (a solid block with a thin border: 0.822 at r=2, decaying to 0.648).
const MAX_SELF_SIMILARITY: f64 = 0.98;

/// First lag of the dense axis sweep: the smallest fine radius, which is
/// already outside the subpixel refinement range and already probed along the
/// axes — so the sweep starts here and skips the fine radii it would repeat.
const DENSE_LAG_MIN: i64 = SELF_SIMILARITY_RADII[0];

/// Work cap for the dense axis sweep, in pixel products **across both axes**.
///
/// The sweep is `O(W·H·(W + H))` and the caller registers synchronously, so
/// coverage has to give way to a bound at some size. Measured on this machine
/// in release: the sweep costs ~1.5 ns per product, so the cap is ~90 ms —
/// `400×300` (30 M products) costs 45 ms, `200×200` (5.6 M) costs 8.7 ms,
/// `600×400` keeps only its cheaper axis, `900×600` (119 M for the cheaper
/// axis alone) keeps neither. Above the cap the metric falls back to the fine
/// radii — today's coverage, a boundary the caller can compute from its own
/// template size — and never silently degrades *within* the bound.
const MAX_DENSE_LAG_WORK: u64 = 60_000_000;

/// Minimum decay of self-similarity from the smallest probe radius to the
/// largest before a template counts as genuinely localizable.
///
/// # Why decay, not an absolute level (measured, not guessed)
///
/// Variance is *not* a proxy for localizability, which is the trap this
/// check exists to avoid. Measured on 60×40 templates with the same sampling
/// this function uses (max over `(r,0) (0,r) (r,r) (r,-r)` per radius):
///
/// | template | std | r=2 | r=4 | r=8 | r=16 | verdict |
/// |---|---|---|---|---|---|---|
/// | linear gradient | 73.6 | 1.000 | 1.000 | 1.000 | 1.000 | unlocatable |
/// | grating (fx=1, fy=2) | 60.0 | 0.980 | 0.924 | 0.738 | 0.195 | refused by the dense sweep |
/// | solid block + thin border | 97.5 | 0.822 | 0.711 | 0.692 | 0.648 | weak but locatable |
/// | hash texture | 74.8 | 0.016 | 0.009 | 0.025 | 0.017 | excellent |
///
/// The gradient and the hash texture have **nearly identical variance**
/// (73.6 vs 74.8) and opposite localizability (1.000 vs 0.025): variance has
/// essentially no discriminating power here, so any threshold on it is
/// useless or actively inverted. What separates them is self-similarity —
/// and, for templates that start out high, whether it **falls off** with
/// distance.
///
/// The block's 0.822 → 0.648 plateau passes because the decay requirement
/// only applies above `near > 0.9`: its absolute level is already far enough
/// from 1.0 that its peak is unambiguous.
///
/// The grating row is the cautionary tale about the probe radii themselves.
/// It decays beautifully from 0.980 to 0.195, and that decay is *real* — but
/// it says nothing about the 20 px period the pattern actually repeats on,
/// because no probe lands there. Two windows one period apart score equally,
/// so the reading is arbitrary and silently so. `MAX_DENSE_LAG_WORK`'s sweep
/// is what catches it (measured `1.0000` at a 20 px vertical lag); a grating
/// with only **one** period per axis still decorrelates and stays usable
/// (measured `0.9795`, accepted).
const MIN_SELF_SIMILARITY_DECAY: f64 = 0.1;

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
        let (tw, th) = (template.width(), template.height());
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
    for y in y0..(y0 + step).min(t.height()) {
        for x in x0..(x0 + step).min(t.width()) {
            sum += t.luma_at(x, y);
            n += 1.0;
        }
    }
    if n == 0.0 { 0.0 } else { sum / n }
}

/// The template's luma as a tightly-packed plane, same weights as
/// [`luma_plane`] (which takes a frame).
///
/// The dense sweep touches every pixel once per lag, so the alternative —
/// calling [`RgbaImage::luma_at`] in the inner loop — would redo the Rec.709
/// multiply for the same value once per lag.
fn template_luma_plane(template: &RgbaImage) -> Vec<f32> {
    let (w, h) = (template.width() as usize, template.height() as usize);
    let Some(len) = w.checked_mul(h) else {
        return Vec::new();
    };
    let mut plane = Vec::with_capacity(len);
    for y in 0..h as u32 {
        for x in 0..w as u32 {
            plane.push(template.luma_at(x, y));
        }
    }
    plane
}

/// Column (or row) luma moments of the template, as prefix sums along the
/// axis a shift moves along.
///
/// `pre1[i + 1]` is the luma total over columns `0..=i` (rows `0..h`), `pre2`
/// the same for squared luma. Every overlap window of an axis shift is then a
/// difference of two prefix values, so the mean and the squared deviation sums
/// cost `O(1)` per lag and the sweep's only per-lag work is the cross term.
fn axis_moments(plane: &[f32], w: usize, h: usize, along_x: bool) -> (Vec<f64>, Vec<f64>) {
    let (n, m) = if along_x { (w, h) } else { (h, w) };
    let mut pre1 = vec![0.0f64; n + 1];
    let mut pre2 = vec![0.0f64; n + 1];
    for i in 0..n {
        let (mut sum, mut sq) = (0.0f64, 0.0f64);
        for j in 0..m {
            let v = plane[if along_x { j * w + i } else { i * w + j }] as f64;
            sum += v;
            sq += v * v;
        }
        pre1[i + 1] = pre1[i] + sum;
        pre2[i + 1] = pre2[i] + sq;
    }
    (pre1, pre2)
}

/// `Σ a·b` over the overlap of a shift by `lag` along one axis.
fn cross_term(plane: &[f32], w: usize, h: usize, lag: usize, along_x: bool) -> f64 {
    let mut acc = 0.0f64;
    if along_x {
        for row in plane.chunks(w) {
            for x in 0..w - lag {
                acc += row[x] as f64 * row[x + lag] as f64;
            }
        }
    } else {
        // The overlap is two contiguous blocks of rows: `0..(h - lag)` against
        // `lag..h`, so the walk is sequential in memory either way.
        let len = (h - lag) * w;
        let offset = lag * w;
        for i in 0..len {
            acc += plane[i] as f64 * plane[i + offset] as f64;
        }
    }
    acc
}

/// NCC of the template against itself shifted by `lag` along one axis,
/// computed from precomputed moments.
///
/// The two refusals are the same as [`self_ncc`]'s: a window thinner than 4
/// samples on either axis, and a window whose variance is at quantization
/// level (dividing by that manufactures a score out of nothing). Both are
/// checked *before* the cross term, which is the expensive part.
fn axis_ncc(
    plane: &[f32],
    w: usize,
    h: usize,
    lag: usize,
    along_x: bool,
    moments: &(Vec<f64>, Vec<f64>),
) -> Option<f64> {
    let (n, m) = if along_x { (w, h) } else { (h, w) };
    if n - lag < 4 || m < 4 {
        return None;
    }
    let count = ((n - lag) * m) as f64;
    let (pre1, pre2) = moments;
    // Window A is the shifted-away source `0..(n - lag)`, window B the target
    // `lag..n`; a prefix difference gives each window's moments in O(1).
    let (sa, sb) = (pre1[n - lag], pre1[n] - pre1[lag]);
    let (saa, sbb) = (pre2[n - lag], pre2[n] - pre2[lag]);
    let (ma, mb) = (sa / count, sb / count);
    let (da, db) = (saa - count * ma * ma, sbb - count * mb * mb);
    if da < MIN_VARIANCE_PER_SAMPLE * count || db < MIN_VARIANCE_PER_SAMPLE * count {
        return None;
    }
    let num = cross_term(plane, w, h, lag, along_x) - count * ma * mb;
    Some(num / (da.sqrt() * db.sqrt()))
}

/// Pixel products the dense sweep would spend sweeping one axis of length `n`
/// and depth `m`, or `None` when that axis has no lags left to sweep.
///
/// The lags the fine pass already scored are excluded from the price and from
/// the sweep: paying twice for the same lag would inflate the budget's notion
/// of a template's size.
fn axis_lag_work(n: usize, m: usize) -> Option<u64> {
    let floor = DENSE_LAG_MIN as usize;
    let cap = n / 2;
    if cap < floor {
        return None;
    }
    let total: u64 = (floor..=cap)
        .filter(|l| !SELF_SIMILARITY_RADII.iter().any(|r| *r as usize == *l))
        .map(|l| ((n - l) * m) as u64)
        .sum();
    Some(total)
}

/// Which axes the dense sweep can afford — cheapest first — and the last lag
/// of each.
///
/// The budget is on the *total* across both axes, because the total is what the
/// caller pays. Pricing the axes apart means an unaffordable axis is dropped
/// without taking the affordable one down with it.
fn plan_dense_axes(w: usize, h: usize) -> Vec<(bool, usize)> {
    let mut priced: Vec<(u64, bool, usize)> = Vec::new();
    for along_x in [true, false] {
        let (n, m) = if along_x { (w, h) } else { (h, w) };
        if let Some(work) = axis_lag_work(n, m) {
            priced.push((work, along_x, n / 2));
        }
    }
    priced.sort_unstable_by_key(|(work, _, _)| *work);
    let mut spent = 0u64;
    let mut plan = Vec::with_capacity(priced.len());
    for (work, along_x, cap) in priced {
        match spent.checked_add(work) {
            Some(total) if total <= MAX_DENSE_LAG_WORK => {
                spent = total;
                plan.push((along_x, cap));
            }
            // Out of budget (or out of range): the list is sorted ascending, so
            // no later axis fits either.
            _ => break,
        }
    }
    plan
}

/// Worst self-similarity over **every** axis lag the template can testify to.
///
/// # Scope, which is a boundary rather than a knob
///
/// A mis-lock displaces the reported position by the screen's own repetition
/// period `P`. If `P` fits inside the template, the template is self-similar
/// at `P` and registration can see it — that is exactly the case this sweep
/// closes, and it is swept *densely*, so unlike a radius list it has no gap
/// for a period to hide in. If `P` exceeds the template, the template holds
/// one copy of the content and nothing about the appearance says the screen
/// repeats: that ambiguity is a property of the desktop, and no template-side
/// metric can measure it. Half the extent is where the first case ends — past
/// it the overlap is a sliver that manufactures scores (see
/// [`localizability`]'s boundaries) — and repeat vectors that are neither
/// horizontal nor vertical are still sampled only at the fine radii, because
/// the full 2-D lag space costs 22 s for a 200×200 template, measured.
///
/// Returns `None` when the template is too large to sweep within budget, or
/// when no lag had a usable overlap, and stops at the first lag at or above
/// [`MAX_SELF_SIMILARITY`]: the verdict is already "refuse", and finishing the
/// sweep would only cost the caller.
fn dense_axis_worst(template: &RgbaImage) -> Option<f64> {
    let (w, h) = (template.width() as usize, template.height() as usize);
    // Price the sweep before copying the template: the plane is a megabyte per
    // megapixel, and paying for it only to find the sweep unaffordable would
    // make the budget itself expensive.
    let plan = plan_dense_axes(w, h);
    if plan.is_empty() {
        return None;
    }
    let plane = template_luma_plane(template);
    let mut worst: Option<f64> = None;
    for (along_x, cap) in plan {
        let moments = axis_moments(&plane, w, h, along_x);
        for lag in (DENSE_LAG_MIN as usize)..=cap {
            if SELF_SIMILARITY_RADII.iter().any(|r| *r as usize == lag) {
                continue; // the fine pass already scored this lag
            }
            if let Some(s) = axis_ncc(&plane, w, h, lag, along_x, &moments) {
                if s >= MAX_SELF_SIMILARITY {
                    return Some(s);
                }
                worst = Some(worst.map_or(s, |prev| prev.max(s)));
            }
        }
    }
    worst
}

/// Why a template cannot be tracked, as reported by [`localizability`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unlocatable {
    /// Smaller than the smallest probe shift, so self-similarity cannot even
    /// be measured — and a template this small carries too little structure
    /// to survive a busy background regardless.
    TooSmall,
    /// Flat: no matchable structure at all (a solid color, or noise below
    /// the quantization floor). See `MIN_VARIANCE_PER_SAMPLE`.
    Featureless,
    /// The template looks like itself under translation, so the NCC peak is
    /// not a peak: the match position is arbitrary within the ambiguous
    /// region. Linear gradients are the canonical case.
    SelfSimilar {
        /// Highest NCC against a shifted copy of itself.
        worst: f64,
    },
}

impl core::fmt::Display for Unlocatable {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Unlocatable::TooSmall => write!(f, "template is smaller than the probe radius"),
            Unlocatable::Featureless => write!(f, "template is flat: no matchable structure"),
            Unlocatable::SelfSimilar { worst } => write!(
                f,
                "template is translation-ambiguous (self-similarity {worst:.3}); \
                 a gradient or a repeating pattern cannot be localized"
            ),
        }
    }
}

/// Whether `template` can be located at all, and how distinctly.
///
/// Returns the **worst** (highest) self-similarity the probe can see: over
/// `SELF_SIMILARITY_RADII` in every direction, plus *every* horizontal and
/// vertical lag up to half the template's extent (`dense_axis_worst`). 0 means
/// every shifted copy is uncorrelated (ideal), values approaching 1 mean
/// the match position is increasingly arbitrary.
///
/// # Why this is checked at registration and not only at match time
///
/// `match_template` already refuses a *flat* template, but flatness is the
/// easy case. A linear gradient is not flat — it has a luma std. dev. of ~74
/// — yet it correlates 1.000 with itself at every shift, so NCC returns a
/// confident-looking score at an essentially random position. Downstream that
/// is indistinguishable from a real measurement: it enters the fusion with
/// full confidence and drags the Kalman track to a fictitious place. Refusing
/// at registration turns a silent, permanent tracking error into an immediate
/// error return the caller can act on (project convention 4: degenerate input
/// is refused, not smoothed over).
///
/// # What this check cannot see
///
/// Four boundaries, each a limit of what a *template-side* metric can testify
/// to rather than a threshold that could be tuned away:
///
/// 1. **A screen period longer than the template itself.** A mis-lock
///    displaces the position by the *screen's* repetition period `P`. If `P`
///    fits inside the template, the template is self-similar at `P` and this
///    sweep sees it — which is why the axis lags are swept densely, leaving no
///    gap for a period to hide in. If `P` exceeds the template, the template
///    holds one copy of the content and nothing about the appearance says the
///    desktop repeats: that ambiguity is a property of the desktop, and no
///    template-side measurement can bound it.
/// 2. **Repeats along vectors that are neither horizontal nor vertical.**
///    Those are sampled only at the fine radii, because a dense sweep of the
///    full 2-D lag space costs 22 s for a 200×200 template (measured). A
///    diagonal-only repeat with a period above 16 px can therefore still pass.
///    Unlike the axis case, no measurement of the resulting behaviour exists —
///    for axes the stable mis-lock *was* measured (equal peaks, full
///    confidence), so this is an open boundary, not a mitigated one.
/// 3. **Lags beyond half the extent.** The overlap there is a sliver: measured
///    on a real 160×160 crop, the score at a 154 px vertical lag reached
///    1.0000 on a 6 px overlap — which would refuse a legitimate target. The
///    cap is where a template can no longer be asked about its own period.
/// 4. **Lags the work budget had to give up.** Above `MAX_DENSE_LAG_WORK` the
///    sweep is skipped axis by axis, cheapest first, so a large render can
///    register on *partial* coverage: measured, `600×400` keeps its vertical
///    axis and `900×600` keeps neither, falling back to the fine radii. A host
///    can compute whether its own template fits from the size — which is why
///    every host-facing description of the ceiling has to say what it is *not*
///    a promise about.
pub fn localizability(template: &RgbaImage) -> Result<f64, Unlocatable> {
    let (w, h) = (template.width() as i64, template.height() as i64);
    let min_radius = SELF_SIMILARITY_RADII[0];
    // Need real overlap left after the largest shift, not merely a nonzero
    // one: a sliver of a few pixels produces noisy, meaningless correlations.
    if w <= min_radius * 2 || h <= min_radius * 2 {
        return Err(Unlocatable::TooSmall);
    }
    if !SampledTemplate::build(template, 1).is_matchable() {
        return Err(Unlocatable::Featureless);
    }

    let mut by_radius = Vec::with_capacity(SELF_SIMILARITY_RADII.len());
    for r in SELF_SIMILARITY_RADII {
        // Shifts along both axes and both diagonals: a pattern can be
        // ambiguous along one direction only (a vertical gradient is
        // perfectly distinct horizontally), and one such direction is enough
        // to make the position arbitrary.
        let mut worst = f64::NEG_INFINITY;
        for (dx, dy) in [(r, 0), (0, r), (r, r), (r, -r)] {
            if let Some(s) = self_ncc(template, dx, dy) {
                worst = worst.max(s);
            }
        }
        if worst.is_finite() {
            by_radius.push(worst);
        }
    }
    if by_radius.is_empty() {
        return Err(Unlocatable::TooSmall);
    }

    let mut worst = by_radius.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if worst >= MAX_SELF_SIMILARITY {
        return Err(Unlocatable::SelfSimilar { worst });
    }
    // Highly self-similar templates must at least *decay*: a periodic
    // pattern that decorrelates over distance is trackable, a
    // translation-invariant one is not. Templates whose similarity is
    // already low everywhere skip this (see MIN_SELF_SIMILARITY_DECAY).
    // Deliberately still decided by the fine radii alone: the question is
    // "does similarity fall off with distance", and a sweep capped at half the
    // extent cannot answer it — the far end is the cap, not a decorrelated lag.
    let near = by_radius[0];
    let far = *by_radius.last().expect("non-empty");
    if near > 0.9 && near - far < MIN_SELF_SIMILARITY_DECAY {
        return Err(Unlocatable::SelfSimilar { worst: near });
    }
    // The coarse pass runs only after the fine pass has failed to decide, so
    // the shapes this check exists for — gradients and near-uniform fills,
    // ambiguous at 2 px already — cost no more than they used to.
    if let Some(dense) = dense_axis_worst(template) {
        worst = worst.max(dense);
        if worst >= MAX_SELF_SIMILARITY {
            return Err(Unlocatable::SelfSimilar { worst });
        }
    }
    Ok(worst.clamp(0.0, 1.0))
}

/// NCC between `template` and a copy of itself shifted by `(dx, dy)`, over
/// their overlap. `None` when the overlap is too small or either side is
/// flat there.
fn self_ncc(template: &RgbaImage, dx: i64, dy: i64) -> Option<f64> {
    let (w, h) = (template.width() as i64, template.height() as i64);
    let (x0, x1) = ((-dx).max(0), w.min(w - dx));
    let (y0, y1) = ((-dy).max(0), h.min(h - dy));
    if x1 - x0 < 4 || y1 - y0 < 4 {
        return None;
    }

    let n = ((x1 - x0) * (y1 - y0)) as f64;
    let (mut sa, mut sb) = (0.0f64, 0.0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            sa += template.luma_at(x as u32, y as u32) as f64;
            sb += template.luma_at((x + dx) as u32, (y + dy) as u32) as f64;
        }
    }
    let (ma, mb) = (sa / n, sb / n);

    let (mut num, mut da, mut db) = (0.0f64, 0.0f64, 0.0f64);
    for y in y0..y1 {
        for x in x0..x1 {
            let a = template.luma_at(x as u32, y as u32) as f64 - ma;
            let b = template.luma_at((x + dx) as u32, (y + dy) as u32) as f64 - mb;
            num += a * b;
            da += a * a;
            db += b * b;
        }
    }
    // Same rule as `ncc`: refuse degenerate denominators rather than divide
    // by an epsilon and manufacture a score (project convention 4).
    if da < MIN_VARIANCE_PER_SAMPLE * n || db < MIN_VARIANCE_PER_SAMPLE * n {
        return None;
    }
    Some(num / (da.sqrt() * db.sqrt()))
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
    let Some(len) = (frame.width as usize).checked_mul(frame.height as usize) else {
        return (0, 0, Vec::new());
    };
    let mut p = Vec::with_capacity(len);
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
/// template box-blurred and downsampled by `COARSE_STEP`); phase 2 refines
/// the top few candidates at full resolution; phase 3 fits a 1D parabola per
/// axis for subpixel precision.
pub fn match_template(frame: &Frame, template: &RgbaImage, roi: SearchRoi) -> Option<TemplateMatch> {
    if !frame.is_valid() {
        return None;
    }
    let plane = luma_plane(frame);
    let (fw, fh) = (plane.0, plane.1);
    let (tw, th) = (template.width(), template.height());
    if tw == 0 || th == 0 || tw.checked_add(2).is_none_or(|n| n > fw) || th.checked_add(2).is_none_or(|n| n > fh) {
        return None;
    }
    // SearchRoi is public input, so malformed geometry must not reverse the
    // bounds passed to `clamp` (which panics when min > max). Rejecting it is
    // preferable to fabricating a match from an untrusted search request.
    if !roi.half.is_finite()
        || roi.half < 0.0
        || !roi.center.0.is_finite()
        || !roi.center.1.is_finite()
    {
        return None;
    }
    let span = (roi.half * 2.0).ceil() as u64;
    let search_positions = span.checked_add(1).and_then(|n| n.checked_mul(n));
    if search_positions.is_none_or(|n| n > MAX_SEARCH_POSITIONS) {
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
    fn malformed_roi_is_refused_without_clamp_panic() {
        let f = frame_with_patch(60, 40, 16, 16);
        let tpl = patch_template(16, 16);
        for half in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(
                match_template(&f, &tpl, SearchRoi { center: (68.0, 48.0), half }).is_none(),
                "malformed half extent {half:?} must be rejected"
            );
        }
        for center in [(f64::NAN, 48.0), (68.0, f64::INFINITY)] {
            assert!(match_template(&f, &tpl, SearchRoi { center, half: 40.0 }).is_none());
        }
    }

    #[test]
    fn oversized_roi_is_refused_before_scanning() {
        let f = frame_with_patch(60, 40, 16, 16);
        let tpl = patch_template(16, 16);
        assert!(match_template(
            &f,
            &tpl,
            SearchRoi { center: (68.0, 48.0), half: 10_000.0 },
        ).is_none());
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

    /// Builds a 60×40 RGBA template from a luma function.
    fn tpl_from(f: impl Fn(u32, u32) -> u8) -> RgbaImage {
        let (w, h) = (60u32, 40u32);
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        RgbaImage::from_raw(w, h, data)
    }

    #[test]
    fn gradients_are_refused_despite_high_variance() {
        // The whole reason this check is not a variance threshold: a linear
        // gradient has a luma std. dev. of ~74 — higher than the hash texture
        // that tracks perfectly — yet it correlates 1.000 with itself at
        // every shift, so NCC reports a confident score at an arbitrary
        // position. Variance says "plenty of signal"; localizability says
        // "no information about *where*".
        let horizontal = tpl_from(|x, _| (x * 255 / 60) as u8);
        assert!(
            matches!(localizability(&horizontal), Err(Unlocatable::SelfSimilar { .. })),
            "horizontal gradient accepted: {:?}",
            localizability(&horizontal)
        );

        // Ambiguity along a single axis is enough: this one is perfectly
        // distinct horizontally, and useless vertically.
        let vertical = tpl_from(|_, y| (y * 255 / 40) as u8);
        assert!(
            matches!(localizability(&vertical), Err(Unlocatable::SelfSimilar { .. })),
            "vertical gradient accepted: {:?}",
            localizability(&vertical)
        );

        // Adding ±1 LSB of noise to a gradient raises the variance but not
        // the information: it must still be refused.
        let noisy = tpl_from(|x, y| {
            let base = (x * 255 / 60) as i32 + (((x * 3) ^ (y * 5)) % 3) as i32 - 1;
            base.clamp(0, 255) as u8
        });
        assert!(
            localizability(&noisy).is_err(),
            "noise-dithered gradient accepted: {:?}",
            localizability(&noisy)
        );
    }

    #[test]
    fn flat_and_tiny_templates_are_refused() {
        assert_eq!(localizability(&tpl_from(|_, _| 128)), Err(Unlocatable::Featureless));

        // Quantization-level noise is not structure. Note the assertion is
        // on *refusal*, not on which variant: a ±1 LSB dither sits exactly on
        // the MIN_VARIANCE_PER_SAMPLE boundary, so whether it is caught as
        // `Featureless` or as `SelfSimilar` depends on rounding, not on
        // specification. Pinning the variant here would fossilize an
        // accident of the current implementation (project convention 5) —
        // what the spec requires is only that it never be tracked.
        let near_flat = tpl_from(|x, y| (128 + (((x * 7) ^ (y * 13)) % 2)) as u8);
        assert!(
            localizability(&near_flat).is_err(),
            "±1 LSB dither accepted: {:?}",
            localizability(&near_flat)
        );

        let tiny = RgbaImage::from_raw(3, 3, vec![200; 3 * 3 * 4]);
        assert_eq!(localizability(&tiny), Err(Unlocatable::TooSmall));
    }

    #[test]
    fn genuinely_locatable_templates_are_accepted() {
        // Hash texture: the best case, self-similarity near zero. The bound
        // is tight (measured ~0.025) rather than a loose "< 0.5": this table
        // is quoted as evidence in spec §11.1, so a regression that degraded
        // it to 0.4 must fail here instead of passing quietly.
        let hash = tpl_from(hash_luma);
        let s = localizability(&hash).expect("hash texture must be trackable");
        assert!(s < 0.1, "hash texture self-similarity {s}, expected ~0.025");

        // A grating with **one** period across each axis is smooth (self-
        // similarity 0.98 at 2 px) yet no repeat of it fits inside the template,
        // so its peak is unambiguous and it must not be refused. This is the
        // case a naive "self-similarity > threshold" test would wrongly kill.
        // Measured: 0.9795 worst, coming from the 2 px lag itself — the sweep
        // adds nothing here, which is the point.
        let grating = tpl_from(|x, y| {
            (127.0
                + 120.0
                    * (2.0 * std::f64::consts::PI * x as f64 / 60.0).sin()
                    * (2.0 * std::f64::consts::PI * y as f64 / 40.0).sin())
            .clamp(0.0, 255.0) as u8
        });
        let s = localizability(&grating).expect("single-period grating must be trackable");
        assert!(s > 0.9, "single-period grating should look ambiguous at small lags: {s}");
        assert!(s < MAX_SELF_SIMILARITY, "single-period grating refused: {s}");

        // Weak texture, but locatable: a solid fill with a thin border. Its
        // self-similarity plateaus around 0.65 — high, yet far enough from
        // 1.0 that the peak is unambiguous. Refusing this would reject a
        // perfectly ordinary UI element (a plain window with a frame).
        let bordered =
            tpl_from(|x, y| if (2..57).contains(&x) && (2..37).contains(&y) { 250 } else { 20 });
        assert!(
            localizability(&bordered).is_ok(),
            "bordered block refused: {:?}",
            localizability(&bordered)
        );
    }

    /// The sweep computes its scores from prefix sums, so it must be shown to
    /// produce *the same numbers* as the two-pass reference rather than
    /// approximations of them: a summation-order artefact here would move the
    /// accept/refuse line silently. Measured worst deviation over these fixtures
    /// and every swept lag: 5.9e-14, against a 1e-9 bound.
    #[test]
    fn dense_sweep_reproduces_the_two_pass_reference() {
        let fixtures = [
            tpl_from(hash_luma),
            tpl_from(|x, y| {
                if (2..57).contains(&x) && (2..37).contains(&y) {
                    250
                } else {
                    20
                }
            }),
            tiled(&tpl_from(hash_luma), 2),
        ];
        for t in fixtures {
            let (w, h) = (t.width() as usize, t.height() as usize);
            let plane = template_luma_plane(&t);
            for along_x in [true, false] {
                let n = if along_x { w } else { h };
                let moments = axis_moments(&plane, w, h, along_x);
                for lag in (DENSE_LAG_MIN as usize)..=(n / 2) {
                    let swept = axis_ncc(&plane, w, h, lag, along_x, &moments);
                    let reference = if along_x {
                        self_ncc(&t, lag as i64, 0)
                    } else {
                        self_ncc(&t, 0, lag as i64)
                    };
                    assert_eq!(
                        swept.is_some(),
                        reference.is_some(),
                        "lag {lag} along_x={along_x}: one side refused the overlap and the other did not"
                    );
                    if let (Some(a), Some(b)) = (swept, reference) {
                        assert!(
                            (a - b).abs() < 1e-9,
                            "lag {lag} along_x={along_x}: prefix {a} vs two-pass {b}"
                        );
                    }
                }
            }
        }
    }

    /// Highest self-similarity over the fine radii alone — the coverage the
    /// gate had before the dense sweep existed.
    fn fine_radius_worst(template: &RgbaImage) -> f64 {
        let mut worst = f64::NEG_INFINITY;
        for r in SELF_SIMILARITY_RADII {
            for (dx, dy) in [(r, 0), (0, r), (r, r), (r, -r)] {
                if let Some(s) = self_ncc(template, dx, dy) {
                    worst = worst.max(s);
                }
            }
        }
        worst
    }

    /// A template tiled `copies` times along x: repetition with a period the
    /// fine radii cannot reach.
    fn tiled(base: &RgbaImage, copies: u32) -> RgbaImage {
        let (w, h) = (base.width(), base.height());
        let mut data = Vec::with_capacity((w * copies * h * 4) as usize);
        for y in 0..h {
            for _ in 0..copies {
                for x in 0..w {
                    data.extend_from_slice(&base.rgba(x, y));
                }
            }
        }
        RgbaImage::from_raw(w * copies, h, data)
    }

    /// The hole this sweep exists to close: a template that repeats at a period
    /// beyond the fine radii used to score as *distinctive*.
    #[test]
    fn repetition_beyond_the_fine_radii_is_caught() {
        // `[A|A]` of a hash texture: self-similar at 60 px, which no radius in
        // [2,4,8,16] can reach. The fine pass scores it 0.016 — better than an
        // ordinary texture, because ordinary textures are not *quite* this
        // uncorrelated. Measured.
        let halves = tiled(&tpl_from(hash_luma), 2);
        let fine = fine_radius_worst(&halves);
        assert!(fine < 0.1, "the fine pass should have been blind here, got {fine}");
        assert!(
            matches!(localizability(&halves), Err(Unlocatable::SelfSimilar { worst }) if worst > 0.99),
            "two identical halves were accepted: {:?}",
            localizability(&halves)
        );

        // A grating with *two* periods vertically inside a 40 px template: the
        // same shape, in the other axis. This is the case spec §11.1 used to
        // call "periodic but decaying, usable" — that verdict came from
        // sampling only up to 16 px, below the 20 px period, and the dense
        // sweep falsifies it: the template is identical to itself at lag 20.
        let two_period = tpl_from(|x, y| {
            (127.0
                + 120.0
                    * (2.0 * std::f64::consts::PI * x as f64 / 60.0).sin()
                    * (2.0 * 2.0 * std::f64::consts::PI * y as f64 / 40.0).sin())
            .clamp(0.0, 255.0) as u8
        });
        assert!(
            localizability(&two_period).is_err(),
            "exactly 2-periodic grating accepted: {:?}",
            localizability(&two_period)
        );
    }

    /// Not every coarse repeat is a refusal: an ambiguity that is real but
    /// partial must *discount* the ceiling instead, so there is no cliff at the
    /// accept/refuse boundary.
    #[test]
    fn partial_coarse_repetition_discounts_without_refusing() {
        // An RGB texture that looks like noise and is not: rows are XOR-shifts
        // of one palette, which makes them correlate at a 20 px vertical lag.
        // Fine pass: 0.117. Dense pass: 0.487. Both accept; the ceiling drops
        // from 0.88 to 0.51, which is the honest number for this render.
        let mut data = Vec::with_capacity(60 * 40 * 4);
        for y in 0..40u32 {
            for x in 0..60u32 {
                let v = (((x * 7) ^ (y * 13)) % 251) as u8;
                let g = ((v as u16 * 3) % 251) as u8;
                data.extend_from_slice(&[v, g, 250 - v, 255]);
            }
        }
        let looks_like_noise = RgbaImage::from_raw(60, 40, data);
        let fine = fine_radius_worst(&looks_like_noise);
        let reported = localizability(&looks_like_noise).expect("must stay trackable");
        assert!(fine < 0.2, "expected the fine pass to understate this: {fine}");
        assert!(reported > fine + 0.3, "coarse repeat did not raise the verdict: {reported}");
    }

    /// A stricter gate may refuse more, and must never *understate* ambiguity:
    /// the sweep maximises over a superset of the fine lags, so the reported
    /// value can only rise. That monotonicity is why the refusals measured
    /// before it existed (a gradient, a whole window with transparent margins)
    /// stay refused without re-running them.
    #[test]
    fn the_coarse_pass_can_only_discount_never_rescue() {
        let fixtures = [
            tpl_from(hash_luma),
            tpl_from(|x, y| ((x * 255) / 60).clamp(0, 255) as u8 ^ ((y * 7) % 3) as u8),
            tiled(&tpl_from(hash_luma), 2),
            tpl_from(|x, y| if (2..57).contains(&x) && (2..37).contains(&y) { 250 } else { 20 }),
        ];
        for t in fixtures {
            let floor = fine_radius_worst(&t).min(1.0);
            match localizability(&t) {
                Ok(s) => assert!(
                    s + 1e-9 >= floor,
                    "gate got looser than the fine radii alone: {s} < {floor}"
                ),
                Err(Unlocatable::SelfSimilar { worst }) => assert!(
                    worst + 1e-9 >= floor,
                    "gate got looser than the fine radii alone: {worst} < {floor}"
                ),
                // Flat or too small to probe: refused outright, which no
                // ordering claim applies to.
                Err(_) => {}
            }
        }
    }

    /// The budget drops whole axes, cheapest first, rather than sampling a
    /// large template thinly: coverage is either there or named as absent.
    /// Boundary values measured as pixel products from `axis_lag_work`.
    #[test]
    fn the_budget_gives_up_axes_not_resolution() {
        // 5.6 M products across both axes: fully covered.
        assert_eq!(plan_dense_axes(200, 200).len(), 2);
        // 30 M: still both, and comfortably inside the cap.
        assert_eq!(plan_dense_axes(400, 300).len(), 2);
        // 34.8 M for the cheaper (vertical) axis, 52.8 M for the other: the sum
        // is over the cap, so only the axis that fits is swept.
        assert_eq!(plan_dense_axes(600, 400), vec![(false, 200)]);
        // 119 M for the cheaper axis alone: nothing is swept, and the verdict
        // falls back to the fine radii — a boundary a caller can compute.
        assert!(plan_dense_axes(900, 600).is_empty());
    }

    #[test]
    fn localizability_never_panics_on_degenerate_shapes() {
        // Registration takes whatever the caller renders, so every shape has
        // to produce a verdict rather than an index panic: 1-pixel strips,
        // exactly-at-the-radius sizes, and extreme aspect ratios.
        for (w, h) in [(1, 1), (1, 40), (60, 1), (4, 4), (5, 5), (4, 100), (100, 4)] {
            let data: Vec<u8> = (0..(w * h))
                .flat_map(|i| {
                    let v = hash_luma(i % w, i / w);
                    [v, v, v, 255]
                })
                .collect();
            let img = RgbaImage::from_raw(w, h, data);
            let _ = localizability(&img); // must not panic
        }
    }

    /// A frame of arbitrary size with a distinctive patch at `(px, py)`, for
    /// geometries the fixed `W`/`H` scaffolding cannot express.
    fn frame_sized_with_patch(w: u32, h: u32, px: u32, py: u32, s: u32) -> Frame {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = (100 + ((x / 8 + y / 8) % 3) * 4) as u8;
                PixelFormat::Xrgb8888.write_rgba(&mut data, ((y * w + x) * 4) as usize, [v, v, v, 255]);
            }
        }
        for y in py..(py + s) {
            for x in px..(px + s) {
                let v = hash_luma(x - px, y - py);
                PixelFormat::Xrgb8888.write_rgba(&mut data, ((y * w + x) * 4) as usize, [v, v, v, 255]);
            }
        }
        Frame { width: w, height: h, stride: w * 4, format: PixelFormat::Xrgb8888, data }
    }

    #[test]
    fn cold_whole_output_request_is_rejected_above_roughly_1080p() {
        // With no prior fix the fused estimator asks for a half-extent of
        // `max(w, h) + max(tw, th)` — the whole output. That request is priced
        // by `MAX_SEARCH_POSITIONS` *before* clamping to the frame, so the
        // guard meant for untrusted `SearchRoi` input also fires on our own
        // cold start, and it fires on the geometry rather than the target.
        let s = 96;
        let tpl = patch_template(s, s);
        let small = frame_sized_with_patch(1366, 768, 500, 300, s);
        let large = frame_sized_with_patch(1920, 1080, 700, 400, s);
        let cold = |f: &Frame| SearchRoi {
            center: (f.width as f64 / 2.0, f.height as f64 / 2.0),
            half: f.width.max(f.height) as f64 + f64::from(s),
        };
        assert!(
            match_template(&small, &tpl, cold(&small)).is_some(),
            "a 1366-wide cold start must search"
        );
        assert!(
            match_template(&large, &tpl, cold(&large)).is_none(),
            "a 1920-wide cold start must be cap-rejected"
        );
        // Same large frame, ordinary window: found. So the rejection above is
        // the position cap, not a missing target.
        let tight = SearchRoi { center: (748.0, 448.0), half: 200.0 };
        assert!(match_template(&large, &tpl, tight).is_some());
    }
}
