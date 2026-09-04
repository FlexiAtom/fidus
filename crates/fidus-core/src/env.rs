//! Environment description and permission model (spec §5.2, §5.3).

/// Coarse compositor identification for diagnostics and strategy hints.
///
/// Detection is heuristic (desktop-environment environment variables) and is
/// only ever used for reporting and policy selection — never as a coordinate
/// source, so it does not violate zero-trust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositorKind {
    /// Niri.
    Niri,
    /// Sway.
    Sway,
    /// Hyprland.
    Hyprland,
    /// KWin (KDE Plasma).
    KWin,
    /// Mutter (GNOME).
    Mutter,
    /// Weston.
    Weston,
    /// labwc.
    Labwc,
    /// Wayfire.
    Wayfire,
    /// Something else; free-form identifier.
    Other(String),
    /// Could not be determined.
    Unknown,
}

impl CompositorKind {
    /// Detects the compositor heuristically from `XDG_CURRENT_DESKTOP` /
    /// `XDG_SESSION_DESKTOP`. Colon-separated values (e.g. `"niri:wlroots"`)
    /// are tokenized before matching.
    pub fn detect_from_env() -> Self {
        let mut val = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if val.is_empty() {
            val = std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default();
        }
        Self::parse(&val)
    }

    /// Parses a desktop identifier string (case-insensitive, `:`-separated).
    pub fn parse(value: &str) -> Self {
        for token in value.split(':') {
            match token.to_ascii_lowercase().as_str() {
                "niri" => return CompositorKind::Niri,
                "sway" => return CompositorKind::Sway,
                "hyprland" => return CompositorKind::Hyprland,
                "kde" | "plasma" | "kwin" => return CompositorKind::KWin,
                "gnome" | "unity" | "mutter" => return CompositorKind::Mutter,
                "weston" => return CompositorKind::Weston,
                "labwc" => return CompositorKind::Labwc,
                "wayfire" => return CompositorKind::Wayfire,
                _ => {}
            }
        }
        if value.is_empty() {
            CompositorKind::Unknown
        } else {
            CompositorKind::Other(value.to_string())
        }
    }
}

/// Lifecycle state of a platform permission (spec §5.3 state machine).
///
/// ```text
/// Unknown → Requesting → Granted → Revoked → RequiresRestart
/// ```
///
/// macOS requires a process restart after permission changes; Linux Portals
/// re-ask on every use. The state is exposed so callers can drive UX.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PermissionState {
    /// Not probed yet.
    #[default]
    Unknown,
    /// A permission request is in flight.
    Requesting,
    /// Granted and effective.
    Granted,
    /// Was granted, then revoked by the user.
    Revoked,
    /// Changed since process start; a restart is required for it to take
    /// effect (macOS-specific behavior).
    RequiresRestart,
}

impl PermissionState {
    /// `true` when capture may proceed right now.
    pub fn is_effective(self) -> bool {
        matches!(self, PermissionState::Granted)
    }
}

/// A permission fidus may need from the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionType {
    /// Generic screen capture permission (macOS screen recording, Portal
    /// ScreenCast consent, …).
    ScreenCapture,
}

impl PermissionType {
    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            PermissionType::ScreenCapture => "screen capture",
        }
    }
}

/// Description of the runtime environment (spec §5.2).
///
/// This is the *only* environmental input fidus accepts: it describes
/// capabilities and conditions but carries no coordinate value, so the
/// zero-trust rule ("no native coordinate input in public APIs") is upheld at
/// the type level.
#[derive(Clone, Debug, PartialEq)]
pub struct EnvironmentContext {
    /// Whether `zwlr_layer_shell_v1` (or equivalent) is available. Required
    /// by the L9 Crosshair calibrator.
    pub has_layer_shell: bool,
    /// Whether the desktop shows dynamic (video/animated) wallpaper.
    /// `None` = unknown; callers may supply this knowledge.
    pub is_dynamic_wallpaper: Option<bool>,
    /// Number of outputs detected.
    pub multi_monitor_count: usize,
    /// Heuristic compositor identification.
    pub compositor_type: CompositorKind,
    /// State of the screen-capture permission.
    pub screen_capture_permission: PermissionState,
    /// Whether input regions of projected surfaces are honored (click-through
    /// support).
    pub wayland_input_region_supported: bool,
}

impl Default for EnvironmentContext {
    fn default() -> Self {
        Self {
            has_layer_shell: false,
            is_dynamic_wallpaper: None,
            multi_monitor_count: 0,
            compositor_type: CompositorKind::Unknown,
            screen_capture_permission: PermissionState::Unknown,
            wayland_input_region_supported: true,
        }
    }
}

impl EnvironmentContext {
    /// Merges caller-provided knowledge (`caller`) over backend probe results
    /// (`probe`).
    ///
    /// Capability fields always come from the probe (the backend is
    /// authoritative about what it can actually do). Only caller-supplied
    /// *knowledge* that the probe cannot obtain — currently
    /// [`is_dynamic_wallpaper`](EnvironmentContext::is_dynamic_wallpaper) —
    /// is taken from `caller` when the probe reports `None`.
    pub fn merge(probe: Self, caller: Self) -> Self {
        EnvironmentContext {
            is_dynamic_wallpaper: caller.is_dynamic_wallpaper.or(probe.is_dynamic_wallpaper),
            ..probe
        }
    }
}
