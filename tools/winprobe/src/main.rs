// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0
//
//! Windows-backend **measurement harness** (proposal evidence, not production).
//!
//! It answers the questions the Windows backend proposal must answer with real
//! numbers before a line of the backend is written:
//!
//! 1. Does a window *we* place at exact screen coordinates land at exactly
//!    those pixels in a GDI capture of the work area, with no rounding error?
//! 2. Which capture primitive sees it: `BitBlt(GetDC(NULL))`, the same with
//!    `CAPTUREBLT`, or `PrintWindow(GetDesktopWindow)`? (A plain-WM_GDI window
//!    that never shows up in the capture is the failure this must rule out.)
//! 3. Is `DwmFlush` needed to make "shown" and "gone" true *before* the next
//!    capture, or does an immediate capture already see the change?
//! 4. Is the marker pixel-exact in colour, and does it stop intercepting the
//!    mouse (`WindowFromPoint`) without stealing focus?
//! 5. What do the **real** L0 Anchor calibrator and the **real** detector
//!    report end-to-end through these primitives (rms, verification,
//!    consistency, solved scale)?
//! 6. How long does one work-area capture take (steady-state estimator cost)?
//!
//! Everything here talks to the platform through GDI windows + capture only:
//! no `GetWindowRect` / `GetClientRect` / `ClientToScreen` anywhere, because
//! the backend contract forbids them even where they are accurate.

#![cfg(windows)]
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss)]

use std::ffi::c_void;
use std::time::Instant;

use fidus_calibrate::detect::{detect_colored_change, DetectConfig};
use fidus_calibrate::{AnchorCalibrator, AnchorConfig};
use fidus_core::coord::LogicalPoint;
use fidus_core::engine::Calibrator;
use fidus_core::io::{
    CalibrationIo, CaptureError, CaptureIo, Frame, MarkerError, MarkerStyle, PixelFormat,
};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const ERROR_CLASS_ALREADY_EXISTS: i32 = 1410;

// ---------------------------------------------------------------- utilities

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_err() -> String {
    std::io::Error::last_os_error().to_string()
}

fn last_err_code() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `COLORREF` is `0x00BBGGRR`.
fn colorref(rgb: [u8; 3]) -> u32 {
    rgb[0] as u32 | (rgb[1] as u32) << 8 | (rgb[2] as u32) << 16
}

fn rgb_of(rgba: [u8; 4]) -> [u8; 3] {
    [rgba[0], rgba[1], rgba[2]]
}

/// Teardown proof: is any top-level window of `class` still alive in this
/// session? The Windows analogue of `niri msg layers` / `xwininfo -root -tree`.
fn class_windows_alive(class: &[u16]) -> bool {
    !unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }.is_null()
}

/// `MAKELPARAM(x, y)` for `WM_NCHITTEST`.
fn make_lparam(x: i32, y: i32) -> isize {
    (((y & 0xffff) << 16) | (x & 0xffff)) as isize
}

/// Deterministic LCG so the run is reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo) as u64) as i32
    }
}

// ------------------------------------------------------------- environment

#[derive(Clone, Copy, Debug)]
struct WorkArea {
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

impl WorkArea {
    fn primary() -> Result<Self, String> {
        let mut rect = RECT::default();
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut rect as *mut RECT as *mut c_void, 0)
        };
        if ok == 0 {
            return Err(format!("SystemParametersInfoW(SPI_GETWORKAREA) failed: {}", last_err()));
        }
        Ok(WorkArea {
            left: rect.left,
            top: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        })
    }
}

fn setup_dpi_awareness() -> String {
    unsafe {
        let ok = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if ok != 0 {
            return "PerMonitorV2".into();
        }
        let err = last_err();
        if SetProcessDPIAware() != 0 {
            return format!("System (PerMonitorV2 refused: {err})");
        }
        format!("none (both refused; last: {})", last_err())
    }
}

// ---------------------------------------------------------------- projection

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Design {
    /// Plain `WS_POPUP` window painted with a solid brush in `WM_PAINT`.
    Gdi,
    /// `WS_EX_LAYERED` window fed through `UpdateLayeredWindow`.
    Layered,
}

impl Design {
    fn name(self) -> &'static str {
        match self {
            Design::Gdi => "gdi",
            Design::Layered => "layered",
        }
    }
}

