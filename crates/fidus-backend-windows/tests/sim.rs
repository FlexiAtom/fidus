// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! No-display simulation tests for the Windows backend (AGENTS §6: every
//! pitfall gets a regression that needs no display server).
//!
//! These run on **any** host, including the Linux CI gate: the projector, the
//! capability mapping and the two pure conversions are platform-neutral on
//! purpose. The real Win32 path is covered by the live probe
//! (`fidus-windows-probe`) on a Windows host.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fidus_backend_windows::api::{
    environment_context, DpiAwareness, MarkerId, MarkerSpec, Platform, PlatformError, ProbeFacts,
    RawFrame, WorkArea,
};
use fidus_backend_windows::plan::{plan_marker, raw_to_frame, MAX_MARKER_EDGE};
use fidus_backend_windows::projector::Projector;
use fidus_core::calibration::{CalibrationMethod, CalibrationStatus, UnsupportedReason};
use fidus_core::coord::LogicalPoint;
use fidus_core::env::{CompositorKind, PermissionState};
use fidus_core::gate::{Gate, ProbeGate};
use fidus_core::io::{MarkerShape, MarkerStyle, PixelFormat};

// ------------------------------------------------------------------- fakes

struct Fake {
    facts: ProbeFacts,
    log: Mutex<Vec<String>>,
    live: Mutex<Vec<u64>>,
    next_id: AtomicU64,
    /// 1-based index of the `create_marker` call that should fail; 0 = none.
    fail_create_on: AtomicU64,
    fail_present: AtomicBool,
    fail_destroy: AtomicBool,
    fail_sync: AtomicBool,
    fail_capture: AtomicBool,
    create_calls: AtomicU64,
}

impl Fake {
    fn blank(facts: ProbeFacts) -> Arc<Self> {
        Arc::new(Self {
            facts,
            log: Mutex::new(Vec::new()),
            live: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(0),
            fail_create_on: AtomicU64::new(0),
            fail_present: AtomicBool::new(false),
            fail_destroy: AtomicBool::new(false),
            fail_sync: AtomicBool::new(false),
            fail_capture: AtomicBool::new(false),
            create_calls: AtomicU64::new(0),
        })
    }

    fn new() -> Arc<Self> {
        Self::blank(ProbeFacts {
            dpi_awareness: DpiAwareness::PerMonitorV2,
            monitors: 1,
            work_area: WorkArea::new(0, 0, 1920, 1032),
            capture_probe: Ok(()),
        })
    }

    fn record(&self, entry: impl Into<String>) {
        self.log.lock().expect("log mutex").push(entry.into());
    }

    fn entries(&self) -> Vec<String> {
        self.log.lock().expect("log mutex").clone()
    }

    fn live_count(&self) -> usize {
        self.live.lock().expect("live mutex").len()
    }
}

impl Platform for Fake {
    fn facts(&self) -> &ProbeFacts {
        &self.facts
    }

    fn capture_work_area(&self) -> Result<RawFrame, PlatformError> {
        self.record("capture");
        if self.fail_capture.load(Ordering::SeqCst) {
            return Err(PlatformError::new("simulated BitBlt failure"));
        }
        Ok(RawFrame { width: 2, height: 2, stride: 8, data: vec![0u8; 16] })
    }

