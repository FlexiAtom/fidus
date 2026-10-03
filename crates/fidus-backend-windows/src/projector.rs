// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! Marker projection driven by a **dedicated owner thread**.
//!
//! # Why an owner thread instead of "just create the windows"
//!
//! Win32 windows are thread-affine: `DestroyWindow` must run on the thread that
//! created the window. fidus sessions are `Send`, and `Drop` may run on any
//! thread, so a backend that creates windows on whichever thread happens to call
//! `show_marker` can only be honest in one of two ways:
//!
//! * assert `unsafe impl Send` and refuse foreign-thread calls — which leaves
//!   window cleanup impossible if `Drop` lands elsewhere, i.e. residual windows,
//!   the one thing principle five forbids;
//! * or own a thread and route every window call through it (what this module
//!   does).
//!
//! With one owner, "destroy on the creating thread" holds by construction, the
//! projector is driven from any thread, and `Drop` can join the thread instead
//! of hoping. Measured on the live probe: zero residual marker windows after
//! both an explicit teardown and a `Drop`.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::api::{MarkerId, MarkerSpec, Platform, PlatformError};

/// Reply channel of one command.
type Reply = Sender<Result<(), PlatformError>>;

/// Commands the owner thread understands.
enum Cmd {
    Show(Vec<MarkerSpec>, Reply),
    Clear(Reply),
    Shutdown,
}

/// Owns every marker window and serialises all window calls onto one thread.
#[derive(Debug)]
pub struct Projector {
    tx: Sender<Cmd>,
    join: Option<JoinHandle<()>>,
}

impl Projector {
    /// Starts the owner thread.
    pub fn start(platform: Arc<dyn Platform>) -> Result<Self, PlatformError> {
        let (tx, rx) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("fidus-marker-projector".into())
            .spawn(move || run(&rx, &*platform))
            .map_err(|e| PlatformError::new(format!("could not start the projector thread: {e}")))?;
        Ok(Self { tx, join: Some(join) })
    }

    /// Replaces the projected set with `marks`, returning once they are on
    /// screen.
    ///
    /// An empty slice simply removes whatever was projected, which is what
    /// `show_markers(&[])` must mean.
    pub fn show(&self, marks: Vec<MarkerSpec>) -> Result<(), PlatformError> {
        self.request(|reply| Cmd::Show(marks, reply))
    }

    /// Removes every projected marker, returning once the removal is visible.
    ///
    /// Idempotent: with nothing projected it performs no platform call at all.
    pub fn clear(&self) -> Result<(), PlatformError> {
        self.request(Cmd::Clear)
    }

    fn request(&self, make: impl FnOnce(Reply) -> Cmd) -> Result<(), PlatformError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send(make(reply_tx))
            .map_err(|_| PlatformError::new("the projector thread is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| PlatformError::new("the projector thread dropped its reply"))?
    }
}

impl Drop for Projector {
    fn drop(&mut self) {
        // Teardown on the owning thread, and wait for it: when this returns, no
        // marker window is left (spec §4.4 / principle five).
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The owner thread: every window operation happens here, in command order.
fn run(rx: &Receiver<Cmd>, platform: &dyn Platform) {
    let mut live: Vec<MarkerId> = Vec::new();
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Show(marks, reply) => {
                let _ = reply.send(set_markers(platform, &mut live, &marks));
            }
            Cmd::Clear(reply) => {
                let _ = reply.send(clear_markers(platform, &mut live));
            }
            Cmd::Shutdown => {
                let _ = clear_markers(platform, &mut live);
                break;
            }
        }
    }
}

/// Destroys everything currently projected, then waits for the removal to be
/// presented. No-op (and no platform call) when nothing is projected.
fn clear_markers(platform: &dyn Platform, live: &mut Vec<MarkerId>) -> Result<(), PlatformError> {
    let (destroyed, first_error) = destroy_all(platform, live);
    if destroyed {
        // The caller's next capture is a baseline: it must not contain what we
        // just removed.
        let synced = platform.sync_presentation();
        if first_error.is_none() {
            return synced;
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Destroys every handle in `live`, **keeping the ones whose destruction
/// failed** so that a later `clear` (or `Drop`) retries them instead of losing
/// track of a window that may still be on screen.
///
/// Returns whether anything was asked to die, plus the first error.
fn destroy_all(platform: &dyn Platform, live: &mut Vec<MarkerId>) -> (bool, Option<PlatformError>) {
    let asked = !live.is_empty();
    let mut first_error = None;
    let mut survivors = Vec::new();
    for id in live.drain(..) {
        if let Err(e) = platform.destroy_marker(id) {
            survivors.push(id);
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    live.extend(survivors);
    (asked, first_error)
}

/// Replaces the projected set. Fails closed: if any part of the projection
/// fails, nothing is left on screen and the error is reported.
fn set_markers(
    platform: &dyn Platform,
    live: &mut Vec<MarkerId>,
    marks: &[MarkerSpec],
) -> Result<(), PlatformError> {
    let had = !live.is_empty();
    // The old set dies without a sync of its own: one presentation sync at the
    // end covers both the removal and the new markers, and nothing observes the
    // intermediate state.
    let (_, removing_error) = destroy_all(platform, live);
    if let Some(e) = removing_error {
        return Err(e);
    }
    for spec in marks {
        match platform.create_marker(spec) {
            Ok(id) => {
                live.push(id);
                if let Err(e) = platform.present_marker(id) {
                    let _ = clear_markers(platform, live);
                    return Err(e);
                }
            }
            Err(e) => {
                let _ = clear_markers(platform, live);
                return Err(e);
            }
        }
    }
    if had || !marks.is_empty() {
        // "Shown" is only true once the platform has presented it (contract
        // §2.2). Without this call a plain capture measured 8/20.
        platform.sync_presentation()?;
    }
    Ok(())
}