unsafe extern "system" fn marker_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = lparam as *const CREATESTRUCTW;
            let color = (*cs).lpCreateParams as isize;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, color);
            1
        }
        // Painting is owned by us; never let the default handler erase.
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let color = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32;
            let brush = CreateSolidBrush(color);
            FillRect(hdc, &ps.rcPaint, brush);
            DeleteObject(brush as HGDIOBJ);
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The projector under test: one window per marker, exactly the primitives a
/// per-marker backend would use.
struct Projector {
    design: Design,
    flush: bool,
    hinst: HINSTANCE,
    class: Vec<u16>,
    markers: Vec<HWND>,
}

impl Projector {
    fn new(design: Design, flush: bool) -> Result<Self, String> {
        let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
        if hinst.is_null() {
            return Err(format!("GetModuleHandleW failed: {}", last_err()));
        }
        let class = wide("fidus_probe_marker");
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
            return Err(format!("RegisterClassExW failed: {}", last_err()));
        }
        Ok(Projector { design, flush, hinst, class, markers: Vec::new() })
    }

    fn ex_style(&self) -> u32 {
        let base = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
        match self.design {
            Design::Gdi => base,
            Design::Layered => base | WS_EX_LAYERED,
        }
    }

    /// Projects one solid marker with its top-left corner at screen `(x, y)`.
    fn create_marker(&self, x: i32, y: i32, size: i32, rgb: [u8; 3]) -> Result<HWND, String> {
        let color = colorref(rgb);
        let hwnd = unsafe {
            CreateWindowExW(
                self.ex_style(),
                self.class.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                x,
                y,
                size,
                size,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                self.hinst,
                color as usize as *const c_void,
            )
        };
        if hwnd.is_null() {
            return Err(format!("CreateWindowExW failed: {}", last_err()));
        }
        if self.design == Design::Layered {
            self.paint_layered(hwnd, x, y, size, rgb)?;
        }
        unsafe {
            // SW_SHOWNA: show without activating — the marker must never take
            // focus from whatever the user is doing.
            ShowWindow(hwnd, SW_SHOWNA);
            // Synchronous WM_PAINT dispatch: no message pump is running.
            UpdateWindow(hwnd);
        }
        Ok(hwnd)
    }

    /// Hands the layered window a premultiplied solid-colour bitmap.
    fn paint_layered(&self, hwnd: HWND, x: i32, y: i32, size: i32, rgb: [u8; 3]) -> Result<(), String> {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            if screen.is_null() {
                return Err(format!("GetDC(NULL) failed: {}", last_err()));
            }
            let mem = CreateCompatibleDC(screen);
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = size;
            bmi.bmiHeader.biHeight = -size; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(format!("CreateDIBSection failed: {}", last_err()));
            }
            let old = SelectObject(mem, dib as HGDIOBJ);
            // Opaque marker: alpha 255, so premultiplied == straight colour.
            for px in std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize)
                .chunks_exact_mut(4)
            {
                px[0] = rgb[2]; // B
                px[1] = rgb[1]; // G
                px[2] = rgb[0]; // R
                px[3] = 255; // A
            }
            let dst = POINT { x, y };
            let sz = SIZE { cx: size, cy: size };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let ok = UpdateLayeredWindow(
                hwnd,
                std::ptr::null_mut(),
                &dst,
                &sz,
                mem,
                &src,
                0,
                &blend,
                ULW_ALPHA,
            );
            let err = if ok == 0 { Some(last_err()) } else { None };
            SelectObject(mem, old);
            DeleteObject(dib as HGDIOBJ);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            match err {
                Some(e) => Err(format!("UpdateLayeredWindow failed: {e}")),
                None => Ok(()),
            }
        }
    }

    /// Projects `marks` = `[(screen_x, screen_y, size, rgb)]`, replacing any
    /// previously projected set.
    fn project(&mut self, marks: &[(i32, i32, i32, [u8; 3])]) -> Result<(), String> {
        self.clear()?;
        for (x, y, size, rgb) in marks {
            let hwnd = self.create_marker(*x, *y, *size, *rgb)?;
            self.markers.push(hwnd);
        }
        if self.flush {
            unsafe { DwmFlush() };
        }
        Ok(())
    }

    /// Destroys every marker and (optionally) waits for the removal to be
    /// composited.
    fn clear(&mut self) -> Result<(), String> {
        let had_markers = !self.markers.is_empty();
        for hwnd in self.markers.drain(..) {
            unsafe {
                if DestroyWindow(hwnd) == 0 {
                    return Err(format!("DestroyWindow failed: {}", last_err()));
                }
            }
        }
        if self.flush && had_markers {
            unsafe { DwmFlush() };
        }
        Ok(())
    }
}

