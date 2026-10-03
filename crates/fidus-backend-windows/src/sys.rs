// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! The Win32 half: DPI awareness, the primary work area, layered marker
//! windows, and screen-DC capture.
//!
//! # What this module deliberately does not call
//!
//! `GetWindowRect`, `GetClientRect`, `ClientToScreen` — and any other API that
//! reports where a window is. The only platform geometry that enters fidus from
//! here is the primary monitor's **work area** (`SPI_GETWORKAREA`), used as a
//! *placement* offset and as a size *hint*: if it were wrong, markers would land
//! outside the frame, go undetected, and calibration would retry. It never
//! enters a coordinate frame. Placement itself is the other direction — we tell
//! Windows where our window goes.
//!
//! # Why the window procedure is bare
//!
//! The marker's pixels come from `UpdateLayeredWindow`, not from `WM_PAINT`, so
//! there is no painting code to get wrong. `WM_NCHITTEST` is **not** overridden
//! on purpose: measurement showed the click-through of a layered
//! `WS_EX_TRANSPARENT` window happens in hit-test selection (`WindowFromPoint`
//! skips it), while `DefWindowProc` still answers `HTCLIENT`. Returning
//! `HTTRANSPARENT` by hand would *not* be the real mechanism — that code only
//! passes the mouse to windows **of the same thread** — so it would look like
//! click-through without being it.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmFlush;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// `BOOL` lives in `windows_sys::core` (it is not re-exported by `Foundation`).
use windows_sys::core::BOOL;

use crate::api::{
    DpiAwareness, MarkerId, MarkerSpec, Platform, PlatformError, ProbeFacts, RawFrame, WorkArea,
};

/// Window class shared by every marker window of this process.
pub(crate) const MARKER_CLASS: &str = "fidus_marker_window";

/// `ERROR_CLASS_ALREADY_EXISTS`: window classes are **process-global**, so a
/// second backend instance re-registering the same class is success, not
/// failure. (Measured: the first probe run failed every later cell on this.)
const ERROR_CLASS_ALREADY_EXISTS: i32 = 1410;

/// `DWM_E_COMPOSITIONDISABLED`: nothing to synchronise against.
const DWM_E_COMPOSITIONDISABLED: i32 = 0x8026_3001u32 as i32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_err() -> String {
    std::io::Error::last_os_error().to_string()
}

fn last_err_code() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The connected host. Holds no window handles: [`MarkerId`] carries each one
/// as an integer, created and destroyed only by the projector thread.
pub struct Win32 {
    facts: ProbeFacts,
    class: Vec<u16>,
    /// `HINSTANCE` kept as an integer so the type stays `Send + Sync`.
    hinst: usize,
}

impl Win32 {
    /// Sets DPI awareness, probes the work area and the capture primitive.
    pub fn connect() -> Result<Self, PlatformError> {
        let dpi_awareness = setup_dpi_awareness();
        let work_area = primary_work_area()?;
        let monitors = unsafe { GetSystemMetrics(SM_CMONITORS) }.max(0) as usize;
        // A real 1×1 capture, at connect time: the gate must be able to refuse
        // honestly instead of failing half-way through a calibration.
        let capture_probe = probe_capture(work_area);
        let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
        if hinst.is_null() {
            return Err(PlatformError::new(format!("GetModuleHandleW failed: {}", last_err())));
        }
        let class = wide(MARKER_CLASS);
        register_class(&class, hinst)?;
        Ok(Self {
            facts: ProbeFacts { dpi_awareness, monitors, work_area, capture_probe },
            class,
            hinst: hinst as usize,
        })
    }
}

/// Asks for per-monitor v2 (coordinates *are* physical pixels), falling back
/// honestly.
fn setup_dpi_awareness() -> DpiAwareness {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0 {
            return DpiAwareness::PerMonitorV2;
        }
        if SetProcessDPIAware() != 0 {
            return DpiAwareness::System;
        }
        DpiAwareness::None
    }
}

/// Usable area of the primary display: the Windows counterpart of the
/// compositor-announced usable area.
fn primary_work_area() -> Result<WorkArea, PlatformError> {
    let mut rect = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut RECT as *mut c_void, 0)
    };
    if ok == 0 {
        return Err(PlatformError::new(format!(
            "SystemParametersInfoW(SPI_GETWORKAREA) failed: {}",
            last_err()
        )));
    }
    let area = WorkArea::new(rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top);
    if !area.is_usable() {
        return Err(PlatformError::new("the primary work area is empty"));
    }
    Ok(area)
}

