//! L8 EdgeSync — displacement measurement by low-frequency frame
//! differencing (spec §3.2, §6.1).
//!
//! The differ keeps one reference capture. When the 500 ms window has
//! elapsed, the current capture is differenced against it: changed pixels
//! form blobs, and a blob whose size matches the target and whose content
//! template-verifies (L1 assist — the caller's own render, pure visual) is
//! the displaced target bbox.
//!
//! # Gate interaction (spec §6.1, one documented refinement)
//!
//! The MotionGate judges the change rate `R` over the *last-known target
//! region*. Taken literally, a departing target spikes `R` to ~1.0 there
//! and would block every displacement measurement — the gate would make L8
//! useless exactly when it is needed. The refinement implemented here:
//!
//! * a blob **verified at a displaced position** is emitted even when `R`
//!   is high: the spike is *explained* by the departure, and the arrived
//!   content is template-verified — a stronger check than the statistical
//!   gate;
//! * high `R` **without** a verified displaced blob means the target
//!   content itself is animated (or the departure left nothing verifiable)
//!   → the frame is discarded and the 2-pass resume rule applies, exactly
//!   per spec.
//!
//! Everything runs on fidus' own captures plus the caller's own render —
//! pure vision, zero-trust rule respected.

use std::time::{Duration, Instant};

use fidus_core::coord::{BoundingBox, PhysicalPoint};
use fidus_core::io::Frame;
use fidus_core::target::RgbaImage;

use crate::motion_gate::MotionGate;
use crate::template;
use crate::{FULL_SCORE, MIN_SCORE};

/// Luma difference above which a pixel counts as "changed" (0–255 scale).
const CHANGE_THRESHOLD: f32 = 10.0;
/// A candidate blob's bbox area must lie within `[AREA_MIN_FACTOR,
/// AREA_MAX_FACTOR] ×` the expected target area. The tight upper bound also
/// rejects blobs where the departure and arrival regions merged into one.
const AREA_MIN_FACTOR: f64 = 0.25;
const AREA_MAX_FACTOR: f64 = 2.5;
/// Minimum solidity (pixels / bbox area) for a blob to count as content
/// instead of scattered noise.
const MIN_FILL: f64 = 0.12;
/// Blob centers within this distance of the prediction count as "in place"
/// rather than displaced.
const IN_PLACE_TOLERANCE_PX: f64 = 4.0;
/// Half-extent of the tight template-verification search around a candidate
/// blob's expected top-left corner, in capture pixels.
const VERIFY_HALF_PX: f64 = 8.0;

/// One measured target bbox, in capture pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSyncObservation {
    /// The displaced target's bbox (physical pixels of the capture).
    pub bbox: BoundingBox,
    /// Template-verification confidence in `[0, 1]`.
    pub confidence: f32,
}

/// What one [`EdgeSync::observe`] call concluded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EdgeSyncOutcome {
    /// The reference was (re)stored this call and nothing comparable came
    /// out: first observation, an output geometry change, or a
    /// below-threshold change from which no target-like candidate
    /// survived the filters (quiet scene drift absorbed into the anchor).
    NoReference,
    /// Younger than the differ window: reference refreshed, no diff taken.
    YoungReference,
    /// The gate did not emit: dynamic content discarded the frame (spec
    /// §6.1), or the 2-pass re-arm is still warming up.
    Blocked {
        /// Change rate observed over the last-known target region.
        changed_ratio: f64,
    },
    /// Nothing changed in the search ROI: the target did not move.
    StaticEmpty,
    /// Something changed, but no blob matched the target's size and
    /// content.
    Unmatched,
    /// A displaced (or re-confirmed) target bbox was measured.
    Measured(EdgeSyncObservation),
}

/// A luma snapshot of one capture — the differ's reference side.
struct RefView {
    plane: Vec<f32>,
    size: (u32, u32),
    at: Instant,
}

/// L8 differ: reference capture + MotionGate, producing gated bbox
/// measurements from frame differences (spec §6.1).
pub struct EdgeSync {
    /// The gate classifying change rates (500 ms window, spec thresholds).
    pub gate: MotionGate,
    /// Minimum age of the reference before a diff is taken.
    pub window: Duration,
    reference: Option<RefView>,
}

impl EdgeSync {
    /// A differ with default gate and window.
    pub fn new() -> Self {
        Self {
            gate: MotionGate::default(),
            window: Duration::from_millis(500),
            reference: None,
        }
    }

