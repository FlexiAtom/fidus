//! L9 Crosshair — the flagship calibrator (spec §4.1).
//!
//! Principle: project a marker at *known* layer-shell margins
//! (`ANCHOR_TOP|LEFT + margin`), detect it in fidus' own capture, and solve
//! the logical→physical affine map by least squares over several positions.
//! The map absorbs everything the compositor hides — usable-area offsets,
//! fractional scale factors, output transforms — without ever reading a
//! platform coordinate.
//!
//! Robustness protocol (the v0.5.1 upgrade of "at least 2 independent
//! samples" from §4.3):
//!
//! * markers are placed at jittered corners of the usable area (shuffled
//!   order) so no pass shares a position with another;
//! * the first detection of a pass calibrates the expected marker area, the
//!   rest must match it tightly;
//! * a verification round predicts fresh positions before trusting the map;
//! * two independent passes must agree within tolerance;
//! * teardown runs on every exit path (success, failure, panic-via-Drop on
//!   the session).

use fidus_core::calibration::{CalibrationError, CalibrationMethod};
use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint};
use fidus_core::engine::Calibrator;
use fidus_core::frame::{CalibrationQuality, CoordinateFrame};
use fidus_core::io::{CalibrationIo, MarkerShape, MarkerStyle};

use crate::detect::{detect_single_change, DetectConfig, DetectError};
use crate::rng::Rng;

/// Tuning knobs of the crosshair calibration.
#[derive(Clone, Debug)]
pub struct CrosshairConfig {
    /// Visual style of the projected marker.
    pub style: MarkerStyle,
    /// Corner positions sampled per pass (4 = usable-area corners).
    pub primary_positions: usize,
    /// Fresh positions predicted for the verification round.
    pub verification_positions: usize,
    /// Independent passes that must agree.
    pub passes: usize,
    /// Random position jitter, in logical pixels.
    pub jitter_px: f64,
    /// Distance kept from the usable-area edges, in logical pixels.
    pub edge_padding: f64,
    /// Tolerance on the least-squares residual, in pixels.
    pub residual_tolerance_px: f64,
    /// Tolerance on the verification round, in pixels.
    pub verification_tolerance_px: f64,
    /// Tolerance between independent passes, in pixels.
    pub consistency_tolerance_px: f64,
    /// Plausible range of the solved linear scale (DPI sanity).
    pub scale_range: (f64, f64),
    /// Measurement retries per position before giving up.
    pub retries: usize,
    /// RNG seed; `None` seeds from the wall clock.
    pub seed: Option<u64>,
}

impl Default for CrosshairConfig {
    fn default() -> Self {
        CrosshairConfig {
            style: MarkerStyle::DEFAULT,
            primary_positions: 4,
            verification_positions: 3,
            passes: 2,
            jitter_px: 60.0,
            edge_padding: 90.0,
            residual_tolerance_px: 1.0,
            verification_tolerance_px: 1.5,
            consistency_tolerance_px: 1.5,
            scale_range: (0.2, 5.0),
            retries: 3,
            seed: None,
        }
    }
}

/// The L9 Crosshair calibrator.
#[derive(Clone, Debug)]
pub struct CrosshairCalibrator {
    config: CrosshairConfig,
    detect: DetectConfig,
}

impl CrosshairCalibrator {
    /// Creates a calibrator with the given configuration.
    pub fn new(config: CrosshairConfig) -> Self {
        CrosshairCalibrator { config, detect: DetectConfig::default() }
    }

    /// Creates a calibrator with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(CrosshairConfig::default())
    }
}

impl Calibrator for CrosshairCalibrator {
    fn method(&self) -> CalibrationMethod {
        CalibrationMethod::Crosshair
    }

    fn calibrate(
        &mut self,
        io: &mut dyn CalibrationIo,
    ) -> Result<CoordinateFrame, CalibrationError> {
        let result = self.run(io);
        // Spec §4.4: the overlay is destroyed on every exit path — errors
        // included. `destroy_projector` is idempotent.
        let _ = io.destroy_projector();
        result
    }
}