    fn create_marker(&self, spec: &MarkerSpec) -> Result<MarkerId, PlatformError> {
        let call = self.create_calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.record(format!("create:{}x{}@{},{}", spec.size, spec.size, spec.x, spec.y));
        if self.fail_create_on.load(Ordering::SeqCst) == call {
            return Err(PlatformError::new("simulated CreateWindowExW failure"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.live.lock().expect("live mutex").push(id);
        Ok(MarkerId(id))
    }

    fn present_marker(&self, id: MarkerId) -> Result<(), PlatformError> {
        self.record("present");
        if self.fail_present.load(Ordering::SeqCst) {
            return Err(PlatformError::new("simulated ShowWindow failure"));
        }
        let _ = id;
        Ok(())
    }

    fn destroy_marker(&self, id: MarkerId) -> Result<(), PlatformError> {
        self.record("destroy");
        if self.fail_destroy.load(Ordering::SeqCst) {
            return Err(PlatformError::new("simulated DestroyWindow failure"));
        }
        self.live.lock().expect("live mutex").retain(|other| *other != id.0);
        Ok(())
    }

    fn sync_presentation(&self) -> Result<(), PlatformError> {
        self.record("sync");
        if self.fail_sync.load(Ordering::SeqCst) {
            return Err(PlatformError::new("simulated DwmFlush failure"));
        }
        Ok(())
    }

    fn live_marker_windows(&self) -> usize {
        self.live_count()
    }

    fn point_intercepted_by_marker(&self, x: i32, y: i32) -> bool {
        // The fake owns no real windows; the live probe measures this on a
        // Windows host.
        self.record(format!("hit-test@{x},{y}"));
        false
    }
}

/// Probe facts substituted for a test that only cares about the capability
/// mapping, without rebuilding the fixture by hand.
impl Fake {
    fn with_facts(facts: ProbeFacts) -> Arc<Self> {
        Self::blank(facts)
    }
}

fn marker(x: f64, y: f64) -> MarkerSpec {
    plan_marker(LogicalPoint::new(x, y), MarkerStyle::DEFAULT, WorkArea::new(0, 0, 1920, 1032))
        .expect("marker plans")
}

fn facts(capture: Result<(), String>, monitors: usize) -> ProbeFacts {
    ProbeFacts {
        dpi_awareness: DpiAwareness::PerMonitorV2,
        monitors,
        work_area: WorkArea::new(0, 0, 1920, 1032),
        capture_probe: capture,
    }
}

// ------------------------------------------------- presentation and teardown

/// The measured pitfall: without a presentation sync a capture saw a just-shown
/// marker in only 8/20 trials. Deleting the sync must turn this red.
#[test]
fn projection_presents_then_synchronises_before_returning() {
    let fake = Fake::new();
    let projector = Projector::start(fake.clone()).expect("thread starts");
    projector.show(vec![marker(10.0, 20.0)]).expect("show succeeds");
    assert_eq!(fake.entries(), ["create:28x28@10,20", "present", "sync"]);
}

#[test]
fn show_replaces_the_previous_set_with_one_presentation_sync() {
    let fake = Fake::new();
    let projector = Projector::start(fake.clone()).expect("thread starts");
    projector.show(vec![marker(10.0, 20.0)]).expect("first show");
    projector.show(vec![marker(30.0, 40.0)]).expect("second show");
    // Removal and replacement are covered by a single sync: the intermediate
    // state is never observed, so waiting for it would be a wasted frame.
    assert_eq!(
        fake.entries(),
        [
            "create:28x28@10,20",
            "present",
            "sync",
            "destroy",
            "create:28x28@30,40",
            "present",
            "sync",
        ]
    );
    assert_eq!(fake.live_count(), 1, "only the replacement survives");
}

#[test]
fn a_window_that_refused_to_die_is_retried_rather_than_forgotten() {
    let fake = Fake::new();
    let projector = Projector::start(fake.clone()).expect("thread starts");
    projector.show(vec![marker(1.0, 1.0)]).expect("show");

    // First teardown fails: the handle must stay tracked, not be dropped.
    fake.fail_destroy.store(true, Ordering::SeqCst);
    let error = projector.clear().expect_err("teardown failure is reported");
    assert!(error.to_string().contains("DestroyWindow"), "got {error}");
    assert_eq!(fake.live_count(), 1, "the survivor is still tracked");

    // A later teardown must retry it and succeed.
    fake.fail_destroy.store(false, Ordering::SeqCst);
    projector.clear().expect("retry succeeds");
    assert_eq!(fake.live_count(), 0, "no residual window is left behind");
}

#[test]
fn empty_show_and_repeated_clear_make_no_platform_calls() {
    let fake = Fake::new();
    let projector = Projector::start(fake.clone()).expect("thread starts");
    // Nothing projected: clearing is a no-op, not a spurious destroy+sync.
    projector.clear().expect("clear with nothing projected");
    projector.show(Vec::new()).expect("empty show");
    assert!(fake.entries().is_empty(), "got {:?}", fake.entries());

    projector.show(vec![marker(1.0, 1.0)]).expect("show");
    projector.clear().expect("clear");
    let after_first_clear = fake.entries();
    projector.clear().expect("clear again");
    assert_eq!(fake.entries(), after_first_clear, "clear is idempotent");
    assert_eq!(fake.live_count(), 0);
}

#[test]
fn drop_tears_down_on_the_owner_thread_and_joins() {
    let fake = Fake::new();
    {
        let projector = Projector::start(fake.clone()).expect("thread starts");
        projector
            .show(vec![marker(1.0, 1.0), marker(2.0, 2.0)])
            .expect("show two markers");
        assert_eq!(fake.live_count(), 2);
    }
    // Drop has returned, therefore the thread has joined and nothing is left.
    assert_eq!(fake.live_count(), 0, "no residual marker window after Drop");
    assert_eq!(fake.entries().last().map(String::as_str), Some("sync"));
}

#[test]
fn a_foreign_thread_can_drive_the_projector() {
    let fake = Fake::new();
    let projector = Arc::new(Projector::start(fake.clone()).expect("thread starts"));
    let driver = Arc::clone(&projector);
    std::thread::spawn(move || {
        driver.show(vec![marker(5.0, 5.0)]).expect("show from a foreign thread");
        driver.clear().expect("clear from a foreign thread");
    })
    .join()
    .expect("driver thread");
    assert_eq!(fake.live_count(), 0);
    assert!(fake.entries().contains(&"present".to_string()));
}

#[test]
fn failures_are_reported_and_never_leave_a_marker_on_screen() {
    // A mid-batch creation failure: the markers already created are torn down.
    let fake = Fake::new();
    fake.fail_create_on.store(2, Ordering::SeqCst);
    let projector = Projector::start(fake.clone()).expect("thread starts");
    let error = projector
        .show(vec![marker(1.0, 1.0), marker(2.0, 2.0)])
        .expect_err("second creation fails");
    assert!(error.to_string().contains("CreateWindowExW"), "got {error}");
    assert_eq!(fake.live_count(), 0, "fail closed");

    // A presentation failure is reported and cleaned up too.
    let fake = Fake::new();
    fake.fail_present.store(true, Ordering::SeqCst);
    let projector = Projector::start(fake.clone()).expect("thread starts");
    let error = projector.show(vec![marker(1.0, 1.0)]).expect_err("presentation fails");
    assert!(error.to_string().contains("ShowWindow"), "got {error}");
    assert_eq!(fake.live_count(), 0);

    // A failed presentation sync is surfaced rather than swallowed: the caller
    // must not be told "shown" when it is not.
    let fake = Fake::new();
    fake.fail_sync.store(true, Ordering::SeqCst);
    let projector = Projector::start(fake.clone()).expect("thread starts");
    assert!(projector.show(vec![marker(1.0, 1.0)]).is_err());
}

// ------------------------------------------------------ placement and frames

#[test]
fn marker_placement_is_quantised_once_and_offset_by_the_work_area() {
    let area = WorkArea::new(100, 50, 1920, 1032);
    let style = MarkerStyle { rgba: [255, 0, 255, 255], size_logical: 8.0, shape: MarkerShape::SolidSquare };

    let spec = plan_marker(LogicalPoint::new(10.4, 20.6), style, area).expect("plans");
    assert_eq!((spec.x, spec.y, spec.size), (110, 71, 8), "round once, offset by the usable area");

    let spec = plan_marker(LogicalPoint::new(-10.0, -20.0), style, area).expect("plans");
    assert_eq!((spec.x, spec.y), (90, 30), "negative logical positions are allowed and exact");
}

#[test]
fn marker_placement_refuses_what_it_cannot_project_exactly() {
    let area = WorkArea::new(0, 0, 100, 100);
    let style = |size: f64, shape: MarkerShape| MarkerStyle { rgba: [1, 2, 3, 255], size_logical: size, shape };

    assert!(plan_marker(
        LogicalPoint::new(0.0, 0.0),
        style(8.0, MarkerShape::Crosshair),
        area
    )
    .is_err(), "crosshair is not implemented on Windows and must be refused, not faked");

    for size in [f64::NAN, f64::INFINITY, 0.0, -1.0, MAX_MARKER_EDGE + 1.0] {
        assert!(
            plan_marker(LogicalPoint::new(0.0, 0.0), style(size, MarkerShape::SolidSquare), area).is_err(),
            "edge {size} must be refused"
        );
    }
    assert!(
        plan_marker(LogicalPoint::new(0.0, 0.0), style(0.4, MarkerShape::SolidSquare), area).is_err(),
        "an edge that rounds to zero pixels must be refused, not inflated"
    );
    assert!(
        plan_marker(LogicalPoint::new(f64::NAN, 0.0), style(8.0, MarkerShape::SolidSquare), area).is_err()
    );
}

#[test]
fn raw_capture_is_adopted_top_down_as_xrgb_and_malformed_buffers_are_refused() {
    let good = RawFrame { width: 2, height: 2, stride: 8, data: vec![7u8; 16] };
    let frame = raw_to_frame(good).expect("valid frame");
    assert_eq!(frame.format, PixelFormat::Xrgb8888);
    assert_eq!(frame.size(), (2, 2));
    assert!(frame.is_valid());

    let zero = RawFrame { width: 0, height: 2, stride: 0, data: Vec::new() };
    assert!(raw_to_frame(zero).is_err(), "a zero-extent capture is an error, not a frame");

    let short_stride = RawFrame { width: 2, height: 2, stride: 4, data: vec![0; 16] };
    assert!(raw_to_frame(short_stride).is_err());

    let truncated = RawFrame { width: 2, height: 2, stride: 8, data: vec![0; 15] };
    assert!(raw_to_frame(truncated).is_err(), "a truncated buffer must not become pixels");

    // `Xrgb8888` reads B,G,R,X little-endian; presence of pixels does not
    // depend on the X byte.
    let styled = RawFrame {
        width: 1,
        height: 1,
        stride: 4,
        data: vec![255, 0, 255, 0],
    };
    let frame = raw_to_frame(styled).expect("valid frame");
    assert_eq!(frame.rgba_at(0, 0), [255, 0, 255, 255]);
}

// ------------------------------------------------------------- capability

#[test]
fn capabilities_tell_the_gate_what_this_backend_can_actually_do() {
    let env = environment_context(&facts(Ok(()), 1));
    assert_eq!(env.compositor_type, CompositorKind::Windows);
    assert!(!env.has_layer_shell, "Windows has no layer shell and must not claim one");
    assert!(env.multi_marker_projection, "one window per marker");
    assert!(!env.wayland_input_region_supported);
    assert_eq!(env.screen_capture_permission, PermissionState::Granted);

    let gate = ProbeGate::from_environment(env);
    let crosshair = gate.query_calibrator_availability(CalibrationMethod::Crosshair);
    match crosshair {
        CalibrationStatus::NotSupported { reason: UnsupportedReason::MissingProtocol { protocol }, .. } => {
            assert_eq!(protocol, "zwlr_layer_shell_v1", "L9 must be refused for the missing primitive");
        }
        other => panic!("expected a missing-protocol refusal, got {other:?}"),
    }
    assert!(
        gate.query_calibrator_availability(CalibrationMethod::Anchor).is_usable(),
        "L0 is the universal fallback and must qualify on capability alone"
    );
}

#[test]
fn a_failed_capture_probe_revokes_the_permission_l0_needs() {
    let env = environment_context(&facts(Err("BitBlt failed".into()), 1));
    assert_eq!(env.screen_capture_permission, PermissionState::Revoked);
    let gate = ProbeGate::from_environment(env);

    // L0 Anchor is the calibrator Windows can run, so the refusal has to name
    // the missing permission rather than fail half-way through a calibration.
    match gate.query_calibrator_availability(CalibrationMethod::Anchor) {
        CalibrationStatus::PermissionRequired { permission, .. } => {
            assert_eq!(permission.name(), "screen capture");
        }
        other => panic!("expected a permission refusal for Anchor, got {other:?}"),
    }

    // L9 is refused for the missing layer shell *before* the permission is
    // consulted: on Windows it cannot run even with capture granted, and saying
    // "permission required" would suggest that granting it would help.
    match gate.query_calibrator_availability(CalibrationMethod::Crosshair) {
        CalibrationStatus::NotSupported {
            reason: UnsupportedReason::MissingProtocol { protocol },
            ..
        } => assert_eq!(protocol, "zwlr_layer_shell_v1"),
        other => panic!("expected a missing-protocol refusal for Crosshair, got {other:?}"),
    }
}

#[test]
fn no_display_is_reported_as_such() {
    let env = environment_context(&facts(Ok(()), 0));
    assert_eq!(
        ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Anchor),
        CalibrationStatus::NotSupported {
            method: CalibrationMethod::Anchor,
            reason: UnsupportedReason::NoDisplay,
        }
    );
}

#[test]
fn dpi_awareness_is_reported_honestly() {
    assert!(DpiAwareness::PerMonitorV2.is_physical());
    assert!(DpiAwareness::System.is_physical());
    assert!(!DpiAwareness::None.is_physical());
    let mut probe = facts(Ok(()), 1);
    probe.dpi_awareness = DpiAwareness::None;
    assert!(!probe.dpi_awareness.is_physical());
    assert!(probe.capture_ok());
    assert_eq!(probe.capture_error(), None);
    assert_eq!(ProbeFacts::capture_error(&facts(Err("x".into()), 1)), Some("x"));
}

#[test]
fn probe_facts_substitution_keeps_monitor_and_capture_state() {
    // `Fake::with_facts` lets a test substitute probe facts without rebuilding
    // the fixture; this guards that the substitution actually takes effect.
    let fake = Fake::with_facts(facts(Err("no desktop".into()), 3));
    assert_eq!(fake.facts().monitors, 3);
    assert!(!fake.facts().capture_ok());
    assert_eq!(fake.facts().capture_error(), Some("no desktop"));
    assert_eq!(fake.live_marker_windows(), 0);
    assert!(!fake.point_intercepted_by_marker(0, 0));
}