impl Drop for Projector {
    fn drop(&mut self) {
        let _ = self.clear();
    }
}

// `CaptureIo: Send` is a hard requirement of the fidus session traits. Win32
// window handles are plain values, but `HWND` is a raw pointer, so the type is
// not `Send` automatically. This is a real finding for the proposal: a Windows
// backend must either assert this by hand (and then guarantee, by construction,
// that every window-owning call happens on the window's own thread) or own a
// dedicated projector thread. The probe does all window work on one thread.
unsafe impl Send for Projector {}

// ------------------------------------------------------------------ capture

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cap {
    /// `BitBlt(GetDC(NULL), …, SRCCOPY)`.
    SrcCopy,
    /// `BitBlt(GetDC(NULL), …, SRCCOPY | CAPTUREBLT)`.
    CaptureBlt,
    /// `PrintWindow(GetDesktopWindow(), …, PW_RENDERFULLCONTENT)`.
    PrintWindow,
}

impl Cap {
    const ALL: [Cap; 3] = [Cap::SrcCopy, Cap::CaptureBlt, Cap::PrintWindow];
    fn name(self) -> &'static str {
        match self {
            Cap::SrcCopy => "srccopy",
            Cap::CaptureBlt => "srccopy|captureblt",
            Cap::PrintWindow => "printwindow",
        }
    }
    fn index(self) -> usize {
        match self {
            Cap::SrcCopy => 0,
            Cap::CaptureBlt => 1,
            Cap::PrintWindow => 2,
        }
    }
}

struct Captured {
    width: i32,
    height: i32,
    data: Vec<u8>,
    millis: f64,
    /// Non-zero when the underlying call reported success.
    ok: i32,
}

fn capture(rect: WorkArea, cap: Cap) -> Result<Captured, String> {
    match cap {
        Cap::SrcCopy => capture_bitblt(rect, SRCCOPY),
        Cap::CaptureBlt => capture_bitblt(rect, SRCCOPY | CAPTUREBLT),
        Cap::PrintWindow => capture_printwindow(rect),
    }
}

/// Allocates a 32-bpp top-down DIB selected into a memory DC.
unsafe fn make_dib(mem: HDC, w: i32, h: i32) -> Result<(HBITMAP, *mut c_void, HGDIOBJ), String> {
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    bmi.bmiHeader.biHeight = -h; // negative => top-down rows
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if dib.is_null() || bits.is_null() {
        return Err(format!("CreateDIBSection failed: {}", last_err()));
    }
    let old = SelectObject(mem, dib as HGDIOBJ);
    Ok((dib, bits, old))
}

fn capture_bitblt(rect: WorkArea, rop: u32) -> Result<Captured, String> {
    let started = Instant::now();
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(format!("GetDC(NULL) failed: {}", last_err()));
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(format!("CreateCompatibleDC failed: {}", last_err()));
        }
        let (dib, bits, old) = match make_dib(mem, rect.width, rect.height) {
            Ok(v) => v,
            Err(e) => {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(e);
            }
        };
        let ok = BitBlt(mem, 0, 0, rect.width, rect.height, screen, rect.left, rect.top, rop);
        let err = if ok == 0 { Some(last_err()) } else { None };
        let len = (rect.width as usize) * (rect.height as usize) * 4;
        let data = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
        SelectObject(mem, old);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        if let Some(e) = err {
            return Err(format!("BitBlt failed: {e}"));
        }
        Ok(Captured { width: rect.width, height: rect.height, data, millis: started.elapsed().as_secs_f64() * 1000.0, ok })
    }
}

/// Renders the whole desktop window into a DIB and crops `rect` out of it.
fn capture_printwindow(rect: WorkArea) -> Result<Captured, String> {
    let started = Instant::now();
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(format!("GetDC(NULL) failed: {}", last_err()));
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(format!("CreateCompatibleDC failed: {}", last_err()));
        }
        let (dib, bits, old) = match make_dib(mem, vw, vh) {
            Ok(v) => v,
            Err(e) => {
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return Err(e);
            }
        };
        let ok = PrintWindow(GetDesktopWindow(), mem, PW_RENDERFULLCONTENT);
        // PrintWindow is documented to return 0 in some cases where it still
        // rendered; keep the flag and read the pixels anyway.
        let full = std::slice::from_raw_parts(bits as *const u8, (vw as usize) * (vh as usize) * 4);
        let row = (rect.width as usize) * 4;
        let mut data = vec![0u8; row * rect.height as usize];
        for y in 0..rect.height {
            let sy = rect.top - vy + y;
            let sx = rect.left - vx;
            if sy < 0 || sy >= vh || sx < 0 || sx + rect.width > vw {
                continue;
            }
            let src = (sy as usize * vw as usize + sx as usize) * 4;
            let dst = y as usize * row;
            data[dst..dst + row].copy_from_slice(&full[src..src + row]);
        }
        SelectObject(mem, old);
        DeleteObject(dib as HGDIOBJ);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        Ok(Captured { width: rect.width, height: rect.height, data, millis: started.elapsed().as_secs_f64() * 1000.0, ok })
    }
}