impl CrosshairCalibrator {
    fn run(&mut self, io: &mut dyn CalibrationIo) -> Result<CoordinateFrame, CalibrationError> {
        let cfg = self.config.clone();
        let (uw, uh) = io.usable_size_hint()?;
        let marker = cfg.style.size_logical;

        // Room check: corners must be separated by more than the marker size.
        let min_dim = cfg.edge_padding.mul_add(2.0, marker * 3.0);
        if uw < min_dim || uh < min_dim {
            return Err(CalibrationError::UsableAreaTooSmall {
                size: format!("{uw:.0}×{uh:.0}"),
            });
        }

        let mut rng = match cfg.seed {
            Some(s) => Rng::seed_from(s),
            None => Rng::seed_from_clock(0xB9_u64.rotate_left(32) ^ marker.to_bits()),
        };

        let mut pass_maps: Vec<AffineTransform> = Vec::with_capacity(cfg.passes);
        let mut last_quality: Option<(CalibrationQuality, (u32, u32))> = None;

        for i in 0..cfg.passes {
            let mut rng_pass = rng.clone();
            // Bug fix: advance the *shared* generator, not the throwaway
            // clone. The old `rng_pass.next_u64()` stepped a copy that dies
            // at the end of the iteration, so every pass cloned the same
            // state and sampled IDENTICAL positions — the "independent
            // passes" were not independent.
            rng.next_u64();
            let map = self.calibrate_pass(io, &mut rng_pass, uw, uh)?;
            // TEMP DIAGNOSTIC (remove once P1 lands): per-pass solved map.
            eprintln!(
                "[pass {i}] a={:.5} b={:.5} c={:.2} d={:.5} e={:.5} f={:.2} scale={:.4}",
                map.0.a, map.0.b, map.0.c, map.0.d, map.0.e, map.0.f,
                map.0.linear_scale()
            );
            pass_maps.push(map.0);
            last_quality = Some((map.1, map.2));
        }

        // Independent passes must agree across the whole usable area.
        let probes = corner_probes(uw, uh);
        let consistency = pass_maps[0].max_difference(&pass_maps[cfg.passes - 1], &probes);
        if consistency > cfg.consistency_tolerance_px {
            return Err(CalibrationError::Inconsistent {
                detail: format!(
                    "passes disagree by {consistency:.3} px (tolerance {:.3} px)",
                    cfg.consistency_tolerance_px
                ),
            });
        }

        let map = pass_maps.remove(0);
        let (quality, capture_size) = last_quality.expect("at least one pass ran");
        let quality = CalibrationQuality { consistency_max_err_px: consistency, ..quality };

        CoordinateFrame::new(
            map,
            capture_size,
            CalibrationMethod::Crosshair,
            quality,
            std::time::SystemTime::now(),
        )
        .map_err(CalibrationError::from)
    }

