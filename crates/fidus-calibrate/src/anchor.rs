//! L0 Anchor — the universal fallback calibrator (spec §4.3).
//!
//! Four high-saturation solid sentinels are projected **simultaneously** at
//! the corners of the usable area (one window per marker on backends that
//! can do that), located in fidus' own capture, and the logical→physical
//! affine map is solved from the four corner correspondences.
//!
//! Where L9 has a topmost layer surface, L0 has only "some windows we
//! created" — so the detector cannot assume the sentinel is the biggest
//! change on screen. The protocol therefore stacks four independent
//! defenses (spec §4.3, hardened):
//!
//! * **baseline differencing + color**: a sentinel is a pixel region that
//!   *changed* against the markers-hidden baseline **and** carries the
//!   sentinel color. A wallpaper that happens to contain a sentinel-colored
//!   patch is identical in both frames and drops out of the difference;
//!   only something that changes *between* the two captures can interfere,
//!   and that surfaces as `Ambiguous`/`NotFound` → retry;
//! * **shuffled color→corner mapping** every attempt (随机打乱): a moving
//!   decoy would have to fake a *different* corner each time;
//! * **rectangle constraint**: corner positions are jittered per pass, but
//!   each edge is jittered as a whole, so the four projected centers always
//!   form an axis-aligned rectangle in logical space — and, affine maps
//!   preserving parallelism and midpoints, the detected centers must form a
//!   parallelogram with equal diagonals. A decoy accepted for one corner
//!   breaks this violently;
//! * **verification round**: the solved map predicts fresh interior
//!   positions before it is trusted, then **two independent passes** must
//!   agree over the whole usable area.
//!
//! Teardown runs on every exit path.
//!
//! Marker geometry note: the spec's 2×2-pixel sentinel is honored as the
//! configurational floor; the default is a chunkier 8 px marker (a 2 px
//! blob is below the detector's noise floor on any real capture, and a
//! one-shot calibration overlay tolerates a slightly more visible marker).

use fidus_core::calibration::{CalibrationError, CalibrationMethod};
use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint};
use fidus_core::engine::Calibrator;
use fidus_core::frame::{CalibrationQuality, CoordinateFrame};
use fidus_core::io::{CalibrationIo, Frame, MarkerShape, MarkerStyle};

use crate::detect::{detect_colored_change, DetectConfig, DetectError};
use crate::rng::Rng;

/// Default sentinel colors: high-saturation, mutually maximally separated,
/// and rare in desktop imagery. Alpha is ignored (root-window pixels are
/// opaque).
pub const SENTINEL_COLORS: [[u8; 4]; 4] =
    [[255, 0, 255, 255], [0, 255, 255, 255], [255, 255, 0, 255], [0, 255, 128, 255]];

/// Tuning knobs of the anchor calibration.
#[derive(Clone, Debug)]
pub struct AnchorConfig {
    /// The four sentinel colors (index order is irrelevant; every attempt
    /// shuffles their assignment to corners).
    pub colors: [[u8; 4]; 4],
    /// Marker edge length in logical pixels.
    pub marker_size_logical: f64,
    /// Distance kept from the usable-area edges, in logical pixels.
    pub edge_padding: f64,
    /// Random jitter applied to each rectangle edge, in logical pixels.
    pub jitter_px: f64,
    /// Fresh interior positions predicted in the verification round
    /// (0 disables verification; at most 4 — one per color).
    pub verification_positions: usize,
    /// Independent passes that must agree.
    pub passes: usize,
    /// Detection retries per pass before giving up.
    pub retries: usize,
    /// Per-channel color match tolerance (0 = exact).
    pub color_tolerance: u8,
    /// Tolerance on the least-squares residual, in pixels.
    pub residual_tolerance_px: f64,
    /// Tolerance of the rectangle constraint on detected corners, in pixels.
    pub rectangle_tolerance_px: f64,
    /// Tolerance on the verification round, in pixels.
    pub verification_tolerance_px: f64,
    /// Tolerance between independent passes, in pixels.
    pub consistency_tolerance_px: f64,
    /// Plausible range of the solved linear scale.
    pub scale_range: (f64, f64),
    /// RNG seed; `None` seeds from the wall clock.
    pub seed: Option<u64>,
}

