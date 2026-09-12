// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Coordinate primitives for fidus' own frame.
//!
//! [`LogicalPoint`] and [`PhysicalPoint`] are *fidus-frame* quantities, not
//! platform values: the logical side is anchored to marker margins fidus itself
//! chose during calibration (layer-shell usable-area coordinates), the physical
//! side to pixels fidus itself captured. See the crate documentation.

use core::fmt;

/// A point in the calibrated logical space.
///
/// Logical space is the layer-shell usable-area coordinate space of the
/// calibrated output — the same space in which layer-surface margins are
/// expressed. Its origin and scale are established by fidus' own calibration,
/// never read from a platform API.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalPoint {
    /// Horizontal coordinate, in logical pixels.
    pub x: f64,
    /// Vertical coordinate, in logical pixels.
    pub y: f64,
}

impl LogicalPoint {
    /// Creates a logical point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to `other`.
    pub fn distance(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

impl fmt::Display for LogicalPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:.2}, {:.2}) logical", self.x, self.y)
    }
}

/// A point in capture pixel space.
///
/// Physical space is top-down pixel coordinates inside fidus' own screen
/// captures of the calibrated output (Y-inversion already normalized away by
/// the capture backend).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalPoint {
    /// Horizontal coordinate, in capture pixels.
    pub x: f64,
    /// Vertical coordinate, in capture pixels.
    pub y: f64,
}

impl PhysicalPoint {
    /// Creates a physical point.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Euclidean distance to `other`.
    pub fn distance(self, other: Self) -> f64 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

impl fmt::Display for PhysicalPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:.2}, {:.2}) px", self.x, self.y)
    }
}

/// An axis-aligned integer rectangle in physical pixels, as `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoundingBox {
    /// Left edge (inclusive).
    pub x0: i64,
    /// Top edge (inclusive).
    pub y0: i64,
    /// Right edge (exclusive).
    pub x1: i64,
    /// Bottom edge (exclusive).
    pub y1: i64,
}

impl BoundingBox {
    /// Width in pixels (always ≥ 0).
    pub fn width(&self) -> i64 {
        (self.x1 - self.x0).max(0)
    }

    /// Height in pixels (always ≥ 0).
    pub fn height(&self) -> i64 {
        (self.y1 - self.y0).max(0)
    }

    /// Geometric center of the box.
    pub fn center(&self) -> PhysicalPoint {
        PhysicalPoint::new(
            (self.x0 + self.x1) as f64 / 2.0,
            (self.y0 + self.y1) as f64 / 2.0,
        )
    }
}

/// Least-squares fit quality of an affine solve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Residuals {
    /// Root-mean-square residual over all correspondences, in pixels.
    pub rms: f64,
    /// Largest single residual, in pixels.
    pub max: f64,
}

/// A 2×3 affine transform mapping logical space to physical space:
///
/// ```text
/// x' = a·x + b·y + c
/// y' = d·x + e·y + f
/// ```
///
/// The full affine form (not merely axis-aligned scale + translate) is used so
/// that rotated outputs (panel transforms) are covered by the same model.
///
/// # Why the coefficients are private (principle 1, type-level)
///
/// The fields used to be `pub`, which meant *any* six numbers could be
/// declared a coordinate map — including a translation lifted straight out of
/// `GetWindowRect` / `frameGeometry()`. Principle 1 states that the public API
/// contains no coordinate-input entry point; a `pub` coefficient **is** such
/// an entry point, however well-commented. Project convention 0: what can be
/// sealed by types must not be sealed by convention.
///
/// The only ways to obtain a non-identity transform are therefore
/// [`AffineTransform::from_correspondences`] (solved from measured point
/// pairs) and [`AffineTransform::inverse`] (derived from one that was).
/// [`AffineTransform::IDENTITY`] is reachable but carries no translation and
/// no scale, so it cannot smuggle a position in.
///
/// *What this does **not** protect against, stated honestly*: a caller that
/// fabricates the `PhysicalPoint`s it feeds to `from_correspondences` gets a
/// fabricated map. That hole cannot be closed by types — detectors must be
/// able to construct measured points — so it is closed by construction
/// instead: the only `PhysicalPoint`s inside fidus come from blob detection
/// over fidus' own captures. The seal here removes the *casual* injection
/// path (`AffineTransform { c: x, f: y, .. }`), which is the one that
/// actually happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineTransform {
    /// x' coefficient of x.
    a: f64,
    /// x' coefficient of y.
    b: f64,
    /// x' offset.
    c: f64,
    /// y' coefficient of x.
    d: f64,
    /// y' coefficient of y.
    e: f64,
    /// y' offset.
    f: f64,
}

