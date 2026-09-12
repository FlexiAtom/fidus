// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

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
    /// noise of power spectral density `q`.
    ///
    /// `q` is a **PSD in px²/s³**, not an acceleration in px/s²: the
    /// discrete process-noise block below integrates it as `q·dt³/3`,
    /// `q·dt²/2`, `q·dt`, which is only dimensionally consistent for a PSD.
    /// (The previous doc comment said px/s², which misled tuning.)
    ///
    /// A non-positive `dt` is a no-op rather than a covariance corruption:
    /// `pp += 2·dt·pv + …` with `dt < 0` can drive the variance negative,
    /// and a filter with negative variance produces garbage gains forever.
    /// Callers already clamp, but a library primitive must guard itself.
    pub fn predict(&mut self, dt: f64, q: f64) {
        if !dt.is_finite() || dt <= 0.0 || !q.is_finite() || q < 0.0 {
            return;
        }
        self.p += self.v * dt;
        let dt2 = dt * dt;
        self.pp += 2.0 * dt * self.pv + dt2 * self.vv + q * dt2 * dt / 3.0;
        self.pv += dt * self.vv + q * dt2 / 2.0;
        self.vv += q * dt;
    }

    /// Assimilates a position measurement with variance `r` (px²).
    ///
    /// The posterior covariance uses the **Joseph form**
    /// `P = (I−KH)P(I−KH)ᵀ + KRKᵀ`, which stays symmetric positive
    /// semi-definite under rounding and — the reason it is used here — does
    /// not depend on the order in which the entries are written.
    ///
    /// *Failure mode it prevents*: the P2-c code wrote the short form
    /// in place as `pp *= 1−kp; pv *= 1−kp; vv -= kv*self.pv`, where the
    /// third line consumed the **already-updated** `pv`. It subtracted
    /// `kv·pv·(1−kp)` instead of `kv·pv` — 26× too little at typical gains,
    /// leaving the velocity variance ~8% overestimated forever, so the
    /// filter never became confident about velocity and coasting stayed
    /// needlessly wide (project convention 3).
    pub fn update(&mut self, z: f64, r: f64) {
        if !r.is_finite() || r < 0.0 || !z.is_finite() {
            return; // Not a measurement; refuse rather than corrupt state.
        }
        let s = self.pp + r;
        if !s.is_finite() || s <= 0.0 {
            // Degenerate: zero prior variance *and* zero measurement noise.
            // Snapping to the measurement while leaving `v`/`pv`/`vv` stale
            // would leave state and covariance mutually inconsistent, so
            // re-anchor the whole axis instead.
            self.reset(z);
            return;
        }
        let kp = self.pp / s;
        let kv = self.pv / s;
        let innovation = z - self.p;
        self.p += kp * innovation;
        self.v += kv * innovation;

        // Joseph form with H = [1 0]: A = I − K·H = [[1−kp, 0], [−kv, 1]].
        let (pp, pv, vv) = (self.pp, self.pv, self.vv);
        let a = 1.0 - kp;
        self.pp = a * a * pp + kp * kp * r;
        self.pv = a * (pv - kv * pp) + kp * kv * r;
        self.vv = kv * kv * pp - 2.0 * kv * pv + vv + kv * kv * r;
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

    /// Velocity variance ((px/s)²) after the last predict/update.
    pub fn velocity_variance(&self) -> f64 {
        self.vv
    }

    /// Position/velocity covariance (diagnostics and tests).
    pub fn covariance(&self) -> (f64, f64, f64) {
        (self.pp, self.pv, self.vv)
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
    fn posterior_covariance_matches_the_textbook_update() {
        // Regression for the P2-c in-place-order bug: `vv -= kv * self.pv`
        // consumed the already-updated `pv` and subtracted 26x too little,
        // leaving the velocity variance permanently overestimated.
        let (pp, pv, vv, r) = (100.0, 20.0, 50.0, 4.0);
        let mut k = Kinematic1D::uninformative();
        k.p = 0.0;
        k.v = 0.0;
        k.pp = pp;
        k.pv = pv;
        k.vv = vv;
        k.update(10.0, r);

        let s = pp + r;
        let (kp, kv) = (pp / s, pv / s);
        let (want_pp, want_pv, want_vv) = (pp * (1.0 - kp), pv * (1.0 - kp), vv - kv * pv);
        let (got_pp, got_pv, got_vv) = k.covariance();
        assert!((got_pp - want_pp).abs() < 1e-9, "pp {got_pp} != {want_pp}");
        assert!((got_pv - want_pv).abs() < 1e-9, "pv {got_pv} != {want_pv}");
        assert!((got_vv - want_vv).abs() < 1e-9, "vv {got_vv} != {want_vv}");
        // The old code produced 49.85 here instead of 46.15.
        assert!(got_vv < 47.0, "velocity variance must actually shrink");
    }

    #[test]
    fn covariance_stays_positive_semidefinite_over_a_long_run() {
        // Terminal-state check (project convention 2): the Joseph form must
        // keep det(P) >= 0 and both variances positive indefinitely.
        let mut k = Kinematic1D::uninformative();
        k.reset(0.0);
        for i in 1..=2000 {
            k.predict(0.016, 50.0);
            k.update(i as f64 * 1.6, 4.0);
            let (pp, pv, vv) = k.covariance();
            assert!(pp > 0.0 && vv > 0.0, "variance went non-positive at {i}: {pp}, {vv}");
            assert!(pp * vv - pv * pv > -1e-6, "covariance lost PSD at {i}");
        }
    }

    #[test]
    fn non_finite_or_backwards_input_is_refused() {
        let mut k = Kinematic1D::uninformative();
        k.reset(100.0);
        let before = k.covariance();
        k.predict(-1.0, 10.0); // time going backwards
        assert_eq!(k.covariance(), before, "negative dt must not corrupt covariance");
        k.predict(f64::NAN, 10.0);
        k.update(f64::NAN, 4.0);
        k.update(50.0, -1.0);
        assert_eq!(k.covariance(), before);
        assert_eq!(k.position(), 100.0);
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