/// One 1×1 `BitBlt` from the screen DC, to learn whether capture works at all.
fn probe_capture(area: WorkArea) -> Result<(), String> {
    capture_rect(WorkArea::new(area.left, area.top, 1, 1), SRCCOPY | CAPTUREBLT)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn register_class(class: &[u16], hinst: HINSTANCE) -> Result<(), PlatformError> {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(marker_wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: std::ptr::null_mut(),
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 && last_err_code() != ERROR_CLASS_ALREADY_EXISTS {
        return Err(PlatformError::new(format!("RegisterClassExW failed: {}", last_err())));
    }
    Ok(())
}

/// Window styles for a marker, each with the reason it is there.
fn ex_style() -> u32 {
    WS_EX_LAYERED         // UpdateLayeredWindow, and the measured click-through
        | WS_EX_TOPMOST   // above the caller's own window
        | WS_EX_TOOLWINDOW // no taskbar button, no Alt+Tab entry
        | WS_EX_NOACTIVATE // never take focus (measured: foreground unchanged)
        | WS_EX_TRANSPARENT // click-through together with WS_EX_LAYERED
}

/// Marker windows are pure output: never erase (the pixels are handed to DWM by
/// `UpdateLayeredWindow`), and let `DefWindowProc` answer everything else.
unsafe extern "system" fn marker_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Allocates a 32-bpp **top-down** DIB (`biHeight` negative) selected into
/// `mem`, and returns its pixel pointer plus the previous selection.
///
/// # Safety
///
/// `mem` must be a live device context.
unsafe fn make_dib(
    mem: HDC,
    width: i32,
    height: i32,
) -> Result<(HBITMAP, *mut c_void, HGDIOBJ), PlatformError> {
    // Written out field by field: every value here is load-bearing for the
    // "top-down, 32-bpp, uncompressed" reading that `RawFrame` promises.
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // negative height => rows top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD::default()],
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = unsafe {
        CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0)
    };
    if dib.is_null() || bits.is_null() {
        return Err(PlatformError::new(format!("CreateDIBSection failed: {}", last_err())));
    }
    let previous = unsafe { SelectObject(mem, dib as HGDIOBJ) };
    Ok((dib, bits, previous))
}

/// `BitBlt`s `area` (virtual-screen coordinates) into a fresh top-down DIB.
fn capture_rect(area: WorkArea, rop: u32) -> Result<RawFrame, PlatformError> {
    if !area.is_usable() {
        return Err(PlatformError::new("capture area is empty"));
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(PlatformError::new(format!("GetDC(NULL) failed: {}", last_err())));
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(PlatformError::new(format!("CreateCompatibleDC failed: {}", last_err())));
        }
        let (dib, bits, previous) = match make_dib(mem, area.width, area.height) {
            Ok(v) => v,
            Err(e) => {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(e);
            }
        };
        let ok = BitBlt(mem, 0, 0, area.width, area.height, screen, area.left, area.top, rop);
        let error = if ok == 0 {
            Some(PlatformError::new(format!("BitBlt failed: {}", last_err())))
        } else {
            None
        };
        // Rows are tightly packed for 32-bpp BI_RGB, so the stride is exact.
        let stride = (area.width as u32).saturating_mul(4);
        let len = (area.width as usize) * (area.height as usize) * 4;
        let data = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        SelectObject(mem, previous);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if let Some(e) = error {
            // Never return a black or stale frame: an unreadable screen is an
            // error (contract §2.1).
            return Err(e);
        }
        Ok(RawFrame { width: area.width as u32, height: area.height as u32, stride, data })
    }
}

/// Hands `spec`'s solid colour to a layered window as a premultiplied bitmap.
fn set_layered_pixels(hwnd: HWND, spec: &MarkerSpec) -> Result<(), PlatformError> {
    let (size, rgb) = (spec.size, spec.rgb);
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(PlatformError::new(format!("GetDC(NULL) failed: {}", last_err())));
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(PlatformError::new(format!("CreateCompatibleDC failed: {}", last_err())));
        }
        let (dib, bits, previous) = match make_dib(mem, size, size) {
            Ok(v) => v,
            Err(e) => {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(e);
            }
        };
        // Opaque marker: alpha 255 means premultiplied == straight colour.
        for px in std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize)
            .chunks_exact_mut(4)
        {
            px[0] = rgb[2];
            px[1] = rgb[1];
            px[2] = rgb[0];
            px[3] = 255;
        }
        let destination = POINT { x: spec.x, y: spec.y };
        let extent = SIZE { cx: size, cy: size };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let ok = UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            &destination,
            &extent,
            mem,
            &source,
            0,
            &blend,
            ULW_ALPHA,
        );
        let error = if ok == 0 {
            Some(PlatformError::new(format!("UpdateLayeredWindow failed: {}", last_err())))
        } else {
            None
        };
        SelectObject(mem, previous);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        match error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Counting context for [`count_marker_windows`].
