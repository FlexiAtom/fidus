// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Marker detection: frame-difference + connected components.
//!
//! The detector compares a capture against a baseline (marker hidden) and
//! finds the changed region. Because the overlay is topmost and fidus
//! controls it, the *difference* isolates the marker regardless of what the
//! desktop shows behind it — dynamic wallpaper can only break a single
//! measurement (by changing between baseline and capture), never fake one,
//! and such breakage surfaces as `Ambiguous`/`NotFound` for the retry loop.
//!
//! Two entry points share one component pass:
//!
//! * [`detect_single_change`] — L9: "the one thing that changed";
//! * [`detect_colored_change`] — L0: "the one thing that changed **and** has
//!   this color", which is what lets four simultaneous sentinels be told
//!   apart in a single capture.

use fidus_core::coord::BoundingBox;
use fidus_core::io::Frame;

/// Detector tuning knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectConfig {
    /// Per-channel difference that counts as "changed".
    pub diff_threshold: u8,
    /// A candidate component must fill at least this fraction of its
    /// bounding box (markers are solid; scattered wallpaper noise is not).
    pub min_fill_ratio: f64,
    /// Accepted component area as a fraction of the expected marker area.
    pub area_tolerance: (f64, f64),
    /// Smallest plausible component area in pixels (rejects cursor-sized
    /// blips).
    pub min_area: u32,
}

impl Default for DetectConfig {
    fn default() -> Self {
        DetectConfig {
            diff_threshold: 12,
            min_fill_ratio: 0.45,
            area_tolerance: (0.25, 3.0),
            min_area: 9,
        }
    }
}

/// One detected marker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    /// Centroid of the changed region, in capture pixels.
    pub centroid: (f64, f64),
    /// Changed-pixel count.
    pub area: u32,
    /// Bounding box of the changed region, in capture pixels.
    pub bbox: BoundingBox,
}

/// Detection failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DetectError {
    /// No changed pixels at all (marker not presented yet?).
    #[error("no changed pixels found")]
    NotFound,
    /// Changed pixels exist but none matches the expected marker signature.
    #[error("changed pixels found but none matches the expected marker area")]
    AreaMismatch,
    /// Several components match; the measurement is ambiguous and must be
    /// retried (usually the background changed between captures).
    #[error("{0} plausible regions found; measurement ambiguous")]
    Ambiguous(usize),
    /// The two frames have different dimensions.
    #[error("baseline and capture differ in size")]
    SizeMismatch,
    /// A frame has empty or truncated row storage.
    #[error("frame buffer is invalid")]
    InvalidFrame,
}

/// Finds the single changed region matching the expected marker signature.
///
/// `expected_area` gates the accepted component area. Without an area prior
/// the detector accepts exactly one coherent candidate; multiple candidates
/// are ambiguous because a background repaint can be larger than the marker.
/// This fail-closed rule trades an occasional retry for never turning an
/// unrelated large repaint into a coordinate measurement.
pub fn detect_single_change(
    baseline: &Frame,
    current: &Frame,
    expected_area: Option<f64>,
    cfg: &DetectConfig,
) -> Result<Detection, DetectError> {
    let mask = changed_mask(baseline, current, cfg.diff_threshold, |_| true)?;
    pick_candidate(components(&mask, current.width, current.height, expected_area, cfg), expected_area)
}

/// Finds the single region that both changed against the baseline **and**
/// matches `rgba` within `color_tolerance` per channel.
///
/// This is L0 Anchor's detector: the difference isolates fidus' own
/// projections from anything static (a wallpaper that happens to contain
/// the sentinel color is identical in both frames and drops out), and the
/// color separates the four simultaneous sentinels from each other.
pub fn detect_colored_change(
    baseline: &Frame,
    current: &Frame,
    rgba: [u8; 4],
    color_tolerance: u8,
    expected_area: Option<f64>,
    cfg: &DetectConfig,
) -> Result<Detection, DetectError> {
    let tol = color_tolerance as i32;
    let mask = changed_mask(baseline, current, cfg.diff_threshold, |px| {
        (0..3).all(|c| (px[c] as i32 - rgba[c] as i32).abs() <= tol)
    })?;
    pick_candidate(components(&mask, current.width, current.height, expected_area, cfg), expected_area)
}

