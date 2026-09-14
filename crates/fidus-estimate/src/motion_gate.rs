// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

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
//!
//! # Two defects this module was rebuilt to fix (P2-f)
//!
//! The P2-b implementation had a pair of failures that reinforced each
//! other; both have regression tests below.
//!
//! 1. **Baseline saturation (self-destruct).** DYNAMIC frames also taught
//!    the baseline, so a steady animation at `R = 0.5` raised the baseline
//!    until `baseline * 3 > 0.5`: after **22 observations** the real
//!    animation started passing, and once `baseline > 1/3` the threshold
//!    exceeded 1.0, which `R ∈ [0, 1]` can never cross — an absorbing state
//!    with no way back.
//! 2. **The 500 ms window was never enforced.** `last_reference` was
//!    written but never read, so the streak counter and the EMA advanced
//!    per *call*, not per window. A 60 fps caller reached the saturation
//!    above in 0.37 s instead of 11 s, and "resume after 2 frames" meant
//!    33 ms instead of 1 s.

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
    /// `true` when this verdict repeats the previous one because the
    /// observation arrived before the window elapsed — no state advanced.
    pub coalesced: bool,
}

/// The gate state machine.
#[derive(Clone, Debug)]
pub struct MotionGate {
    /// Frame-difference window (spec: 500 ms). Observations arriving sooner
    /// than this after the last *accounted* one replay the previous verdict
    /// without advancing any state.
    pub window: Duration,
    /// Relative dynamic threshold (spec: `baseline * 3`).
    pub baseline_factor: f64,
    /// Absolute dynamic threshold (spec: 15%).
    pub absolute_threshold: f64,
    /// Consecutive passes required to resume after a dynamic episode
    /// (spec: 2).
    pub resume_passes: u32,
    /// Consecutive dynamic windows after which the gate reports that the
    /// region is persistently animated. This replaces the old "let the
    /// baseline absorb it" behavior, which was the self-destruct path.
    pub persistent_dynamic_windows: u32,
    /// Learned static-change-rate baseline.
    ///
    /// **Only passing (static) observations teach it.** A DYNAMIC frame
    /// must never raise the bar that judges it — that is a positive
    /// feedback loop, and it is precisely what made the P2-b gate saturate.
    baseline: f64,
    /// Upward baseline learning rate per accounted observation.
    learn_up: f64,
    /// Downward baseline learning rate per accounted observation.
    learn_down: f64,
    static_streak: u32,
    dynamic_streak: u32,
    armed: bool,
    /// Timestamp of the last observation that advanced the state.
    last_accounted: Option<Instant>,
    /// Verdict replayed for observations inside the window.
    last_verdict: Option<GateVerdict>,
}

impl Default for MotionGate {
    fn default() -> Self {
        MotionGate {
            window: Duration::from_millis(500),
            baseline_factor: 3.0,
            absolute_threshold: 0.15,
            resume_passes: 2,
            persistent_dynamic_windows: 6,
            baseline: 0.0,
            // Gentle both ways: a single quiet window must not throw away
            // the learned noise tolerance (the old `learn_down = 0.5`
            // halved it instantly and sawtoothed against `learn_up = 0.02`).
            learn_up: 0.05,
            learn_down: 0.15,
            static_streak: 0,
            dynamic_streak: 0,
            armed: true,
            last_accounted: None,
            last_verdict: None,
        }
    }
}

impl MotionGate {
    /// The largest threshold the gate will ever apply.
    ///
    /// Hard ceiling at 0.5: a region where more than half the pixels change
    /// per window is animated under any reasonable definition, and no
    /// learned baseline may argue otherwise.
    ///
    /// *Failure mode without it*: the threshold can be pushed above 1.0,
    /// which `R ∈ [0, 1]` cannot cross — the gate then passes everything
    /// forever with no path back (the P2-b defect).
    pub const MAX_THRESHOLD: f64 = 0.5;