impl Default for AnchorConfig {
    fn default() -> Self {
        AnchorConfig {
            colors: SENTINEL_COLORS,
            marker_size_logical: 8.0,
            edge_padding: 24.0,
            jitter_px: 16.0,
            verification_positions: 2,
            passes: 2,
            retries: 3,
            color_tolerance: 8,
            residual_tolerance_px: 1.0,
            rectangle_tolerance_px: 2.0,
            verification_tolerance_px: 1.5,
            consistency_tolerance_px: 1.5,
            scale_range: (0.2, 5.0),
            seed: None,
        }
    }
}

/// The L0 Anchor calibrator.
#[derive(Clone, Debug)]
pub struct AnchorCalibrator {
    config: AnchorConfig,
    detect: DetectConfig,
}

impl AnchorCalibrator {
    /// Creates a calibrator with the given configuration.
    pub fn new(config: AnchorConfig) -> Self {
        AnchorCalibrator { config, detect: DetectConfig::default() }
    }

    /// Creates a calibrator with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(AnchorConfig::default())
    }
}

impl Calibrator for AnchorCalibrator {
    fn method(&self) -> CalibrationMethod {
        CalibrationMethod::Anchor
    }

    fn calibrate(
        &mut self,
        io: &mut dyn CalibrationIo,
    ) -> Result<CoordinateFrame, CalibrationError> {
        let result = self.run(io);
        // Spec §4.4: teardown on every exit path; idempotent.
        let _ = io.destroy_projector();
        result
    }
}

/// One solved pass.
struct PassSolution {
    map: AffineTransform,
    quality: CalibrationQuality,
    capture_size: (u32, u32),
}

/// One simultaneous projection, measured: logical centers paired with the
/// detected physical centers, in `marks` order.
struct Measured {
    correspondences: Vec<(LogicalPoint, PhysicalPoint)>,
    capture_size: (u32, u32),
}

impl AnchorCalibrator {
    fn run(&mut self, io: &mut dyn CalibrationIo) -> Result<CoordinateFrame, CalibrationError> {
        let cfg = self.config.clone();
        let (uw, uh) = io.usable_size_hint()?;
        let marker = cfg.marker_size_logical;

        let min_dim = (cfg.edge_padding + cfg.jitter_px).mul_add(2.0, marker * 3.0);
        if uw < min_dim || uh < min_dim {
            return Err(CalibrationError::UsableAreaTooSmall {
                size: format!("{uw:.0}×{uh:.0}"),
            });
        }

        let mut rng = match cfg.seed {
            Some(s) => Rng::seed_from(s),
            None => Rng::seed_from_clock(0xA0_u64.rotate_left(32) ^ marker.to_bits()),
        };

        let mut solutions: Vec<PassSolution> = Vec::with_capacity(cfg.passes);
        for _ in 0..cfg.passes {
            let mut rng_pass = rng.clone();
            rng.next_u64(); // advance the shared generator: passes must differ
            solutions.push(self.calibrate_pass(io, &mut rng_pass, uw, uh)?);
        }

        // Independent passes must agree across the whole usable area.
        let probes = [
            LogicalPoint::new(0.0, 0.0),
            LogicalPoint::new(uw, 0.0),
            LogicalPoint::new(0.0, uh),
            LogicalPoint::new(uw, uh),
            LogicalPoint::new(uw / 2.0, uh / 2.0),
        ];
        let first = &solutions[0];
        let last = &solutions[solutions.len() - 1];
        let consistency = first.map.max_difference(&last.map, &probes);
        if consistency > cfg.consistency_tolerance_px {
            return Err(CalibrationError::Inconsistent {
                detail: format!(
                    "anchor passes disagree by {consistency:.3} px (tolerance {:.3} px)",
                    cfg.consistency_tolerance_px
                ),
            });
        }

        // Report the worst pass honestly rather than the best.
        let worst = |f: fn(&CalibrationQuality) -> f64| {
            solutions.iter().map(|s| f(&s.quality)).fold(0.0, f64::max)
        };
        let quality = CalibrationQuality {
            rms_residual_px: worst(|q| q.rms_residual_px),
            max_residual_px: worst(|q| q.max_residual_px),
            verification_max_err_px: worst(|q| q.verification_max_err_px),
            consistency_max_err_px: consistency,
            sample_count: first.quality.sample_count,
            independent_passes: cfg.passes,
        };

        CoordinateFrame::new(
            first.map,
            first.capture_size,
            CalibrationMethod::Anchor,
            quality,
            std::time::SystemTime::now(),
        )
        .map_err(CalibrationError::from)
    }