fn to_frame(c: &Captured) -> Frame {
    Frame {
        width: c.width as u32,
        height: c.height as u32,
        stride: (c.width as u32) * 4,
        // DIB 32bpp BI_RGB is B,G,R,X little-endian, which is exactly what
        // `Xrgb8888` reads as [R, G, B, 255].
        format: PixelFormat::Xrgb8888,
        data: c.data.clone(),
    }
}

/// Pixels in `c` whose RGB equals `rgb` exactly, plus their bounding box.
fn count_color(c: &Captured, rgb: [u8; 3]) -> (u64, Option<(i32, i32, i32, i32)>) {
    let mut n = 0u64;
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (i, px) in c.data.chunks_exact(4).enumerate() {
        if px[0] == rgb[2] && px[1] == rgb[1] && px[2] == rgb[0] {
            n += 1;
            let x = (i as i32) % c.width;
            let y = (i as i32) / c.width;
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    (n, if n > 0 { Some((x0, y0, x1, y1)) } else { None })
}

/// Pixels whose RGB differs by more than `threshold` between two captures.
fn count_changed(a: &Captured, b: &Captured, threshold: i32) -> u64 {
    if a.data.len() != b.data.len() {
        return 0;
    }
    a.data
        .chunks_exact(4)
        .zip(b.data.chunks_exact(4))
        .filter(|(p, q)| {
            (0..3).any(|c| (i32::from(p[c]) - i32::from(q[c])).abs() > threshold)
        })
        .count() as u64
}

// --------------------------------------------------------------- statistics

#[derive(Default, Clone)]
struct Stats {
    trials: u32,
    found: u32,
    exact_color: u32,
    exact_center: u32,
    /// Trials where the marker *was* present in this primitive's post frame.
    clear_checked: u32,
    /// ...and was gone from the next capture.
    clear_clean: u32,
    residual_max: f64,
    residual_sum: f64,
    area_ok: u32,
    marker_px: u64,
    changed_px: u64,
    errors: Vec<String>,
}

impl Stats {
    fn report(&self, label: &str) {
        let pct = |n: u32| 100.0 * f64::from(n) / f64::from(self.trials.max(1));
        println!(
            "{label}: trials={} found={} ({:.0}%) centroid_exact={} ({:.0}%) color_exact={} ({:.0}%) area_ok={} clear_clean={}/{} residual_max={:.3}px residual_mean={:.3}px marker_px={} changed_px={}",
            self.trials,
            self.found,
            pct(self.found),
            self.exact_center,
            pct(self.exact_center),
            self.exact_color,
            pct(self.exact_color),
            self.area_ok,
            self.clear_clean,
            self.clear_checked,
            self.residual_max,
            if self.found > 0 { self.residual_sum / f64::from(self.found) } else { 0.0 },
            self.marker_px,
            self.changed_px,
        );
        for e in self.errors.iter().take(2) {
            println!("{label}: error {e}");
        }
    }
}

// ------------------------------------------------------------------- trials

/// One full "baseline → show → capture (3 primitives) → clear → capture" cycle
/// at a random position, measured with the production detector.
fn run_trials(
    projector: &mut Projector,
    wa: WorkArea,
    trials: u32,
    sizes: &[i32],
    rng: &mut Lcg,
    capture_ms: &mut Vec<f64>,
) -> [Stats; 3] {
    let mut stats: [Stats; 3] = Default::default();
    let detect = DetectConfig::default();
    let color: [u8; 4] = [255, 0, 255, 255];
    let rgb = rgb_of(color);
    let margin = 40;

    for st in stats.iter_mut() {
        st.trials = trials;
    }

    for t in 0..trials {
        let size = sizes[(t as usize) % sizes.len()];
        let x = wa.left + rng.range(margin, (wa.width - size - margin).max(margin + 1));
        let y = wa.top + rng.range(margin, (wa.height - size - margin).max(margin + 1));

        let baseline = match capture(wa, Cap::CaptureBlt) {
            Ok(c) => c,
            Err(e) => {
                for st in stats.iter_mut() {
                    st.errors.push(format!("baseline capture: {e}"));
                }
                continue;
            }
        };
        capture_ms.push(baseline.millis);

        if let Err(e) = projector.project(&[(x, y, size, rgb)]) {
            for st in stats.iter_mut() {
                st.errors.push(format!("project: {e}"));
            }
            continue;
        }
        let mut posts: Vec<(Cap, Captured)> = Vec::new();
        for cap in Cap::ALL {
            match capture(wa, cap) {
                Ok(c) => {
                    capture_ms.push(c.millis);
                    posts.push((cap, c));
                }
                Err(e) => stats[cap.index()].errors.push(format!("post capture: {e}")),
            }
        }
        if let Err(e) = projector.clear() {
            for st in stats.iter_mut() {
                st.errors.push(format!("clear: {e}"));
            }
        }
        let after = match capture(wa, Cap::CaptureBlt) {
            Ok(c) => c,
            Err(e) => {
                for st in stats.iter_mut() {
                    st.errors.push(format!("after capture: {e}"));
                }
                continue;
            }
        };
        capture_ms.push(after.millis);

        let bf = to_frame(&baseline);
        let af = to_frame(&after);

        // Removal must be visible in the very next capture. Only meaningful
        // for a primitive that actually saw the marker — a primitive that saw
        // nothing would report a trivially "clean" removal.
        let still_there =
            detect_colored_change(&bf, &af, color, 8, Some(f64::from(size * size)), &detect).is_ok();

        let exp_x = f64::from(x - wa.left) + f64::from(size) / 2.0;
        let exp_y = f64::from(y - wa.top) + f64::from(size) / 2.0;

        for (cap, post) in &posts {
            let st = &mut stats[cap.index()];
            let pf = to_frame(post);
            let (marker_px, bbox) = count_color(post, rgb);
            st.marker_px += marker_px;
            st.changed_px += count_changed(&baseline, post, detect.diff_threshold as i32);
            if let Some(b) = bbox {
                st.errors.push(format!(
                    "trial{t} {} bbox=({},{})-({},{}) expected_topleft=({},{})",
                    cap.name(),
                    b.0,
                    b.1,
                    b.2,
                    b.3,
                    x - wa.left,
                    y - wa.top
                ));
            }

            match detect_colored_change(&bf, &pf, color, 8, Some(f64::from(size * size)), &detect) {
                Ok(d) => {
                    st.found += 1;
                    st.clear_checked += 1;
                    if !still_there {
                        st.clear_clean += 1;
                    }
                    let center = d.bbox.center();
                    let (dx, dy) = (center.x - exp_x, center.y - exp_y);
                    let residual = (dx * dx + dy * dy).sqrt();
                    st.residual_max = st.residual_max.max(residual);
                    st.residual_sum += residual;
                    if dx == 0.0 && dy == 0.0 {
                        st.exact_center += 1;
                    }
                    if d.area == (size * size) as u32 {
                        st.area_ok += 1;
                    }
                    let mut exact = true;
                    for yy in d.bbox.y0.max(0) as u32..d.bbox.y1.max(0) as u32 {
                        for xx in d.bbox.x0.max(0) as u32..d.bbox.x1.max(0) as u32 {
                            let px = pf.rgba_at(xx, yy);
                            if px[0] != color[0] || px[1] != color[1] || px[2] != color[2] {
                                exact = false;
                            }
                        }
                    }
                    if exact {
                        st.exact_color += 1;
                    }
                }
                Err(e) => {
                    if st.errors.len() < 40 {
                        st.errors.push(format!("trial{t} detect: {e}"));
                    }
                }
            }
        }
    }
    stats
}

// ------------------------------------------------- real calibrator end-to-end

struct ProbeIo<'a> {
    projector: &'a mut Projector,
    wa: WorkArea,
    cap: Cap,
    captures: u32,
    capture_ms: Vec<f64>,
}

impl ProbeIo<'_> {
    fn grab(&mut self) -> Result<Frame, CaptureError> {
        match capture(self.wa, self.cap) {
            Ok(c) => {
                self.captures += 1;
                self.capture_ms.push(c.millis);
                Ok(to_frame(&c))
            }
            Err(e) => Err(CaptureError::Backend(e)),
        }
    }
}

