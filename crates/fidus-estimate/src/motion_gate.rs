//! MotionGate — the L8 input gate from spec §6.1.
//!
//! Before an EdgeSync bbox measurement enters the pool, the target region's
//! frame-to-frame change rate `R` (over a ~500 ms window) must pass:
//!
//! * `R > max(baseline * 3, 15%)` → the target **content** is animated
//!   (DYNAMIC): the frame's measurement is discarded and L8 waits;
//! * after a dynamic episode, **two consecutive passing frames** are
//!   required before L8 outputs measurements again.
//!
//! This is pure frame-difference vision — no native API involved, exactly
//! the zero-trust rule.

use std::time::{Duration, Instant};

/// Verdict for one observed change rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateVerdict {
    /// The observed change rate in `[0, 1]`.
    pub changed_ratio: f64,
    /// Whether L8 may emit a measurement from this frame.
    pub passed: bool,
    /// Whether the gate currently considers the content animated.
    pub dynamic: bool,
}

/// The gate state machine.
#[derive(Clone, Debug)]
pub struct MotionGate {
    /// Frame-difference window (spec: 500 ms).
    pub window: Duration,
    /// Relative dynamic threshold (spec: `baseline * 3`).
    pub baseline_factor: f64,
    /// Absolute dynamic threshold (spec: 15%).
    pub absolute_threshold: f64,
    /// Consecutive passes required to resume after a dynamic episode
    /// (spec: 2).
    pub resume_passes: u32,
    /// Learned static-change-rate baseline (EMA of passing ratios).
    baseline: f64,
    static_streak: u32,
    armed: bool,
    last_reference: Option<Instant>,
}

impl Default for MotionGate {
    fn default() -> Self {
        MotionGate {
            window: Duration::from_millis(500),
            baseline_factor: 3.0,
            absolute_threshold: 0.15,
            resume_passes: 2,
            baseline: 0.0,
            static_streak: 0,
            armed: true,
            last_reference: None,
        }
    }
}

impl MotionGate {
    /// Feeds one change-rate observation (measured by the differ over the
    /// target region within the gate's 500 ms window) and returns the
    /// verdict.
    pub fn observe(&mut self, ratio: f64, now: Instant) -> GateVerdict {
        // The caller diffs against the reference frame only when the window
        // has elapsed; younger observations replace the reference instead.
        if let Some(t) = self.last_reference {
            if now.duration_since(t) >= self.window {
                self.last_reference = Some(now);
            }
        } else {
            self.last_reference = Some(now);
        }

        let threshold = (self.baseline * self.baseline_factor).max(self.absolute_threshold);
        if ratio > threshold {
            self.static_streak = 0;
            self.armed = false;
            return GateVerdict { changed_ratio: ratio, passed: false, dynamic: true };
        }

        self.static_streak = self.static_streak.saturating_add(1);
        // Learn the static baseline from passing frames (slow EMA).
        self.baseline = self.baseline * 0.8 + ratio * 0.2;
        let passed = self.static_streak >= self.resume_passes;
        if passed {
            self.armed = true;
        }
        GateVerdict { changed_ratio: ratio, passed, dynamic: false }
    }

    /// Whether the gate is currently emitting (armed) measurements.
    pub fn is_armed(&self) -> bool {
        self.armed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_then_dynamic_then_resume() {
        let mut gate = MotionGate::default();
        let t0 = Instant::now();

        // Two static frames arm the gate.
        assert!(!gate.observe(0.01, t0).passed, "first static frame never passes");
        let v = gate.observe(0.01, t0 + Duration::from_millis(600));
        assert!(v.passed && !v.dynamic);

        // An animated frame is discarded and disarms the gate…
        let v = gate.observe(0.5, t0 + Duration::from_millis(1200));
        assert!(!v.passed && v.dynamic);
        // …and two consecutive static frames are needed to re-arm.
        assert!(!gate.observe(0.02, t0 + Duration::from_millis(1800)).passed);
        assert!(gate.observe(0.02, t0 + Duration::from_millis(2400)).passed);
    }

    #[test]
    fn baseline_adapts_above_fifteen_percent() {
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        // A persistently noisy-but-static region raises the baseline, so a
        // ratio above 15% eventually passes again.
        for k in 1..=40 {
            gate.observe(0.20, t0 + Duration::from_millis(600 * k));
        }
        assert!(gate.is_armed(), "baseline should absorb 20% steady noise");
    }

    #[test]
    fn single_static_frame_after_dynamic_is_not_enough() {
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        gate.observe(0.01, t0);
        gate.observe(0.01, t0 + Duration::from_millis(600));
        gate.observe(0.9, t0 + Duration::from_millis(1200));
        let v = gate.observe(0.01, t0 + Duration::from_millis(1800));
        assert!(!v.passed, "spec requires 2 consecutive passes to resume");
    }
}