impl AffineTransform {
    /// The identity transform.
    ///
    /// Safe to expose: zero translation and unit scale, so it cannot carry a
    /// platform-supplied position. It is a mathematical unit, not a frame —
    /// and on its own it cannot build a [`crate::frame::CoordinateFrame`],
    /// which requires a [`SolvedMap`].
    pub const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 0.0, e: 1.0, f: 0.0 };

    /// The six coefficients as `[a, b, c, d, e, f]`.
    ///
    /// Read-only on purpose: diagnostics and logging need to see the map,
    /// nothing needs to write it.
    pub fn coefficients(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// Maps a logical point to physical space.
    pub fn apply(&self, p: LogicalPoint) -> PhysicalPoint {
        PhysicalPoint::new(
            self.a * p.x + self.b * p.y + self.c,
            self.d * p.x + self.e * p.y + self.f,
        )
    }

    /// Maps a physical point through the same coefficients, typed for the
    /// inverse direction (use on an inverse map: physical → logical).
    pub fn apply_physical(&self, p: PhysicalPoint) -> LogicalPoint {
        LogicalPoint::new(
            self.a * p.x + self.b * p.y + self.c,
            self.d * p.x + self.e * p.y + self.f,
        )
    }

    /// Returns the inverse transform (physical → logical).
    ///
    /// Fails if the linear part is singular, which cannot happen for a
    /// transform fitted from real screen correspondences.
    pub fn inverse(&self) -> Result<AffineTransform, SolveError> {
        let det = self.a * self.e - self.b * self.d;
        if det.abs() < 1e-9 {
            return Err(SolveError::Singular);
        }
        let ia = self.e / det;
        let ib = -self.b / det;
        let id = -self.d / det;
        let ie = self.a / det;
        // Inverse of [L | t] is [L⁻¹ | -L⁻¹·t].
        Ok(AffineTransform {
            a: ia,
            b: ib,
            c: -(ia * self.c + ib * self.f),
            d: id,
            e: ie,
            f: -(id * self.c + ie * self.f),
        })
    }

    /// Absolute area scale factor of the linear part (`sqrt(|det|)`).
    ///
    /// For a usable-area → capture-pixels map this equals the output scale
    /// factor (e.g. 2.0 on a 2× HiDPI output), which makes it a useful sanity
    /// signal: values far outside `[0.2, 5.0]` indicate a bogus solve.
    pub fn linear_scale(&self) -> f64 {
        (self.a * self.e - self.b * self.d).abs().sqrt()
    }

    /// Fits the transform to `≥ 3` non-collinear correspondences by least
    /// squares and reports the residuals.
    ///
    /// The result is a [`SolvedMap`] — a witness that these coefficients came
    /// out of measured point pairs. [`crate::frame::CoordinateFrame`] accepts
    /// nothing else, so "calibrated frame" cannot be asserted, only earned.
    pub fn from_correspondences(
        corr: &[(LogicalPoint, PhysicalPoint)],
    ) -> Result<SolvedMap, SolveError> {
        if corr.len() < 3 {
            return Err(SolveError::Degenerate);
        }
        // Normal equations for x' = a·x + b·y + c: M·p = q with
        // M = Σ [x² xy x; xy y² y; x y 1], q = Σ [x·x'; y·x'; x'].
        let mut m = [[0.0f64; 3]; 3];
        let mut qx = [0.0f64; 3];
        let mut qy = [0.0f64; 3];
        for (l, p) in corr {
            let row = [l.x, l.y, 1.0];
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += row[i] * row[j];
                }
                qx[i] += row[i] * p.x;
                qy[i] += row[i] * p.y;
            }
        }
        let px = solve3(m, qx).ok_or(SolveError::Singular)?;
        let py = solve3(m, qy).ok_or(SolveError::Singular)?;
        let t = AffineTransform { a: px[0], b: px[1], c: px[2], d: py[0], e: py[1], f: py[2] };

        let mut sum_sq = 0.0f64;
        let mut max = 0.0f64;
        for (l, p) in corr {
            let got = t.apply(*l);
            let d = got.distance(*p);
            sum_sq += d * d;
            max = max.max(d);
        }
        let n = corr.len() as f64;
        Ok(SolvedMap { map: t, residuals: Residuals { rms: (sum_sq / n).sqrt(), max }, points: corr.len() })
    }

    /// Largest deviation from `other` over the given probe points, evaluated
    /// in physical pixels. Used to compare two independently solved maps.
    pub fn max_difference(&self, other: &AffineTransform, probes: &[LogicalPoint]) -> f64 {
        probes
            .iter()
            .map(|p| self.apply(*p).distance(other.apply(*p)))
            .fold(0.0f64, |acc, d| acc.max(d))
    }
}