impl CaptureIo for ProbeIo<'_> {
    fn capture(&mut self) -> Result<Frame, CaptureError> {
        self.grab()
    }
}

impl CalibrationIo for ProbeIo<'_> {
    fn usable_size_hint(&mut self) -> Result<(f64, f64), MarkerError> {
        Ok((f64::from(self.wa.width), f64::from(self.wa.height)))
    }

    fn show_marker(&mut self, pos: LogicalPoint, style: MarkerStyle) -> Result<(), MarkerError> {
        self.show_markers(&[(pos, style)])
    }

    fn show_markers(&mut self, marks: &[(LogicalPoint, MarkerStyle)]) -> Result<(), MarkerError> {
        let projected: Vec<(i32, i32, i32, [u8; 3])> = marks
            .iter()
            .map(|(pos, style)| {
                (
                    self.wa.left + pos.x.round() as i32,
                    self.wa.top + pos.y.round() as i32,
                    style.size_logical.round() as i32,
                    rgb_of(style.rgba),
                )
            })
            .collect();
        self.projector.project(&projected).map_err(MarkerError::Backend)
    }

    fn clear_marker(&mut self) -> Result<(), MarkerError> {
        self.projector.clear().map_err(MarkerError::Backend)
    }

    fn destroy_projector(&mut self) -> Result<(), MarkerError> {
        self.projector.clear().map_err(MarkerError::Backend)
    }
}