struct CountCtx {
    class: *const u16,
    class_len: usize,
    count: usize,
}

unsafe extern "system" fn count_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `*mut CountCtx` passed by `count_marker_windows`
    // in the same `EnumWindows` call.
    let ctx = unsafe { &mut *(lparam as *mut CountCtx) };
    let mut buffer = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    if len > 0 {
        let len = len as usize;
        if ctx.class_len == len + 1 {
            let wanted = unsafe { std::slice::from_raw_parts(ctx.class, len) };
            if wanted == &buffer[..len] {
                ctx.count += 1;
            }
        }
    }
    1
}

/// How many marker windows of our class are alive right now.
fn count_marker_windows(class: &[u16]) -> usize {
    let mut ctx = CountCtx { class: class.as_ptr(), class_len: class.len(), count: 0 };
    unsafe {
        EnumWindows(Some(count_enum_proc), &mut ctx as *mut CountCtx as LPARAM);
    }
    ctx.count
}

/// Whether the platform routes a mouse hit at `(x, y)` to a window of `class`.
///
/// `WindowFromPoint` is the function the input system uses to decide which
/// window a mouse message belongs to, so this is the automatable form of the
/// contract's "click on the marker and the event must reach the window below".
/// Measured on the live probe: layered markers are skipped, plain `WS_POPUP`
/// markers are not.
fn point_hits_class(class: &[u16], x: i32, y: i32) -> bool {
    unsafe {
        let hit = WindowFromPoint(POINT { x, y });
        if hit.is_null() {
            return false;
        }
        let mut buffer = [0u16; 128];
        let len = GetClassNameW(hit, buffer.as_mut_ptr(), buffer.len() as i32);
        if len <= 0 {
            return false;
        }
        let len = len as usize;
        class.len() == len + 1 && &buffer[..len] == &class[..len]
    }
}

impl Platform for Win32 {
    fn facts(&self) -> &ProbeFacts {
        &self.facts
    }

    fn capture_work_area(&self) -> Result<RawFrame, PlatformError> {
        // The work area, not the whole screen: markers are placed inside it, so
        // the frame is exactly the space they can appear in, and the taskbar
        // strip cannot hide one.
        capture_rect(self.facts.work_area, SRCCOPY | CAPTUREBLT)
    }

    fn create_marker(&self, spec: &MarkerSpec) -> Result<MarkerId, PlatformError> {
        if spec.size < 1 {
            return Err(PlatformError::new("marker edge must be at least one pixel"));
        }
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style(),
                self.class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                spec.x,
                spec.y,
                spec.size,
                spec.size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.hinst as HINSTANCE,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(PlatformError::new(format!("CreateWindowExW failed: {}", last_err())));
        }
        if let Err(e) = set_layered_pixels(hwnd, spec) {
            // Fail closed: a marker window without its pixels must not survive.
            unsafe { DestroyWindow(hwnd) };
            return Err(e);
        }
        Ok(MarkerId(hwnd as usize as u64))
    }

    fn present_marker(&self, id: MarkerId) -> Result<(), PlatformError> {
        let hwnd = id.0 as usize as HWND;
        unsafe {
            if IsWindow(hwnd) == 0 {
                return Err(PlatformError::new("marker window vanished before presentation"));
            }
            // SW_SHOWNA: show without activating. Its return value reports
            // whether the window *was* visible, so 0 is not an error here.
            ShowWindow(hwnd, SW_SHOWNA);
        }
        Ok(())
    }

    fn destroy_marker(&self, id: MarkerId) -> Result<(), PlatformError> {
        let hwnd = id.0 as usize as HWND;
        if unsafe { DestroyWindow(hwnd) } == 0 {
            return Err(PlatformError::new(format!("DestroyWindow failed: {}", last_err())));
        }
        Ok(())
    }

    fn sync_presentation(&self) -> Result<(), PlatformError> {
        let hr = unsafe { DwmFlush() };
        // With composition disabled there is no composition to wait for and no
        // presenting pass to miss; any other failure is real.
        if hr < 0 && hr != DWM_E_COMPOSITIONDISABLED {
            return Err(PlatformError::new(format!("DwmFlush failed: 0x{:08x}", hr as u32)));
        }
        Ok(())
    }

    fn live_marker_windows(&self) -> usize {
        count_marker_windows(&self.class)
    }

    fn point_intercepted_by_marker(&self, x: i32, y: i32) -> bool {
        point_hits_class(&self.class, x, y)
    }
}