    /// One pass: sample corners, solve, verify at fresh positions.
    #[allow(clippy::type_complexity)]
    fn calibrate_pass(
        &mut self,
        io: &mut dyn CalibrationIo,
        rng: &mut Rng,
        uw: f64,
        uh: f64,
    ) -> Result<(AffineTransform, CalibrationQuality, (u32, u32)), CalibrationError> {
        let cfg = self.config.clone();
        let marker_half = cfg.style.size_logical / 2.0;
        // Margin quantization: layer-shell margins are integer logical
        // pixels, so `marker::show` rounds every requested position. A
        // fractional request therefore lands the projected marker up to
        // 0.5 px away from the logical point used as the correspondence —
        // a per-point, jitter-dependent error that flows straight into the
        // fit residuals. Quantizing here makes the backend's round a no-op
        // and keeps both sides of every correspondence exact.
        let positions: Vec<LogicalPoint> = sample_positions(rng, &cfg, uw, uh)
            .into_iter()
            .map(|p| LogicalPoint::new(p.x.round(), p.y.round()))
            .collect();
        let mut correspondences: Vec<(LogicalPoint, PhysicalPoint)> = Vec::new();
        let mut expected_area: Option<f64> = None;
        let mut capture_size = (0u32, 0u32);

        for (i, top_left) in positions.iter().take(cfg.primary_positions).enumerate() {
            let center = LogicalPoint::new(top_left.x + marker_half, top_left.y + marker_half);
            let detection = self.measure(io, *top_left, expected_area, rng)?;
            if i == 0 {
                // First detection of the pass fixes the expected marker area
                // (scale-adaptive: no assumption about the output DPI).
                expected_area = Some(detection.area as f64);
            }
            capture_size = detection.capture_size;
            correspondences.push((center, detection.center));
        }

        let (map, residuals) = AffineTransform::from_correspondences(&correspondences)?;
        if residuals.rms > cfg.residual_tolerance_px || residuals.max > cfg.residual_tolerance_px * 2.0 {
            return Err(CalibrationError::AccuracyBelowThreshold {
                measured: residuals.rms,
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

        // Verification: predict fresh positions before trusting the map.
        let mut verify_max = 0.0f64;
        for top_left in positions.iter().skip(cfg.primary_positions).take(cfg.verification_positions) {
            let center = LogicalPoint::new(top_left.x + marker_half, top_left.y + marker_half);
            let predicted = map.apply(center);
            let detection = self.measure(io, *top_left, expected_area, rng)?;
            capture_size = detection.capture_size;
            verify_max = verify_max.max(predicted.distance(detection.center));
        }
        if verify_max > cfg.verification_tolerance_px {
            return Err(CalibrationError::AccuracyBelowThreshold {
                measured: verify_max,
                tolerance: cfg.verification_tolerance_px,
                stage: "verification",
            });
        }

        Ok((
            map,
            CalibrationQuality {
                rms_residual_px: residuals.rms,
                max_residual_px: residuals.max,
                verification_max_err_px: verify_max,
                consistency_max_err_px: 0.0, // filled in by the caller
                sample_count: correspondences.len(),
                independent_passes: cfg.passes,
            },
            capture_size,
        ))
    }
    /// One marker measurement: baseline → show → capture → clear → detect.
    fn measure(
        &mut self,
        io: &mut dyn CalibrationIo,
        top_left: LogicalPoint,
        expected_area: Option<f64>,
        rng: &mut Rng,
    ) -> Result<Measurement, CalibrationError> {
        let cfg = self.config.clone();
        let mut last_err: Option<DetectError> = None;
        for _ in 0..cfg.retries {
            let baseline = io.capture()?;
            io.show_marker(top_left, cfg.style)?;
            let post = io.capture()?;
            io.clear_marker()?;

            match detect_single_change(&baseline, &post, expected_area, &self.detect) {
                Ok(d) => {
                    return Ok(Measurement {
                        center: d.bbox.center(),
                        area: d.area,
                        capture_size: (post.width, post.height),
                    })
                }
                Err(e) => last_err = Some(e),
            }
            // Burn a tick before retrying so a settling compositor or an
            // animated background can diverge less between baseline and post.
            let _ = rng.next_u64();
        }
        Err(detection_failed(cfg.retries, top_left, last_err))
    }
}


/// Builds a `DetectionFailed` error enriched with the last detector cause.
fn detection_failed(attempts: usize, at: LogicalPoint, cause: Option<DetectError>) -> CalibrationError {
    let position = match cause {
        Some(c) => format!("({:.0}, {:.0}); last detector error: {c}", at.x, at.y),
        None => format!("({:.0}, {:.0})", at.x, at.y),
    };
    CalibrationError::DetectionFailed { attempts, position }
}

/// One successful measurement.
struct Measurement {
    /// Correspondence point: geometric center of the marker's bounding box
    /// (x1 exclusive), in capture pixels. For the solid square this is the
    /// exact center of the projected rectangle — the connected-component
    /// centroid averages *pixel indices*, which sit half a pixel below the
    /// geometric convention (pixel k spans [k, k+1)), producing the
    /// constant −0.5 px bias previously absorbed into c/f. The box center
    /// is also insensitive to holes inside the blob.
    center: PhysicalPoint,
    area: u32,
    capture_size: (u32, u32),
}

fn corner_probes(uw: f64, uh: f64) -> Vec<LogicalPoint> {
    vec![
        LogicalPoint::new(0.0, 0.0),
        LogicalPoint::new(uw, 0.0),
        LogicalPoint::new(0.0, uh),
        LogicalPoint::new(uw, uh),
        LogicalPoint::new(uw / 2.0, uh / 2.0),
    ]
}

/// Samples shuffled, jittered corner positions followed by verification
/// positions spread over the middle region.
fn sample_positions(rng: &mut Rng, cfg: &CrosshairConfig, uw: f64, uh: f64) -> Vec<LogicalPoint> {
    let p = cfg.edge_padding;
    let j = cfg.jitter_px;
    let mut corners = vec![
        LogicalPoint::new(p, p),
        LogicalPoint::new(uw - p, p),
        LogicalPoint::new(p, uh - p),
        LogicalPoint::new(uw - p, uh - p),
    ];
    rng.shuffle(&mut corners);
    for c in &mut corners {
        c.x += rng.range(-j, j);
        c.y += rng.range(-j, j);
    }
    corners.truncate(cfg.primary_positions.max(1));

    let mut verify = Vec::with_capacity(cfg.verification_positions);
    for k in 0..cfg.verification_positions.max(0) {
        let fx = (k as f64 + 0.5) / cfg.verification_positions as f64;
        verify.push(LogicalPoint::new(
            uw * (0.25 + 0.5 * ((fx * 7.0).fract())),
            uh * (0.25 + 0.5 * ((fx * 13.0).fract())),
        ));
    }
    corners.extend(verify);
    corners
}

/// The crosshair marker shape, for documentation purposes.
///
/// The current implementation projects a solid square (see
/// [`MarkerShape::SolidSquare`]): its centroid is unambiguous and the
/// margin→centroid correspondence is exact. Line-shaped crosshairs trade
/// that for slightly larger support; they remain a refinement option via
/// [`MarkerShape::Crosshair`].
#[allow(dead_code)]
fn marker_shape_note() -> MarkerShape {
    MarkerShape::SolidSquare
}