    /// Feeds one change-rate observation (measured by the differ over the
    /// target region) and returns the verdict.
    ///
    /// Observations arriving sooner than [`window`](Self::window) after the
    /// last accounted one replay the previous verdict with
    /// `coalesced: true` and advance nothing: the spec's timings are stated
    /// in milliseconds, so they must not be reachable by calling faster.
    pub fn observe(&mut self, ratio: f64, now: Instant) -> GateVerdict {
        // Non-finite input is not a measurement. Refusing (as "dynamic")
        // rather than clamping keeps it out of the baseline entirely —
        // project convention 4: a guard must reject, not fabricate.
        if !ratio.is_finite() {
            // Invalid measurements must be refused even inside the coalescing
            // window; replaying a prior passing verdict would admit NaN/Inf.
            self.last_accounted = Some(now);
            self.static_streak = 0;
            self.dynamic_streak = self.dynamic_streak.saturating_add(1);
            self.armed = false;
            let verdict = GateVerdict {
                // Keep GateVerdict's documented [0, 1] output contract while
                // refusing the invalid input as maximally dynamic.
                changed_ratio: 1.0,
                passed: false,
                dynamic: true,
                coalesced: false,
            };
            self.last_verdict = Some(verdict);
            return verdict;
        }
        let ratio = ratio.clamp(0.0, 1.0);

        if let (Some(t), Some(prev)) = (self.last_accounted, self.last_verdict) {
            if now.duration_since(t) < self.window {
                return GateVerdict { changed_ratio: ratio, coalesced: true, ..prev };
            }
        }
        self.last_accounted = Some(now);

        let threshold = self.threshold();
        let verdict = if ratio > threshold {
            self.static_streak = 0;
            self.dynamic_streak = self.dynamic_streak.saturating_add(1);
            self.armed = false;
            // Deliberately NOT learning here — see the `baseline` field
            // comment. Letting a dynamic frame raise its own bar is the
            // positive feedback loop that killed the previous version.
            GateVerdict { changed_ratio: ratio, passed: false, dynamic: true, coalesced: false }
        } else {
            self.dynamic_streak = 0;
            self.static_streak = self.static_streak.saturating_add(1);
            let rate = if ratio > self.baseline { self.learn_up } else { self.learn_down };
            self.baseline += (ratio - self.baseline) * rate;
            // Spec §6.1's 2-pass rule governs *resuming after a dynamic
            // episode*. A cold start is not a dynamic episode, so the gate
            // starts armed and the first quiet window already emits.
            let passed = self.armed || self.static_streak >= self.resume_passes;
            if passed {
                self.armed = true;
            }
            GateVerdict { changed_ratio: ratio, passed, dynamic: false, coalesced: false }
        };
        self.last_verdict = Some(verdict);
        verdict
    }

    /// The threshold currently applied, clamped to
    /// [`MAX_THRESHOLD`](Self::MAX_THRESHOLD).
    pub fn threshold(&self) -> f64 {
        (self.baseline * self.baseline_factor)
            .max(self.absolute_threshold)
            .min(Self::MAX_THRESHOLD)
    }

    /// The learned static baseline (diagnostics).
    pub fn baseline(&self) -> f64 {
        self.baseline
    }

    /// Whether the gate is currently emitting (armed) measurements.
    pub fn is_armed(&self) -> bool {
        self.armed
    }

    /// `true` when the region has been dynamic for
    /// [`persistent_dynamic_windows`](Self::persistent_dynamic_windows)
    /// consecutive windows.
    ///
    /// This is the honest replacement for "absorb it into the baseline": a
    /// region animated for seconds on end is *reported* as such so the
    /// caller can re-anchor, pick another target, or tell the user. It is
    /// never silently reclassified as static.
    pub fn is_persistently_dynamic(&self) -> bool {
        self.dynamic_streak >= self.persistent_dynamic_windows
    }