    /// Feeds one capture and returns what it concluded.
    ///
    /// * `gate_region` — the last-known target bbox (physical pixels): the
    ///   change rate `R` is measured here (spec §6.1 "目标区域").
    /// * `search_roi` — the expanded region blobs are extracted from; it
    ///   must contain `gate_region` to also see the arrival blob.
    /// * `verify` — the target's template at **capture-pixel scale**; every
    ///   size-plausible candidate blob must template-verify before it is
    ///   believed (the arrival blob shows the target's content, the
    ///   departure blob only shows background).
    /// * `now` — capture timestamp driving the window and the gate.
    pub fn observe(
        &mut self,
        frame: &Frame,
        gate_region: BoundingBox,
        search_roi: BoundingBox,
        verify: &RgbaImage,
        now: Instant,
    ) -> EdgeSyncOutcome {
        let (fw, fh) = frame.size();

        // Store / refresh the reference.
        match &self.reference {
            // First observation ever, or the output geometry changed
            // (hotplug / mode switch): start a fresh reference.
            None => {
                self.reference = Some(Self::snap(frame, now));
                return EdgeSyncOutcome::NoReference;
            }
            Some(r) if r.size != (fw, fh) => {
                self.reference = Some(Self::snap(frame, now));
                return EdgeSyncOutcome::NoReference;
            }
            // Younger than the window: HOLD the reference. The diff anchor
            // must stay the frame from ~`window` ago — that is exactly the
            // spec's 500 ms differencing window (§6.1). Refreshing here
            // would reset `at` on every fast call (a 30–60 fps tracking
            // loop), the reference would never mature, and L8 would go
            // silent forever. The reference is replaced only after a diff
            // is taken (see below) or on geometry change.
            Some(r) if now.duration_since(r.at) < self.window => {
                return EdgeSyncOutcome::YoungReference;
            }
            // Reference matured: fall through to the diff below.
            Some(_) => {}
        }

        let reference = self.reference.take().expect("reference checked above");
        let (_, _, current) = template::luma_plane(frame);

        // Gate input: change rate over the last-known target region.
        let ratio = changed_ratio(&reference.plane, &current, fw, fh, gate_region);
        let verdict = self.gate.observe(ratio, now);

        // Blob extraction over the search ROI.
        let (mask_roi, mask, changed_pixels) =
            diff_mask(&reference.plane, &current, fw, fh, search_roi);
        // The current capture becomes the next reference.
        self.reference = Some(RefView { plane: current, size: (fw, fh), at: now });

        if changed_pixels == 0 {
            // A direct observation, no gate needed: nothing moved.
            return EdgeSyncOutcome::StaticEmpty;
        }

        let expected_area = verify.width as f64 * verify.height as f64;
        let predicted = gate_region.center();
        let candidates: Vec<BoundingBox> = blobs_of(mask_roi, &mask)
            .into_iter()
            .filter(|b| {
                let area = b.bbox.width() as f64 * b.bbox.height() as f64;
                let fill = b.pixels as f64 / area.max(1.0);
                area >= expected_area * AREA_MIN_FACTOR
                    && area <= expected_area * AREA_MAX_FACTOR
                    && fill >= MIN_FILL
            })
            .map(|b| b.bbox)
            .collect();
        let had_candidates = !candidates.is_empty();

        // Template verification (L1 assist): the arrival blob shows the
        // target's content, the departure blob shows only background.
        // NOTE: `SearchRoi::center` is the center of the *top-left search
        // region*, so the verification ROI must be anchored at the expected
        // template top-left (blob center minus the template half-size).
        let mut best_moved: Option<EdgeSyncObservation> = None;
        let mut best_in_place: Option<EdgeSyncObservation> = None;
        for bb in candidates {
        // A blob can be a strict subset of the true footprint: pixels whose
        // luma sits within CHANGE_THRESHOLD of the background never cross
        // into the diff mask, so the blob-derived expected top-left can
        // miss the true one by up to half the missing extent per axis. Grow
        // the tight window by that deficit so the search still contains the
        // target; a full-footprint blob yields grow = 0 and stays tight.
        let grow_x = (verify.width as f64 - bb.width() as f64) / 2.0;
        let grow_y = (verify.height as f64 - bb.height() as f64) / 2.0;
        let roi = template::SearchRoi {
            center: (
                bb.center().x - verify.width as f64 / 2.0,
                bb.center().y - verify.height as f64 / 2.0,
            ),
            half: VERIFY_HALF_PX + grow_x.max(grow_y).max(0.0),
        };
            let Some(m) = template::match_template(frame, verify, roi) else {
                continue;
            };
            if m.score < MIN_SCORE {
                continue;
            }
            let obs = EdgeSyncObservation {
                bbox: centered_bbox(m.center, verify.width, verify.height),
                confidence: ((m.score - MIN_SCORE) / (FULL_SCORE - MIN_SCORE))
                    .clamp(0.0, 1.0) as f32,
            };
            let displaced = m.center.distance(predicted) > IN_PLACE_TOLERANCE_PX;
            if displaced {
                if best_moved.as_ref().is_none_or(|b| obs.confidence > b.confidence) {
                    best_moved = Some(obs);
                }
            } else if best_in_place.as_ref().is_none_or(|b| obs.confidence > b.confidence) {
                best_in_place = Some(obs);
            }
        }

        match (best_moved, best_in_place) {
            // A template-verified displaced blob is a displacement
            // measurement even when R spiked: the spike is explained by the
            // departure (see the module docs). Emitted immediately.
            (Some(m), _) => EdgeSyncOutcome::Measured(m),
            // Target re-confirmed where we thought, content static.
            (None, Some(m)) if verdict.passed => EdgeSyncOutcome::Measured(m),
            // Target still present but its content changed too fast:
            // DYNAMIC discard per spec §6.1, 2-pass resume applies.
            (None, Some(_)) => EdgeSyncOutcome::Blocked { changed_ratio: ratio },
            // Changed content, nothing verified, and the rate itself is
            // dynamic: discard.
            _ if verdict.dynamic => EdgeSyncOutcome::Blocked { changed_ratio: ratio },
            // Changed content below the dynamic threshold, but nothing
            // target-like: leave the measurement to other layers.
            _ if !had_candidates => EdgeSyncOutcome::NoReference,
            // Size-plausible blobs existed but none verified: an
            // unexplained, target-shaped change — the capture is no
            // longer a trustworthy diff anchor — drop it; the next
            // observe re-anchors from scratch.
            _ => {
                self.reference = None;
                EdgeSyncOutcome::Unmatched
            }
        }
    }

