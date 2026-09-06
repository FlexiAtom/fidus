//! Constant-velocity Kalman tracking (L4 motion model + L7 fusion substrate).
//!
//! Each axis runs an independent 1D filter over state `[position, velocity]`
//! with a 2×2 covariance — the standard decomposition of a 4-state
//! constant-velocity model, which keeps every operation at 2×2 size.
//!
//! The track lives in **logical** (frame) coordinates: measurements arrive in
//! capture pixels and are mapped through the calibrated frame before being
//! fed in, so a recalibration does not invalidate the state.

use fidus_core::coord::LogicalPoint;

/// One axis of a constant-velocity track: state `[p, v]`, covariance
/// `[[pp, pv], [pv, vv]]`.
#[derive(Clone, Copy, Debug)]
pub struct Kinematic1D {
    p: f64,
    v: f64,
    pp: f64,
    pv: f64,
    vv: f64,
}

impl Kinematic1D {
    /// An uninformative prior: position/velocity unknown with huge variance.
    pub fn uninformative() -> Self {
        Kinematic1D { p: 0.0, v: 0.0, pp: 1e6, pv: 0.0, vv: 1e6 }
    }

    /// Hard reset onto a measured position; velocity zeroed, position
    /// uncertainty kept moderate (the measurement itself is trusted).
    pub fn reset(&mut self, position: f64) {
        self.p = position;
        self.v = 0.0;
        self.pp = 25.0; // 5 px std
        self.pv = 0.0;
        self.vv = 2500.0; // velocity unknown: 50 px/s std
    }

    /// Advances the state by `dt` seconds with white-acceleration process
    /// noise of variance `q` (px/s²).
    pub fn predict(&mut self, dt: f64, q: f64) {
        self.p += self.v * dt;
        let dt2 = dt * dt;
        self.pp += 2.0 * dt * self.pv + dt2 * self.vv + q * dt2 * dt / 3.0;
        self.pv += dt * self.vv + q * dt2 / 2.0;
        self.vv += q * dt;
    }

    /// Assimilates a position measurement with variance `r` (px²).
    pub fn update(&mut self, z: f64, r: f64) {
        let s = self.pp + r;
        if s <= 1e-9 {
            // Degenerate covariance: trust the measurement outright.
            self.p = z;
            return;
        }
        let kp = self.pp / s;
        let kv = self.pv / s;
        let innovation = z - self.p;
        self.p += kp * innovation;
        self.v += kv * innovation;
        self.pp *= 1.0 - kp;
        self.pv *= 1.0 - kp;
        self.vv -= kv * self.pv;
    }

    /// Current position estimate.
    pub fn position(&self) -> f64 {
        self.p
    }

    /// Current velocity estimate (px/s).
    pub fn velocity(&self) -> f64 {
        self.v
    }

    /// Position variance (px²) after the last predict/update.
    pub fn position_variance(&self) -> f64 {
        self.pp
    }
}

/// A 2D constant-velocity track, decomposed per axis.
#[derive(Clone, Copy, Debug)]
pub struct VelocityTrack {
    x: Kinematic1D,
    y: Kinematic1D,
}

impl VelocityTrack {
    /// An uninformative track (no fix yet).
    pub fn uninformative() -> Self {
        VelocityTrack { x: Kinematic1D::uninformative(), y: Kinematic1D::uninformative() }
    }

    /// Hard reset onto a measured position (used for the first fix and for
    /// re-acquisition after a loss, where the old velocity is stale).
    pub fn reset(&mut self, position: LogicalPoint) {
        self.x.reset(position.x);
        self.y.reset(position.y);
    }

    /// Advances both axes by `dt` seconds.
    pub fn predict(&mut self, dt: f64, process_noise: f64) {
        self.x.predict(dt, process_noise);
        self.y.predict(dt, process_noise);
    }

    /// Assimilates a position measurement; the same variance applies to both
    /// axes (template confidence does not distinguish them).
    pub fn update(&mut self, position: LogicalPoint, r_px2: f64) {
        self.x.update(position.x, r_px2);
        self.y.update(position.y, r_px2);
    }

    /// Current position estimate.
    pub fn position(&self) -> LogicalPoint {
        LogicalPoint::new(self.x.position(), self.y.position())
    }

    /// Current velocity estimate (logical px/s).
    pub fn velocity(&self) -> (f64, f64) {
        (self.x.velocity(), self.y.velocity())
    }

    /// Largest axis position variance (px²).
    pub fn position_variance(&self) -> f64 {
        self.x.position_variance().max(self.y.position_variance())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_velocity_is_learned_and_extrapolated() {
        let mut t = VelocityTrack::uninformative();
        t.reset(LogicalPoint::new(100.0, 50.0));
        // A target moving 100 px/s per axis, sampled every 0.5 s.
        for k in 1..=10 {
            t.predict(0.5, 10.0);
            t.update(LogicalPoint::new(100.0 + 50.0 * k as f64, 50.0 + 50.0 * k as f64), 4.0);
        }
        let (vx, vy) = t.velocity();
        assert!((vx - 100.0).abs() < 5.0, "vx = {vx}");
        assert!((vy - 100.0).abs() < 5.0, "vy = {vy}");
        // Coast two seconds: position extrapolates along the velocity.
        t.predict(2.0, 10.0);
        let p = t.position();
        assert!((p.x - (100.0 + 50.0 * 10.0 + 200.0)).abs() < 15.0, "coast x = {}", p.x);
    }

    #[test]
    fn reset_clears_stale_velocity() {
        let mut t = VelocityTrack::uninformative();
        t.reset(LogicalPoint::new(0.0, 0.0));
        for k in 1..=5 {
            t.predict(0.5, 10.0);
            t.update(LogicalPoint::new(50.0 * k as f64, 0.0), 1.0);
        }
        assert!(t.velocity().0 > 50.0);
        // Teleport: re-acquisition resets instead of smearing the old motion.
        t.reset(LogicalPoint::new(900.0, 900.0));
        assert_eq!(t.velocity(), (0.0, 0.0));
        assert_eq!(t.position().x, 900.0);
    }
}
