use fidus_core::calibration::{CalibrationMethod, CalibrationStatus, UnsupportedReason};
use fidus_core::env::{CompositorKind, EnvironmentContext, PermissionState};
use fidus_core::gate::{Gate, ProbeGate};

fn niri_env() -> EnvironmentContext {
    EnvironmentContext {
        has_layer_shell: true,
        is_dynamic_wallpaper: None,
        multi_monitor_count: 1,
        compositor_type: CompositorKind::Niri,
        screen_capture_permission: PermissionState::Granted,
        wayland_input_region_supported: true,
        multi_marker_projection: false,
    }
}

fn x11_env() -> EnvironmentContext {
    EnvironmentContext {
        has_layer_shell: false,
        is_dynamic_wallpaper: None,
        multi_monitor_count: 1,
        compositor_type: CompositorKind::X11,
        screen_capture_permission: PermissionState::Granted,
        wayland_input_region_supported: false,
        multi_marker_projection: true,
    }
}

#[test]
fn layer_shell_present_means_crosshair_available() {
    let gate = ProbeGate::from_environment(niri_env());
    assert_eq!(
        gate.query_calibrator_availability(CalibrationMethod::Crosshair),
        CalibrationStatus::Available { method: CalibrationMethod::Crosshair }
    );
}

#[test]
fn missing_layer_shell_is_reported_with_the_protocol_name() {
    let env = EnvironmentContext { has_layer_shell: false, ..niri_env() };
    let gate = ProbeGate::from_environment(env);
    match gate.query_calibrator_availability(CalibrationMethod::Crosshair) {
        CalibrationStatus::NotSupported {
            reason: UnsupportedReason::MissingProtocol { protocol },
            ..
        } => assert_eq!(protocol, "zwlr_layer_shell_v1"),
        other => panic!("unexpected status: {other:?}"),
    }
}

#[test]
fn multi_monitor_degrades_but_stays_usable() {
    let env = EnvironmentContext { multi_monitor_count: 2, ..niri_env() };
    let gate = ProbeGate::from_environment(env);
    let status = gate.query_calibrator_availability(CalibrationMethod::Crosshair);
    assert!(status.is_usable());
    assert!(matches!(status, CalibrationStatus::Degraded { estimated_confidence, .. } if estimated_confidence < 1.0));
}

#[test]
fn dynamic_wallpaper_degrades_crosshair_mildly() {
    let env = EnvironmentContext { is_dynamic_wallpaper: Some(true), ..niri_env() };
    let status = ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Crosshair);
    assert!(status.is_usable());
    assert!(matches!(status, CalibrationStatus::Degraded { estimated_confidence, .. } if estimated_confidence >= 0.8));
    // Unknown wallpaper state is not a degradation.
    let env = EnvironmentContext { is_dynamic_wallpaper: None, ..niri_env() };
    assert!(matches!(
        ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Crosshair),
        CalibrationStatus::Available { .. }
    ));
}

#[test]
fn revoked_capture_permission_requires_permission_flow() {
    let env = EnvironmentContext {
        screen_capture_permission: PermissionState::Revoked,
        ..niri_env()
    };
    let gate = ProbeGate::from_environment(env);
    assert!(matches!(
        gate.query_calibrator_availability(CalibrationMethod::Crosshair),
        CalibrationStatus::PermissionRequired { .. }
    ));
}

#[test]
fn gradient_field_is_honestly_feature_gated() {
    let gate = ProbeGate::from_environment(niri_env());
    assert!(matches!(
        gate.query_calibrator_availability(CalibrationMethod::GradientField),
        CalibrationStatus::NotSupported { reason: UnsupportedReason::FeatureDisabled { .. }, .. }
    ));
}

#[test]
fn anchor_needs_multi_marker_projection_not_a_platform_name() {
    // Single-surface backend (layer-shell): L0 cannot place four sentinels.
    let gate = ProbeGate::from_environment(niri_env());
    assert!(matches!(
        gate.query_calibrator_availability(CalibrationMethod::Anchor),
        CalibrationStatus::NotSupported { reason: UnsupportedReason::MissingPrimitive { .. }, .. }
    ));

    // The capability decides, not the compositor identity: an unknown
    // platform that provides the primitives qualifies.
    let env = EnvironmentContext { compositor_type: CompositorKind::Unknown, ..x11_env() };
    assert_eq!(
        ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Anchor),
        CalibrationStatus::Available { method: CalibrationMethod::Anchor }
    );

    // Layer-shell absence does not concern L0; Crosshair stays unavailable.
    let gate = ProbeGate::from_environment(x11_env());
    assert!(gate.query_calibrator_availability(CalibrationMethod::Anchor).is_usable());
    assert!(!gate.query_calibrator_availability(CalibrationMethod::Crosshair).is_usable());
}

#[test]
fn anchor_respects_capture_permission_and_degrades_on_dynamic_wallpaper() {
    let env = EnvironmentContext {
        screen_capture_permission: PermissionState::Revoked,
        ..x11_env()
    };
    assert!(matches!(
        ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Anchor),
        CalibrationStatus::PermissionRequired { .. }
    ));

    let env = EnvironmentContext { is_dynamic_wallpaper: Some(true), ..x11_env() };
    assert!(matches!(
        ProbeGate::from_environment(env).query_calibrator_availability(CalibrationMethod::Anchor),
        CalibrationStatus::Degraded { .. }
    ));
}

#[test]
fn compositor_kind_parses_colon_lists() {
    assert_eq!(CompositorKind::parse("niri"), CompositorKind::Niri);
    assert_eq!(CompositorKind::parse("niri:wlroots"), CompositorKind::Niri);
    assert_eq!(CompositorKind::parse("KDE"), CompositorKind::KWin);
    assert_eq!(CompositorKind::parse("GNOME"), CompositorKind::Mutter);
    assert_eq!(CompositorKind::parse(""), CompositorKind::Unknown);
    assert_eq!(CompositorKind::parse("potato"), CompositorKind::Other("potato".into()));
}

#[test]
fn merge_prefers_probe_capabilities_and_caller_knowledge() {
    let probe = niri_env();
    let caller = EnvironmentContext {
        is_dynamic_wallpaper: Some(true),
        multi_monitor_count: 0, // caller knows nothing; must not win
        ..EnvironmentContext::default()
    };
    let merged = EnvironmentContext::merge(probe, caller);
    assert_eq!(merged.compositor_type, CompositorKind::Niri);
    assert_eq!(merged.multi_monitor_count, 1);
    assert_eq!(merged.is_dynamic_wallpaper, Some(true));
}