    fn snap(frame: &Frame, at: Instant) -> RefView {
        let (_, _, plane) = template::luma_plane(frame);
        RefView { plane, size: frame.size(), at }
    }
}

impl Default for EdgeSync {
    fn default() -> Self {
        Self::new()
    }
}

/// BoundingBox centered at `c` with the given size (physical pixels).
fn centered_bbox(c: PhysicalPoint, w: u32, h: u32) -> BoundingBox {
    let (hw, hh) = (w as f64 / 2.0, h as f64 / 2.0);
    BoundingBox {
        x0: (c.x - hw).round() as i64,
        y0: (c.y - hh).round() as i64,
        x1: (c.x + hw).round() as i64,
        y1: (c.y + hh).round() as i64,
    }
}

/// Clamps `region` to the frame bounds; the returned range is empty when
/// the region lies entirely outside.
fn clamp_region(region: BoundingBox, w: u32, h: u32) -> (i64, i64, i64, i64) {
    let x0 = region.x0.clamp(0, w as i64);
    let y0 = region.y0.clamp(0, h as i64);
    let x1 = region.x1.clamp(0, w as i64).max(x0);
    let y1 = region.y1.clamp(0, h as i64).max(y0);
    (x0, y0, x1, y1)
}

/// Fraction of pixels whose luma changed beyond [`CHANGE_THRESHOLD`] inside
/// `region` (clamped to the frame). Empty regions yield `0.0`.
fn changed_ratio(a: &[f32], b: &[f32], w: u32, h: u32, region: BoundingBox) -> f64 {
    let (x0, y0, x1, y1) = clamp_region(region, w, h);
    let mut changed = 0u64;
    let mut total = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w as i64 + x) as usize;
            total += 1;
            if (a[i] - b[i]).abs() > CHANGE_THRESHOLD {
                changed += 1;
            }
        }
    }
    if total == 0 { 0.0 } else { changed as f64 / total as f64 }
}

/// Boolean changed-pixel mask over `roi` (clamped to the frame). Returns
/// the effective mask region, the row-major mask, and the changed count.
fn diff_mask(
    a: &[f32],
    b: &[f32],
    w: u32,
    h: u32,
    roi: BoundingBox,
) -> (BoundingBox, Vec<bool>, usize) {
    let (x0, y0, x1, y1) = clamp_region(roi, w, h);
    let rw = (x1 - x0) as usize;
    let rh = (y1 - y0) as usize;
    let mut mask = vec![false; rw * rh];
    let mut changed = 0usize;
    for yy in 0..rh {
        for xx in 0..rw {
            let i = ((y0 + yy as i64) * w as i64 + x0 + xx as i64) as usize;
            if (a[i] - b[i]).abs() > CHANGE_THRESHOLD {
                mask[yy * rw + xx] = true;
                changed += 1;
            }
        }
    }
    (BoundingBox { x0, y0, x1, y1 }, mask, changed)
}