    /// One pass: jittered rectangle, shuffled colors, one simultaneous
    /// projection, rectangle constraint, solve, verify at fresh positions.
    fn calibrate_pass(
        &mut self,
        io: &mut dyn CalibrationIo,
        rng: &mut Rng,
        uw: f64,
        uh: f64,
    ) -> Result<PassSolution, CalibrationError> {
        let cfg = self.config.clone();

        // Corner top-lefts: each rectangle edge jittered as a whole, then
        // quantized so the backend's integer placement is exact.
        let corners = rectangle_corners(rng, &cfg, uw, uh);
        let measured = self.measure(io, rng, &corners)?;

        // Rectangle constraint on the detections (order is [TL, TR, BL, BR]).
        check_rectangle(&measured.correspondences, cfg.rectangle_tolerance_px)?;

        let (map, res) = AffineTransform::from_correspondences(&measured.correspondences)?;
        if res.rms > cfg.residual_tolerance_px || res.max > cfg.residual_tolerance_px * 2.0 {
            return Err(CalibrationError::AccuracyBelowThreshold {
                measured: res.rms,
                tolerance: cfg.residual_tolerance_px,
                stage: "residual",
            });
        }
        let scale = map.linear_scale();
        if !(cfg.scale_range.0..=cfg.scale_range.1).contains(&scale) {
            return Err(CalibrationError::AccuracyBelowThreshold {
                measured: scale,
                tolerance: cfg.scale_range.1,
                stage: "transform sanity",
            });
        }

        // Verification: fresh interior positions, predicted before detected.
        let mut verify_max = 0.0f64;
        let mut capture_size = measured.capture_size;
        let n_verify = cfg.verification_positions.min(cfg.colors.len());
        if n_verify > 0 {
            let positions = interior_positions(rng, &cfg, uw, uh, n_verify);
            let verified = self.measure(io, rng, &positions)?;
            capture_size = verified.capture_size;
            for (center, detected) in &verified.correspondences {
                verify_max = verify_max.max(map.apply(*center).distance(*detected));
            }
            if verify_max > cfg.verification_tolerance_px {
                return Err(CalibrationError::AccuracyBelowThreshold {
                    measured: verify_max,
                    tolerance: cfg.verification_tolerance_px,
                    stage: "verification",
                });
            }
        }

        Ok(PassSolution {
            map,
            quality: CalibrationQuality {
                rms_residual_px: res.rms,
                max_residual_px: res.max,
                verification_max_err_px: verify_max,
                consistency_max_err_px: 0.0, // filled in by the caller
                sample_count: measured.correspondences.len(),
                independent_passes: cfg.passes,
            },
            capture_size,
        })
    }

    /// One simultaneous measurement with retries: baseline → show shuffled
    /// sentinels at `top_lefts` → capture → clear → detect each color.
    fn measure(
        &mut self,
        io: &mut dyn CalibrationIo,
        rng: &mut Rng,
        top_lefts: &[LogicalPoint],
    ) -> Result<Measured, CalibrationError> {
        let cfg = self.config.clone();
        let half = cfg.marker_size_logical / 2.0;
        let mut last_err: Option<DetectError> = None;

        for _ in 0..cfg.retries {
            // Fresh color shuffle every attempt (spec §4.3).
            let mut order: Vec<usize> = (0..cfg.colors.len()).collect();
            rng.shuffle(&mut order);
            let marks: Vec<(LogicalPoint, MarkerStyle)> = top_lefts
                .iter()
                .zip(order.iter())
                .map(|(pos, &ci)| {
                    (
                        *pos,
                        MarkerStyle {
                            rgba: cfg.colors[ci],
                            size_logical: cfg.marker_size_logical,
                            shape: MarkerShape::SolidSquare,
                        },
                    )
                })
                .collect();

            let baseline = io.capture()?;
            io.show_markers(&marks)?;
            let post = io.capture()?;
            io.clear_marker()?;

            match self.detect_all(&baseline, &post, &marks, half) {
                Ok(correspondences) => {
                    return Ok(Measured { correspondences, capture_size: post.size() });
                }
                Err(e) => last_err = Some(e),
            }
            let _ = rng.next_u64();
        }
        Err(CalibrationError::DetectionFailed {
            attempts: cfg.retries,
            position: match last_err {
                Some(e) => format!("simultaneous sentinels; last detector error: {e}"),
                None => "simultaneous sentinels".into(),
            },
        })
    }