// --------------------------------------------------------------------- main

fn main() {
    let awareness = setup_dpi_awareness();
    let wa = match WorkArea::primary() {
        Ok(wa) => wa,
        Err(e) => {
            println!("FATAL work_area: {e}");
            std::process::exit(1);
        }
    };
    let (vw, vh, vx, vy) = unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    let composition = unsafe {
        let mut enabled: i32 = 0;
        let hr = DwmIsCompositionEnabled(&mut enabled);
        format!("hr=0x{hr:08x} enabled={enabled}")
    };

    println!("== environment ==");
    println!("dpi_awareness = {awareness}");
    println!("dpi_for_system = {}", unsafe { GetDpiForSystem() });
    println!(
        "screen = {}x{}  monitors = {}",
        unsafe { GetSystemMetrics(SM_CXSCREEN) },
        unsafe { GetSystemMetrics(SM_CYSCREEN) },
        unsafe { GetSystemMetrics(SM_CMONITORS) }
    );
    println!("virtual_screen = {vw}x{vh} at ({vx}, {vy})");
    println!("work_area = {}x{} at ({}, {})", wa.width, wa.height, wa.left, wa.top);
    println!("dwm_composition = {composition}");

    let mut capture_ms: Vec<f64> = Vec::new();

    // (0) Does the desktop change at all between two captures? A frozen
    // "capture" would make every later measurement meaningless.
    let mut working_cap = Cap::CaptureBlt;
    {
        let a = capture(wa, Cap::CaptureBlt);
        std::thread::sleep(std::time::Duration::from_millis(250));
        let b = capture(wa, Cap::CaptureBlt);
        match (a, b) {
            (Ok(a), Ok(b)) => {
                let (n, _) = count_color(&a, [255, 0, 255]);
                println!(
                    "capture_sanity = {}x{} {}ms nonblack={:.2}% distinct={} changed_over_250ms={} magenta_px={}",
                    a.width,
                    a.height,
                    a.millis,
                    100.0
                        * a.data
                            .chunks_exact(4)
                            .filter(|p| p[0] | p[1] | p[2] != 0)
                            .count() as f64
                        / (a.width as f64 * a.height as f64),
                    {
                        let mut s = std::collections::HashSet::new();
                        for p in a.data.chunks_exact(4).step_by(97) {
                            s.insert([p[0], p[1], p[2]]);
                        }
                        s.len()
                    },
                    count_changed(&a, &b, 12),
                    n
                );
                capture_ms.push(a.millis);
                capture_ms.push(b.millis);
            }
            _ => println!("capture_sanity = FAILED"),
        }
    }

    // (1) Is our own window painted, visible, and seen by each capture
    // primitive? This is the decisive table.
    println!();
    println!("== visibility probe (one 32px marker at a fixed spot) ==");
    let magenta = [255u8, 0, 255];
    for design in [Design::Gdi, Design::Layered] {
        let mut projector = match Projector::new(design, true) {
            Ok(p) => p,
            Err(e) => {
                println!("design={} -> init failed: {e}", design.name());
                continue;
            }
        };
        let (x, y, size) = (wa.left + 200, wa.top + 200, 32);
        match projector.project(&[(x, y, size, magenta)]) {
            Ok(()) => {
                let hwnd = projector.markers[0];
                let visible = unsafe { IsWindowVisible(hwnd) };
                // Read our own window's surface back: proves WM_PAINT (or
                // UpdateLayeredWindow) really wrote the colour.
                let self_px = unsafe {
                    let dc = GetDC(hwnd);
                    let px = if dc.is_null() { u32::MAX } else { GetPixel(dc, 16, 16) };
                    if !dc.is_null() {
                        ReleaseDC(hwnd, dc);
                    }
                    px
                };
                print!(
                    "design={} IsWindowVisible={} self_dc_pixel=0x{self_px:06x}",
                    design.name(),
                    visible
                );
                for cap in Cap::ALL {
                    match capture(wa, cap) {
                        Ok(c) => {
                            let (n, bbox) = count_color(&c, magenta);
                            print!(
                                " | {}=px{} {} ok={}",
                                cap.name(),
                                n,
                                match bbox {
                                    Some(b) => format!("bbox=({},{})-({},{}) expected=({},{})", b.0, b.1, b.2, b.3, x - wa.left, y - wa.top),
                                    None => "bbox=none".to_string(),
                                },
                                c.ok
                            );
                            capture_ms.push(c.millis);
                        }
                        Err(e) => print!(" | {}=ERR {e}", cap.name()),
                    }
                }
                println!();
                let _ = projector.clear();
            }
            Err(e) => println!("design={} -> project failed: {e}", design.name()),
        }
    }

    // (2) Which primitive works? Full matrix, both raster ops A/B'd per trial.
    println!();
    println!("== design matrix (single marker, baseline->show->capture->clear) ==");
    let mut rng = Lcg(0x5EED_1234_ABCD_0001);
    let sizes = [8, 28];
    for design in [Design::Gdi, Design::Layered] {
        for flush in [false, true] {
            let mut projector = match Projector::new(design, flush) {
                Ok(p) => p,
                Err(e) => {
                    println!("design={} dwmflush={flush} -> init failed: {e}", design.name());
                    continue;
                }
            };
            let stats = run_trials(&mut projector, wa, 20, &sizes, &mut rng, &mut capture_ms);
            for (i, st) in stats.iter().enumerate() {
                let cap = Cap::ALL[i];
                if st.found > 0 || st.marker_px > 0 || !st.errors.is_empty() {
                    st.report(&format!(
                        "design={} dwmflush={} cap={}",
                        design.name(),
                        flush,
                        cap.name()
                    ));
                }
                if st.found > 0 && cap != Cap::PrintWindow {
                    working_cap = cap;
                }
            }
            let class = projector.class.clone();
            drop(projector);
            println!(
                "design={} dwmflush={} residual_windows_after_drop={}",
                design.name(),
                flush,
                class_windows_alive(&class)
            );
        }
    }

    // (3) Click-through and focus: the marker must never intercept input.
    println!();
    println!("== click-through / focus (four simultaneous sentinels, topmost) ==");
    let colors = fidus_calibrate::SENTINEL_COLORS;
    for design in [Design::Gdi, Design::Layered] {
        let mut projector = match Projector::new(design, true) {
            Ok(p) => p,
            Err(e) => {
                println!("design={} -> init failed: {e}", design.name());
                continue;
            }
        };
        let before = unsafe { GetForegroundWindow() };
        let (size, x, y) = (32, wa.left + 120, wa.top + 120);
        if let Err(e) = projector.project(&[(x, y, size, rgb_of(colors[0]))]) {
            println!("design={} -> project failed: {e}", design.name());
            continue;
        }
        let hit = unsafe { WindowFromPoint(POINT { x: x + size / 2, y: y + size / 2 }) };
        let intercepted = projector.markers.iter().any(|h| std::ptr::eq(*h, hit));
        // The documented click-through mechanism: DefWindowProc answers
        // HTTRANSPARENT for a WS_EX_TRANSPARENT window, which is what makes the
        // mouse message fall through to the window below.
        let nchit = unsafe {
            SendMessageW(
                projector.markers[0],
                WM_NCHITTEST,
                0,
                make_lparam(x + size / 2, y + size / 2),
            )
        };
        println!(
            "design={} single_marker_intercepted={} nchittest={} ht_transparent={} foreground_stable={}",
            design.name(),
            intercepted,
            nchit,
            nchit == HTTRANSPARENT as isize,
            before == unsafe { GetForegroundWindow() }
        );
        let four = [
            (wa.left + 40, wa.top + 40, 8, rgb_of(colors[0])),
            (wa.left + wa.width - 48, wa.top + 40, 8, rgb_of(colors[1])),
            (wa.left + 40, wa.top + wa.height - 48, 8, rgb_of(colors[2])),
            (wa.left + wa.width - 48, wa.top + wa.height - 48, 8, rgb_of(colors[3])),
        ];
        if let Err(e) = projector.project(&four) {
            println!("design={} four_marker project failed: {e}", design.name());
        } else {
            let mut intercepted = 0;
            let mut transparent = 0;
            for (fx, fy, fs, _) in four {
                let hit = unsafe { WindowFromPoint(POINT { x: fx + fs / 2, y: fy + fs / 2 }) };
                if projector.markers.iter().any(|h| std::ptr::eq(*h, hit)) {
                    intercepted += 1;
                }
                let nchit = unsafe {
                    SendMessageW(
                        projector.markers[0],
                        WM_NCHITTEST,
                        0,
                        make_lparam(fx + fs / 2, fy + fs / 2),
                    )
                };
                if nchit == HTTRANSPARENT as isize {
                    transparent += 1;
                }
            }
            println!(
                "design={} four_marker_intercepted={}/4 ht_transparent={}/4 foreground_stable={}",
                design.name(),
                intercepted,
                transparent,
                before == unsafe { GetForegroundWindow() }
            );
        }
        let _ = projector.clear();
        println!(
            "design={} residual_windows_after_clear={}",
            design.name(),
            class_windows_alive(&projector.class)
        );
    }

    // (4) The real L0 Anchor calibrator, through the real fidus-core traits.
    println!();
    println!("== L0 Anchor end-to-end through real fidus-core traits ==");
    for design in [Design::Gdi, Design::Layered] {
        let run_started = Instant::now();
        let mut projector = match Projector::new(design, true) {
            Ok(p) => p,
            Err(e) => {
                println!("design={} anchor: init failed: {e}", design.name());
                continue;
            }
        };
        let mut io = ProbeIo { projector: &mut projector, wa, cap: working_cap, captures: 0, capture_ms: Vec::new() };
        let mut calibrator = AnchorCalibrator::new(AnchorConfig {
            seed: Some(0x00A0_5EED),
            ..AnchorConfig::default()
        });
        match calibrator.calibrate(&mut io) {
            Ok(frame) => {
                let q = frame.quality();
                println!("design={} anchor_result = OK (cap={})", design.name(), working_cap.name());
                println!("design={} anchor_map_scale = {:.6}", design.name(), frame.map().linear_scale());
                println!(
                    "design={} anchor_map_coefficients = {:?}",
                    design.name(),
                    frame.map().coefficients()
                );
                println!("design={} anchor_rms_residual_px = {:.3}", design.name(), q.rms_residual_px);
                println!("design={} anchor_max_residual_px = {:.3}", design.name(), q.max_residual_px);
                println!(
                    "design={} anchor_verification_max_err_px = {:.3}",
                    design.name(),
                    q.verification_max_err_px
                );
                println!(
                    "design={} anchor_consistency_max_err_px = {:.3}",
                    design.name(),
                    q.consistency_max_err_px
                );
                println!("design={} anchor_sample_count = {}", design.name(), q.sample_count);
                println!("design={} anchor_passes = {}", design.name(), q.independent_passes);
                println!("design={} anchor_capture_size = {:?}", design.name(), frame.capture_size());
            }
            Err(e) => println!("design={} anchor_result = FAILED {e}", design.name()),
        }
        println!("design={} anchor_captures = {}", design.name(), io.captures);
        let mut ms = std::mem::take(&mut io.capture_ms);
        ms.sort_by(f64::total_cmp);
        if !ms.is_empty() {
            println!(
                "design={} anchor_capture_ms = min {:.2} median {:.2} max {:.2}",
                design.name(),
                ms[0],
                ms[ms.len() / 2],
                ms[ms.len() - 1]
            );
        }
        println!(
            "design={} anchor_wall_ms = {:.0}",
            design.name(),
            run_started.elapsed().as_secs_f64() * 1000.0
        );
        drop(io);
        println!(
            "design={} anchor_residual_windows_after_teardown={}",
            design.name(),
            class_windows_alive(&projector.class)
        );
    }

    capture_ms.sort_by(f64::total_cmp);
    if !capture_ms.is_empty() {
        println!();
        println!(
            "capture_timing_all = n={} min {:.2}ms median {:.2}ms max {:.2}ms",
            capture_ms.len(),
            capture_ms[0],
            capture_ms[capture_ms.len() / 2],
            capture_ms[capture_ms.len() - 1]
        );
    }
}
