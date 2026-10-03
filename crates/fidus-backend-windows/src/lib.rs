// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! fidus backend for Windows.
//!
//! Basic Win32 primitives only ([`docs/backend-contract.md`]): projection uses
//! one **layered** window per marker (`WS_EX_LAYERED` + `UpdateLayeredWindow`,
//! placed at coordinates *we* chose), capture uses `BitBlt` on the screen DC.
//! Nothing here reads window geometry: `GetWindowRect`, `GetClientRect` and
//! `ClientToScreen` are forbidden to this backend even though they are
//! accurate on Windows.
//!
//! Because it creates one window per marker it provides **multi-marker
//! projection**, which is what the L0 Anchor calibrator needs. There is no
//! layer shell on Windows, so L9 Crosshair is honestly unavailable and the
//! gate selects L0 — by capability, not by platform identity.
//!
//! # Why these two primitives (measured, not guessed)
//!
//! The design was fixed by live measurement on Windows 11 (1920×1080, work area
//! 1920×1032, DPI 96, DWM on). Full data:
//! [`docs/measurements/windows-backend-primitives.md`].
//!
//! * **Layered windows** are the only projection that satisfies "the marker
//!   must not intercept input": a plain `WS_POPUP` window, even with
//!   `WS_EX_TRANSPARENT`, is still returned by `WindowFromPoint` (4/4 markers),
//!   while the layered one never is (0/4).
//! * **`BitBlt(GetDC(NULL), …, SRCCOPY | CAPTUREBLT)`** reads the composited
//!   desktop. Without a presentation sync the plain `SRCCOPY` variant saw a
//!   just-shown marker in only **8/20** trials — the Windows form of "the
//!   compositor has not presented yet", which the contract calls the easiest
//!   thing to get wrong. With [`api::Platform::sync_presentation`] every cell
//!   measured 20/20.
//! * **`PrintWindow(GetDesktopWindow)` was measured and rejected**: it returns
//!   0 and renders nothing (0 marker pixels in 20/20 trials, every pixel
//!   differing between frames).
//! * **End to end**, the real `AnchorCalibrator` through these primitives
//!   solves the identity affine with **0.000 px** rms/verification/consistency
//!   residual — placement and capture agree pixel for pixel.
//!
//! # Layout
//!
//! Only [`sys`] and [`backend`] are Windows-only; the rest is platform-neutral
//! on purpose, so that the pitfalls this backend must not repeat (missing
//! presentation sync, non-idempotent teardown, a black frame instead of an
//! error) have regression tests that run **without a display server**.

#![warn(missing_docs)]

pub mod api;
pub mod plan;
pub mod projector;

#[cfg(windows)]
pub mod backend;
#[cfg(windows)]
mod sys;

#[cfg(windows)]
pub use backend::{CalibrationSession, CaptureSession, WindowsBackend};