/// One 8-connected changed-pixel blob.
struct Blob {
    bbox: BoundingBox,
    pixels: usize,
}

/// Flood-fill blob extraction over the mask (8-connectivity).
fn blobs_of(roi: BoundingBox, mask: &[bool]) -> Vec<Blob> {
    let rw = roi.width() as usize;
    let rh = roi.height() as usize;
    let mut seen = vec![false; rw * rh];
    let mut out = Vec::new();
    for sy in 0..rh {
        for sx in 0..rw {
            let start = sy * rw + sx;
            if !mask[start] || seen[start] {
                continue;
            }
            let mut stack = vec![start];
            seen[start] = true;
            let (mut x_min, mut x_max) = (sx, sx);
            let (mut y_min, mut y_max) = (sy, sy);
            let mut pixels = 0usize;
            while let Some(idx) = stack.pop() {
                pixels += 1;
                let cx = idx % rw;
                let cy = idx / rw;
                x_min = x_min.min(cx);
                x_max = x_max.max(cx);
                y_min = y_min.min(cy);
                y_max = y_max.max(cy);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = cx as i64 + dx;
                        let ny = cy as i64 + dy;
                        if nx < 0 || ny < 0 || nx >= rw as i64 || ny >= rh as i64 {
                            continue;
                        }
                        let n = ny as usize * rw + nx as usize;
                        if mask[n] && !seen[n] {
                            seen[n] = true;
                            stack.push(n);
                        }
                    }
                }
            }
            out.push(Blob {
                bbox: BoundingBox {
                    x0: roi.x0 + x_min as i64,
                    y0: roi.y0 + y_min as i64,
                    x1: roi.x0 + x_max as i64 + 1,
                    y1: roi.y0 + y_max as i64 + 1,
                },
                pixels,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidus_core::io::PixelFormat;

    const W: u32 = 640;
    const H: u32 = 480;
    const TW: u32 = 60;
    const TH: u32 = 40;

    fn pattern(x: u32, y: u32, salt: u32) -> [u8; 4] {
        let v = (((x * 7) ^ (y * 13)).wrapping_add(salt * 97) % 251) as u8;
        let g = ((v as u16 * 3) % 251) as u8;
        [v, g, 250 - v, 255]
    }

    /// Flat gray frame with the textured target pasted with its top-left
    /// at `pos` (`None` = absent). `salt` varies the target's content.
    fn scene(pos: Option<(i64, i64)>, salt: u32) -> Frame {
        let format = PixelFormat::Argb8888;
        let mut data = vec![90u8; (W * H * 4) as usize];
        if let Some((tx, ty)) = pos {
            for yy in 0..TH {
                for xx in 0..TW {
                    let px = tx + xx as i64;
                    let py = ty + yy as i64;
                    if px < 0 || py < 0 || px >= W as i64 || py >= H as i64 {
                        continue;
                    }
                    let i = (py as u32 * W + px as u32) as usize * 4;
                    format.write_rgba(&mut data, i, pattern(xx, yy, salt));
                }
            }
        }
        Frame { width: W, height: H, stride: W * 4, format, data }
    }

    fn template(salt: u32) -> RgbaImage {
        let mut data = Vec::with_capacity((TW * TH * 4) as usize);
        for y in 0..TH {
            for x in 0..TW {
                data.extend_from_slice(&pattern(x, y, salt));
            }
        }
        RgbaImage::from_raw(TW, TH, data)
    }

    fn bbox(x0: i64, y0: i64) -> BoundingBox {
        BoundingBox { x0, y0, x1: x0 + TW as i64, y1: y0 + TH as i64 }
    }

    fn expanded(b: BoundingBox, m: i64) -> BoundingBox {
        BoundingBox { x0: b.x0 - m, y0: b.y0 - m, x1: b.x1 + m, y1: b.y1 + m }
    }

    #[test]
    fn first_and_young_observations_only_store_the_reference() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 64);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );
        assert_eq!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(200)
            ),
            EdgeSyncOutcome::YoungReference
        );
    }

    #[test]
    fn static_scene_reports_empty() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 64);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );
        // Identical scenes, one window apart: nothing changed, twice.
        assert_eq!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(600)
            ),
            EdgeSyncOutcome::StaticEmpty
        );
        assert_eq!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(1200)
            ),
            EdgeSyncOutcome::StaticEmpty
        );
    }

    #[test]
    fn displaced_target_is_measured_and_verified() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 96);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );

        // The target moves +80 px right, +70 px down within the window —
        // both blobs well separated. R over the old region spikes to ~1.0,
        // but the arrival blob template-verifies → the displacement
        // measurement is emitted (module-doc refinement).
        let o = es.observe(
            &scene(Some((280, 220)), 0),
            region,
            roi,
            &tpl,
            t0 + Duration::from_millis(600),
        );
        let EdgeSyncOutcome::Measured(m) = o else {
            panic!("expected a measurement, got {o:?}");
        };
        let c = m.bbox.center();
        assert!(
            (c.x - (280.0 + tpl.width as f64 / 2.0)).abs() < 1.5,
            "center x = {} (expected {})",
            c.x,
            280.0 + tpl.width as f64 / 2.0
        );
        assert!(
            (c.y - (220.0 + tpl.height as f64 / 2.0)).abs() < 1.5,
            "center y = {} (expected {})",
            c.y,
            220.0 + tpl.height as f64 / 2.0
        );
        assert!(m.confidence > 0.6, "confidence = {}", m.confidence);
    }

    #[test]
    fn dynamic_content_at_a_known_position_is_blocked() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 64);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );

        // The target stays at (200,150) but its content changes completely
        // (animation frame): R ≈ 1.0 and the in-place blob does not verify
        // against the registered appearance → DYNAMIC discard (spec §6.1).
        assert!(matches!(
            es.observe(
                &scene(Some((200, 150)), 1),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(600)
            ),
            EdgeSyncOutcome::Blocked { changed_ratio } if changed_ratio > 0.5
        ));

        // Content keeps flipping: still blocked.
        assert!(matches!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(1200)
            ),
            EdgeSyncOutcome::Blocked { changed_ratio } if changed_ratio > 0.5
        ));

        // Content settles: empty mask reports held, no measurement.
        assert_eq!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(1800)
            ),
            EdgeSyncOutcome::StaticEmpty
        );
        assert_eq!(
            es.observe(
                &scene(Some((200, 150)), 0),
                region,
                roi,
                &tpl,
                t0 + Duration::from_millis(2400)
            ),
            EdgeSyncOutcome::StaticEmpty
        );
    }

    #[test]
    fn small_unrelated_change_is_unmatched() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 64);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(None, 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );

        // A small bright patch (a cursor, say) crosses the ROI: far below
        // the target's area, so no candidate survives the size filter.
        let mut f = scene(None, 0);
        let format = PixelFormat::Argb8888;
        for y in 230..242 {
            for x in 240..270 {
                let i = (y * W + x) as usize * 4;
                format.write_rgba(&mut f.data, i, [250, 250, 250, 255]);
            }
        }
        assert_eq!(
            es.observe(&f, region, roi, &tpl, t0 + Duration::from_millis(600)),
            EdgeSyncOutcome::NoReference
        );
    }

    #[test]
    fn fast_call_cadence_still_matures_a_diff() {
        // Regression: the YoungReference branch used to refresh the anchor
        // on every call, so a caller looping faster than the 500 ms window
        // (a normal per-frame tracking loop) never produced a diff. The
        // anchor must hold until the window matures.
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 96);
        let tpl = template(0);
        let t0 = Instant::now();

        // ~60 fps cadence: every observation is younger than the window…
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );
        for k in 1..=31 {
            let t = t0 + Duration::from_millis(16 * k);
            assert_eq!(
                es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t),
                EdgeSyncOutcome::YoungReference,
                "step {k}"
            );
        }

        // …and the first frame past the window must diff against the
        // anchored reference (496 ms ago), measuring the displacement.
        let o = es.observe(
            &scene(Some((280, 220)), 0),
            region,
            roi,
            &tpl,
            t0 + Duration::from_millis(512),
        );
        assert!(matches!(o, EdgeSyncOutcome::Measured(_)), "got {o:?}");
    }

    #[test]
    fn geometry_change_restarts_the_reference() {
        let mut es = EdgeSync::new();
        let region = bbox(200, 150);
        let roi = expanded(region, 64);
        let tpl = template(0);
        let t0 = Instant::now();
        assert_eq!(
            es.observe(&scene(Some((200, 150)), 0), region, roi, &tpl, t0),
            EdgeSyncOutcome::NoReference
        );

        // Output resized (hotplug): the stale reference must be dropped.
        let mut f = scene(Some((260, 170)), 0);
        f.width = W / 2;
        f.height = H / 2;
        f.stride = f.width * 4;
        f.data.truncate((f.width * f.height * 4) as usize);
        assert_eq!(
            es.observe(&f, region, roi, &tpl, t0 + Duration::from_millis(600)),
            EdgeSyncOutcome::NoReference
        );
    }
}