    /// Consecutive dynamic windows observed (diagnostics).
    pub fn dynamic_streak(&self) -> u32 {
        self.dynamic_streak
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Advances past the window so each observation is accounted.
    fn tick(k: u32) -> Duration {
        Duration::from_millis(600 * k as u64)
    }

    #[test]
    fn cold_start_emits_immediately_then_resumes_after_two() {
        // Spec §6.1: the 2-pass rule is about resuming *after a dynamic
        // episode*. A cold start must not be punished — the old code's
        // "first frame never passes" was an artifact of `armed` being
        // written but never read, and the old test froze it in place
        // (project convention 5).
        let mut gate = MotionGate::default();
        let t0 = Instant::now();

        assert!(gate.observe(0.01, t0).passed, "cold start emits at once");

        let v = gate.observe(0.5, t0 + tick(1));
        assert!(!v.passed && v.dynamic);

        assert!(!gate.observe(0.02, t0 + tick(2)).passed, "1 of 2 resume passes");
        assert!(gate.observe(0.02, t0 + tick(3)).passed, "2 of 2 resume passes");
    }

    #[test]
    fn steady_animation_never_becomes_static() {
        // The P2-b regression: at R = 0.5 the baseline crossed 0.5/3 after
        // 22 observations, the animation started passing, and then the
        // threshold rose past 1.0 and the gate died for good.
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        for k in 1..=500 {
            let v = gate.observe(0.5, t0 + tick(k));
            assert!(v.dynamic && !v.passed, "animation passed the gate at observation {k}");
        }
        assert!(gate.threshold() <= MotionGate::MAX_THRESHOLD);
        assert!(gate.is_persistently_dynamic(), "persistent animation must be reported");
        // …and the gate still works afterwards: quiet content re-arms it.
        assert!(!gate.observe(0.01, t0 + tick(501)).passed);
        assert!(gate.observe(0.01, t0 + tick(502)).passed, "gate must recover");
    }

    #[test]
    fn threshold_can_never_exceed_the_ceiling() {
        // Terminal-state check (project convention 2): whatever the input
        // history, the threshold must stay crossable by R in [0, 1].
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        for k in 1..=1000 {
            gate.observe(if k % 3 == 0 { 0.49 } else { 0.48 }, t0 + tick(k));
            assert!(
                gate.threshold() <= MotionGate::MAX_THRESHOLD,
                "threshold {} escaped the ceiling at {k}",
                gate.threshold()
            );
        }
    }

    #[test]
    fn fast_callers_cannot_shrink_the_spec_timings() {
        // 60 fps against a 500 ms window: extra calls must coalesce, or
        // "2 frames to resume" degrades from 1 s to 33 ms and the
        // saturation attack gets 30x cheaper.
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        gate.observe(0.01, t0);
        let dynamic_at = t0 + tick(1);
        assert!(gate.observe(0.9, dynamic_at).dynamic);

        for k in 1..=29 {
            let v = gate.observe(0.01, dynamic_at + Duration::from_millis(16 * k));
            assert!(v.coalesced, "call {k} should have coalesced");
            assert!(!v.passed, "a coalesced call must not resume the gate");
        }
        assert!(!gate.observe(0.01, dynamic_at + Duration::from_millis(600)).passed);
        assert!(gate.observe(0.01, dynamic_at + Duration::from_millis(1200)).passed);
    }

    #[test]
    fn noisy_static_region_is_absorbed_without_breaking_the_gate() {
        // The legitimate need the old saturation hack served: a steadily
        // noisy but static region must not deadlock the gate forever. Now
        // it is absorbed through *passing* observations only, and the
        // ceiling keeps the gate alive.
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        for k in 1..=60 {
            gate.observe(0.14, t0 + tick(k));
        }
        assert!(gate.baseline() > 0.1, "baseline learned from static frames");
        assert!(gate.threshold() > 0.15, "threshold lifted above the absolute floor");
        assert!(gate.observe(0.18, t0 + tick(61)).passed, "18% noise now tolerated");
        assert!(gate.observe(0.8, t0 + tick(62)).dynamic, "a real spike is still caught");
    }

    #[test]
    fn non_finite_input_is_refused_not_absorbed() {
        let mut gate = MotionGate::default();
        let t0 = Instant::now();
        for (name, ratio) in [("NaN", f64::NAN), ("+Inf", f64::INFINITY), ("-Inf", f64::NEG_INFINITY)] {
            let v = gate.observe(ratio, t0 + tick(if name == "NaN" { 0 } else if name == "+Inf" { 1 } else { 2 }));
            assert!(v.dynamic && !v.passed, "{name} must not be treated as a quiet measurement");
            assert_eq!(gate.baseline(), 0.0, "{name} must not poison the baseline");
        }
    }
}