/// Builds the "changed pixel" mask. `accept` further filters a changed
/// pixel by its *current* color (identity for plain change detection).
fn changed_mask(
    baseline: &Frame,
    current: &Frame,
    threshold: u8,
    accept: impl Fn([u8; 4]) -> bool,
) -> Result<Vec<bool>, DetectError> {
    if baseline.width != current.width || baseline.height != current.height {
        return Err(DetectError::SizeMismatch);
    }
    if !baseline.is_valid() || !current.is_valid() {
        return Err(DetectError::InvalidFrame);
    }
    if baseline.format != current.format {
        return Err(DetectError::SizeMismatch); // mixing formats would be meaningless
    }

    let (w, h) = (current.width as usize, current.height as usize);
    let stride = current.stride as usize;
    let base_stride = baseline.stride as usize;
    let Some(row_len) = w.checked_mul(4) else {
        return Err(DetectError::InvalidFrame);
    };
    let Some(mask_len) = w.checked_mul(h) else {
        return Err(DetectError::InvalidFrame);
    };
    let mut changed = vec![false; mask_len];
    let threshold = threshold as i32;
    let mut any_changed = false;

    for y in 0..h {
        let c_row = y * stride;
        let b_row = y * base_stride;
        let c_row_bytes = &current.data[c_row..c_row + row_len];
        let b_row_bytes = &baseline.data[b_row..b_row + row_len];
        // Fast path: identical rows (the common case) need no pixel walk.
        if c_row_bytes == b_row_bytes {
            continue;
        }
        for x in 0..w {
            let i = c_row + x * 4;
            let bi = b_row + x * 4;
            let changed_px = (0..3).any(|c| {
                (current.data[i + c] as i32 - baseline.data[bi + c] as i32).abs() > threshold
            });
            if changed_px && current.format.read_rgba(&current.data, i).is_some_and(&accept) {
                changed[y * w + x] = true;
                any_changed = true;
            }
        }
    }
    if any_changed { Ok(changed) } else { Err(DetectError::NotFound) }
}

/// Connected components (8-connectivity) over a mask, filtered by solidity
/// and — when an area prior exists — by area.
fn components(
    mask: &[bool],
    width: u32,
    height: u32,
    expected_area: Option<f64>,
    cfg: &DetectConfig,
) -> Vec<Detection> {
    let (w, h) = (width as usize, height as usize);
    let mut visited = vec![false; w * h];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut candidates: Vec<Detection> = Vec::new();

    let lo_area = expected_area.map(|a| (a * cfg.area_tolerance.0).max(cfg.min_area as f64));
    let hi_area = expected_area.map(|a| (a * cfg.area_tolerance.1).max(cfg.min_area as f64));

    for y in 0..h {
        for x in 0..w {
            if !mask[y * w + x] || visited[y * w + x] {
                continue;
            }
            stack.push((x, y));
            visited[y * w + x] = true;
            let (mut area, mut sx, mut sy) = (0u32, 0u64, 0u64);
            let (mut min_x, mut min_y) = (i64::MAX, i64::MAX);
            let (mut max_x, mut max_y) = (i64::MIN, i64::MIN);
            while let Some((cx, cy)) = stack.pop() {
                area += 1;
                sx += cx as u64;
                sy += cy as u64;
                min_x = min_x.min(cx as i64);
                max_x = max_x.max(cx as i64);
                min_y = min_y.min(cy as i64);
                max_y = max_y.max(cy as i64);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (cx as i64 + dx, cy as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let idx = ny as usize * w + nx as usize;
                        if mask[idx] && !visited[idx] {
                            visited[idx] = true;
                            stack.push((nx as usize, ny as usize));
                        }
                    }
                }
            }

            let bbox = BoundingBox { x0: min_x, y0: min_y, x1: max_x + 1, y1: max_y + 1 };
            let fill = area as f64 / (bbox.width() * bbox.height()).max(1) as f64;
            if fill < cfg.min_fill_ratio {
                continue;
            }
            let area_ok = match (lo_area, hi_area) {
                (Some(lo), Some(hi)) => area as f64 >= lo && area as f64 <= hi,
                _ => true,
            };
            if area_ok {
                candidates.push(Detection {
                    centroid: (sx as f64 / area as f64, sy as f64 / area as f64),
                    area,
                    bbox,
                });
            }
        }
    }
    candidates
}

fn pick_candidate(
    mut candidates: Vec<Detection>,
    expected_area: Option<f64>,
) -> Result<Detection, DetectError> {
    match candidates.len() {
        0 => Err(DetectError::AreaMismatch),
        1 => Ok(candidates.remove(0)),
        // Without an area prior there is no trustworthy way to rank unrelated
        // changes. Reject all multi-candidate frames and retry instead of
        // silently converting a large repaint into a marker measurement.
        _ if expected_area.is_none() => Err(DetectError::Ambiguous(candidates.len())),
        k => Err(DetectError::Ambiguous(k)),
    }
}