/// A transform that was **solved from measured correspondences**, carried
/// together with the fit quality that proves it.
///
/// # Why this type exists (principle 1, type-level)
///
/// Sealing [`AffineTransform`]'s coefficients stops
/// `AffineTransform { c: native_x, f: native_y, .. }`, but
/// [`AffineTransform::IDENTITY`] remains constructible — and handing
/// `IDENTITY` to a frame constructor asserts "logical space *is* capture
/// space", i.e. a 1:1 uncalibrated map masquerading as a calibration result.
/// On an unscaled single-output desktop that map is even nearly right, so the
/// lie would pass every sanity check and only fail on HiDPI or with a panel.
///
/// `SolvedMap` is an unforgeable witness: it has no public constructor, and
/// [`AffineTransform::from_correspondences`] is the only thing that produces
/// one. [`crate::frame::CoordinateFrame::new`] takes a `SolvedMap`, so a
/// coordinate frame can only ever be *earned* by measuring, never asserted.
///
/// *Failure mode if this were skipped*: a well-meaning refactor writes
/// `CoordinateFrame::new(AffineTransform::IDENTITY, ...)` as a "default"
/// frame, every test passes on the developer's 1× screen, and fidus silently
/// reports capture pixels as logical coordinates on every scaled display.
///
/// # The seal, as an executable test
///
/// Writing a platform rectangle straight into a map does not compile:
///
/// ```compile_fail
/// use fidus_core::coord::AffineTransform;
/// // Pretend these came from GetWindowRect / frameGeometry().
/// let (native_x, native_y) = (120.0, 64.0);
/// let injected = AffineTransform { a: 1.0, b: 0.0, c: native_x,
///                                  d: 0.0, e: 1.0, f: native_y };
/// ```
///
/// Nor does declaring a coordinate frame from an unsolved transform:
///
/// ```compile_fail
/// use fidus_core::calibration::CalibrationMethod;
/// use fidus_core::coord::AffineTransform;
/// use fidus_core::frame::{CalibrationQuality, CoordinateFrame};
/// let quality = CalibrationQuality {
///     rms_residual_px: 0.0, max_residual_px: 0.0,
///     verification_max_err_px: 0.0, consistency_max_err_px: 0.0,
///     sample_count: 4, independent_passes: 2,
/// };
/// // `new` takes a SolvedMap; the identity transform is not one.
/// let frame = CoordinateFrame::new(
///     AffineTransform::IDENTITY, (1920, 1080),
///     CalibrationMethod::Crosshair, quality, std::time::SystemTime::now(),
/// );
/// ```
///
/// Solving from measured pairs is the one road that works:
///
/// ```
/// use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint};
/// let corr: Vec<_> = [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (100.0, 100.0)]
///     .into_iter()
///     .map(|(x, y)| (LogicalPoint::new(x, y), PhysicalPoint::new(x * 2.0, y * 2.0)))
///     .collect();
/// let solved = AffineTransform::from_correspondences(&corr).unwrap();
/// assert!((solved.map().linear_scale() - 2.0).abs() < 1e-9);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SolvedMap {
    map: AffineTransform,
    residuals: Residuals,
    points: usize,
}

impl SolvedMap {
    /// The fitted logical→physical transform.
    pub fn map(&self) -> AffineTransform {
        self.map
    }

    /// Least-squares residuals of the fit.
    pub fn residuals(&self) -> Residuals {
        self.residuals
    }

    /// Number of correspondences the fit consumed.
    pub fn point_count(&self) -> usize {
        self.points
    }
}

/// Solves a 3×3 linear system with partial pivoting. Returns `None` if the
/// matrix is (near-)singular.
fn solve3(m: [[f64; 3]; 3], rhs: [f64; 3]) -> Option<[f64; 3]> {
    let mut a = m;
    let mut b = rhs;
    for col in 0..3 {
        // Partial pivot.
        let pivot = (col..3).fold(col, |best, r| if a[r][col].abs() > a[best][col].abs() { r } else { best });
        if a[pivot][col].abs() < 1e-9 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for r in 0..3 {
            if r == col {
                continue;
            }
            let factor = a[r][col] / a[col][col];
            let pivot_row = a[col];
            for c in col..3 {
                a[r][c] -= factor * pivot_row[c];
            }
            b[r] -= factor * b[col];
        }
    }
    Some([b[0] / a[0][0], b[1] / a[1][1], b[2] / a[2][2]])
}

/// Errors produced by affine fitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SolveError {
    /// Fewer than three correspondences, or all of them collinear.
    #[error("need at least 3 non-collinear correspondences")]
    Degenerate,
    /// The fitted linear part is singular.
    #[error("affine transform is singular")]
    Singular,
}