    /// Detects every projected sentinel by (change ∧ color). The first
    /// detection fixes the expected area for the rest — scale-adaptive, no
    /// assumption about the output's pixel density.
    fn detect_all(
        &self,
        baseline: &Frame,
        post: &Frame,
        marks: &[(LogicalPoint, MarkerStyle)],
        half: f64,
    ) -> Result<Vec<(LogicalPoint, PhysicalPoint)>, DetectError> {
        let mut expected_area: Option<f64> = None;
        let mut out = Vec::with_capacity(marks.len());
        for (pos, style) in marks {
            let d = detect_colored_change(
                baseline,
                post,
                style.rgba,
                self.config.color_tolerance,
                expected_area,
                &self.detect,
            )?;
            if expected_area.is_none() {
                expected_area = Some(d.area as f64);
            }
            // Geometric center of the bbox (x1 exclusive): exact center of
            // the projected square, hole-insensitive, no −0.5 px index bias.
            out.push((LogicalPoint::new(pos.x + half, pos.y + half), d.bbox.center()));
        }
        Ok(out)
    }
}

/// The four marker top-lefts, ordered [TL, TR, BL, BR]. Each rectangle edge
/// is jittered independently, so the result is still an axis-aligned
/// rectangle; positions are quantized to integer logical pixels.
fn rectangle_corners(rng: &mut Rng, cfg: &AnchorConfig, uw: f64, uh: f64) -> [LogicalPoint; 4] {
    let (p, j, s) = (cfg.edge_padding, cfg.jitter_px, cfg.marker_size_logical);
    let x0 = (p + rng.range(0.0, j)).round();
    let y0 = (p + rng.range(0.0, j)).round();
    let x1 = (uw - p - s - rng.range(0.0, j)).round().max(x0);
    let y1 = (uh - p - s - rng.range(0.0, j)).round().max(y0);
    [
        LogicalPoint::new(x0, y0),
        LogicalPoint::new(x1, y0),
        LogicalPoint::new(x0, y1),
        LogicalPoint::new(x1, y1),
    ]
}

/// `n` random interior top-lefts for the verification round, kept apart
/// from each other and from the edges.
fn interior_positions(rng: &mut Rng, cfg: &AnchorConfig, uw: f64, uh: f64, n: usize) -> Vec<LogicalPoint> {
    let s = cfg.marker_size_logical;
    let mut out: Vec<LogicalPoint> = Vec::with_capacity(n);
    let mut guard = 0;
    while out.len() < n && guard < 64 {
        guard += 1;
        let p = LogicalPoint::new(
            rng.range(uw * 0.2, uw * 0.8 - s).round(),
            rng.range(uh * 0.2, uh * 0.8 - s).round(),
        );
        if out.iter().all(|q| q.distance(p) > s * 4.0) {
            out.push(p);
        }
    }
    out
}

/// Rectangle constraint: the four detected centers must form a rectangle
/// — diagonals bisect each other (parallelogram) *and* are equally long.
/// `corr` is ordered [TL, TR, BL, BR] by construction.
fn check_rectangle(
    corr: &[(LogicalPoint, PhysicalPoint)],
    tolerance: f64,
) -> Result<(), CalibrationError> {
    let p = |i: usize| corr[i].1;
    let (tl, tr, bl, br) = (p(0), p(1), p(2), p(3));
    let mid = |a: PhysicalPoint, b: PhysicalPoint| {
        PhysicalPoint::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
    };
    let (diag1, diag2) = (tl.distance(br), tr.distance(bl));
    let midpoint_gap = mid(tl, br).distance(mid(tr, bl));
    let diag_gap = (diag1 - diag2).abs();
    if diag1 < 1.0 || midpoint_gap > tolerance || diag_gap > tolerance {
        return Err(CalibrationError::Inconsistent {
            detail: format!(
                "detected corners violate the rectangle constraint: midpoint gap {midpoint_gap:.2} px, diagonal gap {diag_gap:.2} px"
            ),
        });
    }
    Ok(())
}
