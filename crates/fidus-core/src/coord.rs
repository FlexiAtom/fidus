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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffineTransform {
    /// x' coefficient of x.
    pub a: f64,
    /// x' coefficient of y.
    pub b: f64,
    /// x' offset.
    pub c: f64,
    /// y' coefficient of x.
    pub d: f64,
    /// y' coefficient of y.
    pub e: f64,
    /// y' offset.
    pub f: f64,
}

impl AffineTransform {
    /// The identity transform.
    pub const IDENTITY: Self = Self { a: 1.0, b: 0.0, c: 0.0, d: 0.0, e: 1.0, f: 0.0 };

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
    pub fn from_correspondences(
        corr: &[(LogicalPoint, PhysicalPoint)],
    ) -> Result<(AffineTransform, Residuals), SolveError> {
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
        Ok((t, Residuals { rms: (sum_sq / n).sqrt(), max }))
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
