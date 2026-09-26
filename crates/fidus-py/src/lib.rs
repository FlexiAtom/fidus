// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

//! `fidus-py` — Stage 1 Python binding for the fidus zero-trust positioning
//! engine (plan `meapet-embed-contract`, frozen surface P2).
//!
//! Exposes exactly one engine object plus a read-only calibration summary;
//! the binding never accepts coordinates (spec §6 zero-trust holds at the
//! Python boundary too: `estimate` only has an output direction).

use fidus::prelude::*;
use fidus::wayland::WaylandLayerBackend;
use numpy::{PyArrayDyn, PyArrayMethods, PyUntypedArrayMethods};
use pyo3::exceptions::{PyException, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyTuple;

pyo3::create_exception!(fidus, FidusError, PyException);
pyo3::create_exception!(fidus, FidusInitError, FidusError);
pyo3::create_exception!(fidus, FidusCalibrationError, FidusError);
pyo3::create_exception!(fidus, FidusEstimateError, FidusError);
pyo3::create_exception!(fidus, FidusNotCalibrated, FidusEstimateError);
pyo3::create_exception!(fidus, FidusNoTarget, FidusEstimateError);
pyo3::create_exception!(fidus, FidusTargetLost, FidusEstimateError);
pyo3::create_exception!(fidus, FidusUntrackable, FidusEstimateError);

fn init_error(e: InitError) -> PyErr {
    FidusInitError::new_err(e.to_string())
}

fn calibration_error(e: CalibrationError) -> PyErr {
    FidusCalibrationError::new_err(e.to_string())
}

/// Maps every `EstimateError` variant to its frozen exception class
/// (plan P2.4); the message is the Rust `Display` string verbatim — never
/// reworded here, so the binding cannot drift from the engine's honesty.
fn estimate_error(e: EstimateError) -> PyErr {
    let msg = e.to_string();
    match &e {
        EstimateError::NotCalibrated => FidusNotCalibrated::new_err(msg),
        EstimateError::NoTarget => FidusNoTarget::new_err(msg),
        EstimateError::TargetLost => FidusTargetLost::new_err(msg),
        EstimateError::UntrackableTarget { .. } => FidusUntrackable::new_err(msg),
        _ => FidusEstimateError::new_err(msg),
    }
}

/// Read-only summary of one calibration (plan P2.2). Pure data, no engine
/// borrow: the overlay is already destroyed when this crosses to Python.
#[pyclass(get_all, module = "fidus")]
#[derive(Clone)]
struct CalibInfo {
    scale: f64,
    rms_residual_px: f64,
    max_residual_px: f64,
    verification_max_err_px: f64,
    consistency_max_err_px: f64,
    sample_count: usize,
    independent_passes: usize,
}

#[pymethods]
impl CalibInfo {
    fn __repr__(&self) -> String {
        format!(
            "CalibInfo(scale={:.3}, rms_residual_px={:.3}, max_residual_px={:.3}, \
             verification_max_err_px={:.3}, consistency_max_err_px={:.3}, \
             sample_count={}, independent_passes={})",
            self.scale,
            self.rms_residual_px,
            self.max_residual_px,
            self.verification_max_err_px,
            self.consistency_max_err_px,
            self.sample_count,
            self.independent_passes,
        )
    }
}

/// Frozen plain-data mirror of [`CalibInfo`], safe to carry across
/// `Python::detach` (all fields `Send + Copy`).
struct CalibData {
    scale: f64,
    rms_residual_px: f64,
    max_residual_px: f64,
    verification_max_err_px: f64,
    consistency_max_err_px: f64,
    sample_count: usize,
    independent_passes: usize,
}

/// Window-positioning engine for one caller's own render.
///
/// # Thread affinity: create it where you use it
///
/// `unsendable` on this class is a **contract**, not a conservative default:
/// the engine holds a live capture session against the compositor, so a given
/// instance must be constructed **and** used on one OS thread for its lifetime.
/// The wheel deliberately ships no internal worker thread, progress reporting
/// or cancellation — a host that wants estimates off its UI thread runs its own
/// worker and creates the `Fidus` object *inside* it.
///
/// *Failure mode when this is violated*: touching an instance from another
/// thread trips pyo3's thread guard, which surfaces as `PanicException` — a
/// class deriving from `BaseException`, **not** from `Exception` (pyo3 0.27.2,
/// `src/panic.rs`; the same base `SystemExit` uses, deliberately). A defensive
/// `except Exception:` around a tick loop therefore does not catch it, and the
/// host sees an escaped panic rather than an engine error. That is intentional:
/// mis-threading is a bug in the caller, not a state this engine reports. Do
/// not code against the exception's identity — it is not one of this module's
/// exported names, and catching it means catching `BaseException`. Check
/// `threading.get_ident()` against the thread that constructed the object
/// instead.
#[pyclass(unsendable, module = "fidus")]
struct Fidus {
    /// `Option` because the blocking calls `take()` the engine into a
    /// `Python::detach` closure and restore it afterwards.
    ///
    /// Failure mode this tolerates: a Rust panic inside the closure aborts
    /// or unwinds before the restore — the subsequent call then sees `None`
    /// and raises instead of running on a half-known engine. The engine is
    /// `Send` (all four core traits require it), which is exactly what the
    /// move-out pattern relies on; if a future core change drops a `Send`
    /// bound, this file stops compiling, which is the intended alarm.
    engine: Option<FidusEngine>,
}

impl Fidus {
    fn take_engine(&mut self) -> PyResult<FidusEngine> {
        self.engine
            .take()
            .ok_or_else(|| FidusError::new_err("engine closed after a previous panic"))
    }
}

#[pymethods]
impl Fidus {
    /// Connects the layer-shell backend, probes and assembles the engine
    /// (plan P2.1). GIL released: connect + build-time screen classification
    /// do real round-trips and the default classifier's four captures
    /// (~650 ms; if only the gate verdict is needed, see [`Self::probe_gate`]).
    #[staticmethod]
    fn build_wayland(py: Python<'_>) -> PyResult<Self> {
        let engine = py
            .detach(|| FidusBuilder::new().build_with(BackendChoice::WaylandLayer))
            .map_err(init_error)?;
        Ok(Self {
            engine: Some(engine),
        })
    }

    /// Startup pre-check (receipt 7-D): the same frozen P2.1 verdict string
    /// as [`Self::gate_status`], answered without an engine — connect +
    /// registry bind only, zero surfaces and zero captures (~3 ms, against
    /// the ~0.8 s of [`Self::build_wayland`]).
    ///
    /// Honest boundaries: the dynamic-wallpaper degradation axis requires
    /// captures and is reported unknown here, so a `degraded:` verdict can
    /// only come from the multi-monitor axis; `permission_required` is
    /// unreachable — the probe answers "is the wlr-screencopy global
    /// advertised", never portal consent. `connect_to_env` round-trips
    /// without a timeout: a hung compositor blocks this call (GIL released,
    /// so the host can still kill the thread) — treat it as a background
    /// startup probe, not a UI-thread poll.
    #[staticmethod]
    fn probe_gate(py: Python<'_>) -> PyResult<String> {
        py.detach(|| {
            let mut backend = WaylandLayerBackend::connect()
                .map_err(|e| FidusInitError::new_err(format!("no usable display: {e}")))?;
            let gate = ProbeGate::from_environment(backend.probe_environment());
            Ok(best_status(&gate))
        })
    }

    fn __repr__(&self) -> String {
        let calibrated = self
            .engine
            .as_ref()
            .map(|e| e.frame().is_some())
            .unwrap_or(false);
        format!(
            "<fidus.Fidus calibrated={}>",
            if calibrated { "yes" } else { "no" }
        )
    }

    /// Best of {Crosshair, Anchor} per plan P2.1 (Available > Degraded >
    /// PermissionRequired > NotSupported, Crosshair wins ties). Pure env
    /// lookup — no capture, so it keeps the GIL.
    fn gate_status(&self) -> PyResult<String> {
        let engine = self
            .engine
            .as_ref()
            .ok_or_else(|| FidusError::new_err("engine closed after a previous panic"))?;
        Ok(best_status(engine.gate()))
    }

    /// Blocking calibration (seconds; H2 wall-clock unmeasured). The whole
    /// call runs with the GIL released; callers must not invoke it from a
    /// thread that must stay responsive (draft D4 F-2).
    fn calibrate_once(&mut self, py: Python<'_>) -> PyResult<CalibInfo> {
        let mut engine = self.take_engine()?;
        let (engine, result) = py.detach(move || {
            let out = engine.calibrate().map(|frame| {
                let q = frame.quality();
                CalibData {
                    scale: frame.map().linear_scale(),
                    rms_residual_px: q.rms_residual_px,
                    max_residual_px: q.max_residual_px,
                    verification_max_err_px: q.verification_max_err_px,
                    consistency_max_err_px: q.consistency_max_err_px,
                    sample_count: q.sample_count,
                    independent_passes: q.independent_passes,
                }
            });
            (engine, out)
        });
        self.engine = Some(engine);
        let d = result.map_err(calibration_error)?;
        Ok(CalibInfo {
            scale: d.scale,
            rms_residual_px: d.rms_residual_px,
            max_residual_px: d.max_residual_px,
            verification_max_err_px: d.verification_max_err_px,
            consistency_max_err_px: d.consistency_max_err_px,
            sample_count: d.sample_count,
            independent_passes: d.independent_passes,
        })
    }

    /// Registers the caller's own offscreen render. The pixel buffer is
    /// **copied** into the engine (draft D4 F-5): a zero-copy borrow would
    /// let the host mutate a live template by reusing its frame buffer,
    /// which the type system could not catch after the handoff.
    ///
    /// Input validation order and messages are frozen in plan P2.1; every
    /// rejection is `TypeError`/`ValueError` (host-side input faults, not
    /// engine behaviour) and nothing is clamped, transposed or padded.
    ///
    /// # Telling the engine where you drew it (`initial_center`)
    /// Pass `(x, y)` in the **same calibrated logical space** [`Self::estimate`]
    /// returns — the layer-shell usable-area coordinates of the calibrated output,
    /// which is where your own surface margins are expressed. Give the **center**
    /// of the render, not the surface's top-left: the window is built around a
    /// center, so a corner shifts the search half a render away from the truth.
    ///
    /// It is not readable from any platform window API, so it is your belief about
    /// your own drawing, and the engine takes it as nothing more than a search
    /// hint: the first estimate scans a square window centered on it, reaching
    /// `1.5 · max(tpl_w, tpl_h) + 48` pixels on *both* sides — the belief is
    /// converted into the matcher's anchor space, which names where the
    /// template's top-left would sit, so the window is symmetric and its reach is
    /// measured center-to-center. Two details have cost hosts reads: that extent
    /// is in **physical** pixels — the margin is not scaled either — so a 60×40
    /// logical render on a 2.0-scale output reaches 228 px, where arithmetic left
    /// in logical pixels gives 138; and the reach is per-axis (Chebyshev), so a
    /// target whose center is within it diagonally is inside this read's window.
    /// A wrong belief cannot fabricate a position: it spends margin in the
    /// direction you shifted and costs the first few estimates, after which the
    /// window widens 64 px per miss exactly as it does for a lost track.
    ///
    /// Two costs travel with registering, and both are the engine's to state:
    /// registering **always** drops the fix, so every re-registration pays a fresh
    /// cold start; and the beliefless version of that request is priced against
    /// the window it *asks for*, before any clipping to the screen — it asks for
    /// `max(frame) + max(tpl)` px of half-extent, and is refused once that passes
    /// 1999.5 px. Measured boundaries: 634 px of template on a 1366-wide output,
    /// 80 px on a 1920-wide one — one pixel larger and the read is refused. The
    /// refusal looks like an empty screen: it raises `FidusTargetLost` every time,
    /// whatever is on screen, and nothing reports the arithmetic. Passing the
    /// belief is the only way to ask for a big render on a big screen: a believed
    /// request is narrowed to the widest window the guard allows instead of being
    /// refused.
    ///
    /// # The three rungs when a read does not land
    /// In this order: one — search a **narrow window on the belief**, i.e. pass
    /// `initial_center` at the center you actually drew at. Two — **retry without
    /// one**: re-register the same render with `initial_center=None`. That is a
    /// different request and not a repeat, because nothing carries a belief (or a
    /// fix) across a registration, so this read really does price the **whole
    /// output** — and on a large output it may instead be the refusal above,
    /// which is the signal to stop. Three — only now fall back to a synthetic
    /// coordinate of your own. Skipping from one straight to three throws away the
    /// only rung that reports a measurement rather than a guess.
    ///
    /// # Choosing a template that will register
    /// The appearance gate scores the template's self-similarity on its
    /// **luma** plane, and the alpha channel is **not** a mask there: an
    /// `alpha == 0` transparent margin still enters as its stored RGB (often
    /// flat black), so a mostly-transparent sprite — e.g. a Live2D window with
    /// padded margins — can be refused as `FidusUntrackable` even when its
    /// visible content is genuinely trackable. Crop the template to the opaque
    /// content, or composite it over a representative background, first.
    ///
    /// # If registration fails, nothing changes
    /// A refused `register_target` is a no-op on live state: the previously
    /// registered target stays registered and keeps being tracked. So after a
    /// failure [`Self::confidence_ceiling`] still reports *that* target's
    /// ceiling and `estimate` still returns its readings — the failure signal
    /// is the exception alone, never the ceiling. This is deliberate, and it is
    /// the opposite of [`Self::calibrate_once`], which clears the frame first so
    /// a half-solved map can never be used. One consequence for recovery loops:
    /// re-registering to flush the filter (see [`Self::estimate`]) does nothing
    /// at all if that re-registration is itself refused.
    ///
    /// Capture hygiene: a grab taken while the target window is *focused* may
    /// carry compositor decorations (niri's focus ring, for instance, draws a
    /// solid `#7fc8ff` rectangle behind the window). Those pollute the template
    /// and can make an empty background score a false match. Register from a
    /// decoration-free grab and self-check that the decoration colour is absent
    /// inside the target rectangle.
    ///
    /// Coverage of the gate itself: besides the four fine radii (2, 4, 8, 16 px
    /// in every direction), **every** horizontal and vertical lag from 2 px up
    /// to half the template's own extent is probed. A render that repeats inside
    /// itself therefore no longer passes: a template whose right half duplicates
    /// its left used to register at ceiling `0.9759` (measured) and is now
    /// refused as `FidusUntrackable`; with `ambiguous = True` it registers, but
    /// its ceiling falls to the floor `0.05`.
    ///
    /// `ambiguous = True` is a **refusal-gate switch and nothing more**. It
    /// changes no scoring, applies no mask, and never claims — anywhere visible
    /// to you — that this render has more than one plausible location. Its
    /// effect is also conditional in a way that has misled a host: it only ever
    /// fires on a render the gate *would have refused*. On one the gate accepts,
    /// it is inert — the ceiling is whatever `1 − self-similarity` produced, and
    /// passing the flag expecting a warning yields a number that looks exactly
    /// like the one you got without it. That is a correct reading, not a silent
    /// failure: the flag is how you say "I know this appearance is weak and want
    /// a best-effort track anyway", and the price is that the resulting
    /// confidence is labelled weak, not that anything else changes. To tell the
    /// two cases apart, compare the ceiling you get against the floor: `0.05`
    /// means the flag fired; anything else means it did not.
    ///
    /// What that floor buys — and what it does not. Before the sweep, the coarse
    /// repeat was a *silent* mis-lock: on a scripted screen repeating every 32
    /// logical px the engine sat exactly one period off in 6/6 frames, adjacent
    /// readings agreed perfectly, and confidence equalled the 0.9461 ceiling
    /// (all measured). **Judging a reading by agreement between consecutive
    /// estimates therefore cannot detect this** — a stable wrong position is
    /// indistinguishable from a settled right one that way. Compare against where
    /// *you* placed the window. The sweep does not change the geometry of such a
    /// mis-lock; it removes its silence (refused, or honestly floored to 0.05).
    ///
    /// Three limits remain, and none of them is a knob: a screen whose repetition
    /// is *longer* than the template itself (nothing in an appearance can say the
    /// desktop repeats); repeats along vectors that are neither horizontal nor
    /// vertical, which are still sampled only at the fine radii; and large
    /// renders, where a work budget drops whole axes rather than sampling them
    /// thinly. The budget is computable from the template's own size, so a host
    /// can check a render before registering it: for each axis, with `n` pixels
    /// along that axis and `m` along the other, the sweep costs
    /// `Σ (n − l) · m` over the lags `l ∈ [2, n/2]` excluding the four fine
    /// radii. Both axes are priced, the cheaper one is kept first, and an axis is
    /// dropped whole once the running total would pass 60,000,000 pixel-products.
    /// That is why `600×400` keeps its vertical axis and `900×600` keeps neither,
    /// falling back to the fine radii. See [`Self::confidence_ceiling`] and
    /// `docs/spec.md` §11.1.
    #[pyo3(signature = (img, ambiguous = false, initial_center = None))]
    fn register_target(
        &mut self,
        img: &Bound<'_, PyAny>,
        ambiguous: bool,
        initial_center: Option<(f64, f64)>,
    ) -> PyResult<()> {
        let image = parse_rgba(img)?;
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| FidusError::new_err("engine closed after a previous panic"))?;
        let mut target = TargetDescription::new(image);
        if ambiguous {
            target = target.tracking_ambiguous_appearance();
        }
        if let Some((x, y)) = initial_center {
            if !x.is_finite() || !y.is_finite() {
                return Err(PyValueError::new_err("initial_center must be finite"));
            }
            target = target.with_initial_center(LogicalPoint::new(x, y));
        }
        engine.register_target(target).map_err(estimate_error)
    }

    /// Steady-state estimate under the GIL-released pattern (capture is
    /// blocking). Returns `(x, y, confidence)` in calibrated logical pixels.
    ///
    /// There are three outcomes, and only the last raises:
    /// * **match** — `confidence` is a *measurement* confidence bounded above
    ///   by a per-registration ceiling (a distinctive template can still be
    ///   capped if its appearance is self-similar); higher is stronger.
    ///   The first few matches right after the target moves — including a move
    ///   the host itself made — are a **settle transient**, not fresh accuracy:
    ///   the constant-velocity filter converges on the step with a brief,
    ///   lightly overshooting response, so those readings trail the real position
    ///   even though the on-screen target is already where you put it. The trail
    ///   is proportional to the step and shrinks as the gap between reads grows:
    ///   measured on a 216 px move, the first reading trailed by
    ///   `-18.1 / -10.0 / -12.5` px at first-read intervals of 1.07 / 1.32 / 1.16 s
    ///   (see `docs/spec.md` §11.3b for why this is not a stale capture).
    ///   To tell "settled" from "still converging," require two adjacent matches to
    ///   differ by no more than your tolerance — but give it enough readings for
    ///   that to mean anything: on a 20 px move, a host measured four consecutive
    ///   matches still 3.6–4.4 px apart, settling to 0.36 px only by the sixth. This
    ///   rule judges the transient and nothing else; a *stable wrong lock* also
    ///   produces neighbours that agree, see [`Self::register_target`].
    ///   A full miss (a following `coasting`/`exception`) then re-acquisition
    ///   instead resets the track and snaps to the new position in one step, and
    ///   so does registering the target again — which is how to ask for one
    ///   clean reading immediately rather than waiting out the settle, and also
    ///   how to end the runaway described above. Re-registering is the *only*
    ///   flush: `calibrate_once` replaces the coordinate frame and leaves the
    ///   filter state untouched, so it can neither shorten a transient nor
    ///   recover a runaway, and it is orders of magnitude more expensive.
    /// * **coasting** — a prior fix exists but this frame matched nothing:
    ///   returns the Kalman-extrapolated track position with
    ///   `confidence == 0.0` and does **not** raise. Treat `confidence == 0.0`
    ///   as "no measurement this frame — discard the coordinates," never as a
    ///   weak-but-real reading. A stretch of coasting can also be **terminal**:
    ///   each coasting call advances the prediction by the velocity the last
    ///   measurement taught it, nothing re-measures that velocity, and there is
    ///   no re-acquisition timeout (`FidusTargetLost` is reserved for the case
    ///   where no prior fix ever existed). So if a large displacement inflated
    ///   the velocity and the screen then goes still, both layers keep missing
    ///   and the reported position marches away at constant speed indefinitely —
    ///   still labelled `confidence == 0.0`, which is the honest signal, and
    ///   recoverable only by re-registering the target. Do not read per-call
    ///   timing as a health signal either. A host that measured calls in phase
    ///   found one `estimate` costing 47 ms in this state against 163–287 ms
    ///   while locked, and the reason is in the matcher: the coarse pass scans
    ///   the search window *after* clamping it to the frame, so a prediction
    ///   that has run off-screen shrinks the work to a handful of samples.
    ///   The full price is a product of two terms, not one: roughly
    ///   `2 · (window positions / 3) · (template area / 9)` — the coarse grid
    ///   walks the clamped window at a 3 px stride, a constant that depends on
    ///   neither the template nor the window size, and each sample compares the
    ///   whole template, itself downsampled by that same stride. Keep the two
    ///   windows apart when recomputing that: the **refusal** is priced on the
    ///   window you *asked for* — `(2·half + 1)²` against a 16 000 000-position
    ///   cap, before any clipping — while the **work** happens on whatever is
    ///   left after clipping to the frame. Off-screen they differ by orders of
    ///   magnitude, so a cost curve derived from geometry has to clamp first and
    ///   a cap check must not. Both numbers are reproduced in-repo, per size, by
    ///   `belief_window_cost_curve` and `cold_start_cost_breakdown` (both
    ///   `--ignored` measurements in `fidus-estimate`).
    ///   Cost is
    ///   therefore **non-monotone** in template size: measured on a 1366×768
    ///   output the work peaks near a 400 px template edge and falls again past
    ///   it, which is why one host's 633 px render cost no more than its 600 px
    ///   one.
    ///   Duration is therefore a state signal rather than a health signal, and
    ///   the inference is not reversible — the same host saw one tier spread by
    ///   2.4x across runs. (An earlier revision of this paragraph said the call
    ///   interval *halved* in this state; the host that measured it has since
    ///   retracted that as a property of its own loop, three later runs
    ///   disagreeing about the direction.) Coasting is also not where such a
    ///   failure begins: that same host, checking against an independent
    ///   ruler, saw the reported position already wrong by 430–1015 px while
    ///   `confidence` sat exactly at [`Self::confidence_ceiling`], and only
    ///   later in the same runaway did it fall to `0.0`. So `confidence != 0.0`
    ///   is not evidence that a reading is correct.
    ///   A lock that wrong does not wear itself out either, and the reason is one
    ///   number running both rescue routes: *any* absorbed reading — right or
    ///   wrong — clears the miss streak, and that streak is the only thing that
    ///   widens the search window, which is at the same time L1's scan region
    ///   **and** the region L8 extracts its blobs from. So while an on-screen copy
    ///   keeps scoring, the window stays pinned at `1.5·template + 48 px` and a
    ///   truth hundreds of pixels away is invisible to both layers at once, for as
    ///   long as the copy is there. The engine's own tests reproduce it: twelve
    ///   consecutive reads at the ceiling bought nothing toward recovery, and the
    ///   far target was then acquired on the **sixth** read with the copy gone —
    ///   which is arithmetic, not luck: the remaining distance divided by the
    ///   64 px each miss buys back, rounded up, plus one. So the read count is a
    ///   statement about geometry and nothing else, and no amount of patience
    ///   lowers it while a copy keeps scoring. Re-registering — with
    ///   `initial_center` near where you actually put the window — is the exit;
    ///   waiting is not one. It is also the cheap one: a host recovering from
    ///   exactly this runaway measured re-registration at 31.8 ms and was back
    ///   within 5 px on the first read; the same recovery *plus* a
    ///   `calibrate_once` cost that host 2207.7 ms for no better position.
    /// * **exception** — `FidusTargetLost` (never had a fix) or
    ///   `FidusNotCalibrated`. These are genuine failures and are raised, not
    ///   returned as sentinels (spec §4.5); `confidence == 0.0` is the single
    ///   in-band sentinel the return does carry.
    fn estimate<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut engine = self.take_engine()?;
        let (engine, result) = py.detach(move || {
            let out = engine
                .estimate()
                .map(|p| (p.position.x, p.position.y, p.confidence as f64));
            (engine, out)
        });
        self.engine = Some(engine);
        let (x, y, c) = result.map_err(estimate_error)?;
        Ok(PyTuple::new(py, [x, y, c]).expect("three floats"))
    }

    /// Per-registration upper bound on [`Self::estimate`]'s confidence, or
    /// `None` before a target is registered.
    ///
    /// The value is `1 − self-similarity`, so even a distinctive template
    /// lands just **below** `1.0` rather than on it: measured `0.9610` for an
    /// unstructured random-texture render. A template that is self-similar —
    /// where a match is inherently weaker evidence — is capped lower, down to a
    /// floor of `0.05`. Read any value here together with the probe-coverage
    /// note on [`Self::register_target`]: the sweep covers every axial lag up to
    /// half the template's extent, so a low value is a real statement about this
    /// render — but a high one still is not a promise of uniqueness, because a
    /// repetition *longer* than the template is a property of the desktop, and
    /// large templates can fall back to the four fine radii under the work
    /// budget.
    ///
    /// Which is why it is a bound and **not** a quality score to rank renders
    /// against one another: across template sizes it moves the wrong way. A
    /// small render repeats inside itself cheaply, so it advertises a *high*
    /// ceiling while being the least localizable thing on screen; a large
    /// distinctive one advertises a low ceiling while being the most reliable.
    /// A host measured precisely that inversion on one live subject — the render
    /// that reported a position 124 px off claimed `0.371`, the render that was
    /// right by `0.00` px claimed `0.069`, 5.4x apart — and it noted that only
    /// the *direction* survives a change of subject, not the values, which drift
    /// batch to batch (the same render was seen at `0.289`, `0.360` and `0.371`).
    /// So: comparing ceilings inside one size class is meaningful; sorting
    /// candidates *across* sizes by ceiling selects the smallest and most
    /// misleading render, which is the opposite of the choice the number appears
    /// to recommend. Take the larger render and let its ceiling be low.
    ///
    /// It is a **constant for the lifetime of one `register_target`** — read
    /// it once after registering to know how far below `1.0` every subsequent
    /// `estimate` on this target is capped — and
    /// **not** a per-frame health signal (that is `estimate`'s own return).
    /// After a *refused* re-registration it still describes the target that is
    /// still being tracked, so it cannot tell you whether registering worked
    /// (see [`Self::register_target`]).
    /// This is the value that was previously only observable by watching
    /// `estimate` plateau; exposing it lets a host distinguish "capped by a
    /// weak appearance" from "genuinely low this frame."
    ///
    /// Read-only with one hole worth knowing about, because it silently defeats
    /// the check above: assigning to the attribute on an *instance* raises
    /// `AttributeError`, but assigning to `Fidus.confidence_ceiling` writes an
    /// entry in the class dict, and attribute lookup consults that dict before
    /// the getter — so a single class-level assignment shadows the reported
    /// ceiling for **every** instance in the process, including engines created
    /// afterwards (host-measured on the shipped wheel). A test fixture that
    /// configures attributes by name therefore makes any "advertised == applied"
    /// comparison vacuously true. `del` is not a repair: it removes the getter
    /// itself, so the attribute then fails to exist on every instance too, in a
    /// clean process exactly as in a polluted one. The reversible teardown
    /// snapshots the descriptor and puts it back:
    /// ``saved = vars(Fidus)["confidence_ceiling"]`` …
    /// ``setattr(Fidus, "confidence_ceiling", saved)``, which restores the
    /// getter and the per-instance reading bit for bit (host-measured). Both
    /// halves of that are load-bearing: `vars()` returns a read-only mapping
    /// proxy, so there is no pop-the-entry route, and instances have no
    /// `__dict__`, so the class is the only place this can be written and the
    /// only place it can be cleaned.
    #[getter]
    fn confidence_ceiling(&self) -> Option<f32> {
        self.engine
            .as_ref()
            .and_then(|engine| engine.confidence_ceiling())
    }
}

/// Ranks {Crosshair, Anchor} on any gate and renders the frozen P2.1
/// verdict string. Shared by the instance `gate_status` and the class-level
/// `probe_gate` so both answer from one vocabulary.
fn best_status(gate: &dyn Gate) -> String {
    let methods = [CalibrationMethod::Crosshair, CalibrationMethod::Anchor];
    let mut best: Option<CalibrationStatus> = None;
    let rank = |s: &CalibrationStatus| match s {
        CalibrationStatus::Available { .. } => 0,
        CalibrationStatus::Degraded { .. } => 1,
        CalibrationStatus::PermissionRequired { .. } => 2,
        CalibrationStatus::NotSupported { .. } => 3,
    };
    for m in methods {
        let s = gate.query_calibrator_availability(m);
        if best.as_ref().is_none_or(|b| rank(&s) < rank(b)) {
            best = Some(s);
        }
    }
    match best.expect("two methods always queried") {
        CalibrationStatus::Available { .. } => "available".to_string(),
        CalibrationStatus::Degraded {
            estimated_confidence,
            ..
        } => format!("degraded:{estimated_confidence:.2}"),
        CalibrationStatus::PermissionRequired { permission, .. } => {
            format!("permission_required:{}", permission.name())
        }
        CalibrationStatus::NotSupported { reason, .. } => {
            format!("unsupported:{}", reason.summary())
        }
    }
}

/// Extracts a C-contiguous `(h, w, 4)` uint8 buffer per plan P2.1.
/// Downcasting to `PyArrayDyn<u8>` fuses the "is an ndarray" and "dtype is
/// uint8" checks; both failure classes surface as `TypeError` with the
/// frozen message (a float32 array reaching the engine would misread every
/// byte as a pixel).
fn parse_rgba(img: &Bound<'_, PyAny>) -> PyResult<RgbaImage> {
    let arr = img
        .cast::<PyArrayDyn<u8>>()
        .map_err(|_| PyTypeError::new_err("img must be a numpy uint8 RGBA array"))?;
    let shape = arr.shape();
    if arr.ndim() != 3 || shape.len() != 3 || shape[2] != 4 {
        return Err(PyValueError::new_err("expected shape (h, w, 4)"));
    }
    if !arr.is_c_contiguous() {
        return Err(PyValueError::new_err("img must be C-contiguous"));
    }
    let (h, w) = (shape[0] as u32, shape[1] as u32);
    if h == 0 || w == 0 {
        return Err(PyValueError::new_err("empty or inconsistent buffer"));
    }
    // `to_vec` performs the mandated copy; lengths are array-invariant here,
    // but RgbaImage re-checks (is_valid) so a bound-crossing bug stays loud.
    //
    // # Safety (why the unchecked slice borrow cannot break)
    // `as_slice` requires contiguity: checked two lines above. A host thread
    // resizing or de-allocating the array between check and copy would break
    // it, but the GIL is held for this whole function (no `detach` on this
    // path), so no other Python code can run while the borrow is live.
    let data = unsafe {
        arr.as_slice()
            .map_err(|_| PyValueError::new_err("img must be C-contiguous"))?
    }
    .to_vec();
    let image = RgbaImage::from_raw(w, h, data);
    if !image.is_valid() {
        return Err(PyValueError::new_err("empty or inconsistent buffer"));
    }
    Ok(image)
}

#[pymodule]
#[pyo3(name = "fidus")]
fn init_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Fidus>()?;
    m.add_class::<CalibInfo>()?;
    m.add("FidusError", m.py().get_type::<FidusError>())?;
    m.add("FidusInitError", m.py().get_type::<FidusInitError>())?;
    m.add("FidusCalibrationError", m.py().get_type::<FidusCalibrationError>())?;
    m.add("FidusEstimateError", m.py().get_type::<FidusEstimateError>())?;
    m.add("FidusNotCalibrated", m.py().get_type::<FidusNotCalibrated>())?;
    m.add("FidusNoTarget", m.py().get_type::<FidusNoTarget>())?;
    m.add("FidusTargetLost", m.py().get_type::<FidusTargetLost>())?;
    m.add("FidusUntrackable", m.py().get_type::<FidusUntrackable>())?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add(
        "__git_commit__",
        option_env!("FIDUS_GIT_DESCRIBE").unwrap_or("unknown"),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    // Run with `cargo test --no-default-features`: the default feature set
    // links no libpython (extension-module), tests embed one (auto-initialize).
    use super::*;

    fn eval_array<'py>(py: Python<'py>, src: &std::ffi::CStr) -> Bound<'py, pyo3::PyAny> {
        let globals = pyo3::types::PyDict::new(py);
        globals
            .set_item("np", py.import("numpy").expect("numpy required for binding tests"))
            .unwrap();
        py.eval(src, Some(&globals), None).unwrap()
    }

    fn parse_err(py: Python<'_>, src: &std::ffi::CStr) -> PyErr {
        let obj = eval_array(py, src);
        parse_rgba(&obj).expect_err("input must be rejected")
    }

    #[test]
    fn rejects_non_ndarray_and_wrong_dtype() {
        Python::attach(|py| {
            let e = parse_err(py, c"42");
            assert!(e.is_instance_of::<PyTypeError>(py), "{e}");
            assert_eq!(
                e.value(py).str().unwrap().to_string(),
                "img must be a numpy uint8 RGBA array"
            );
            let e = parse_err(py, c"np.zeros((2, 2, 4), dtype=np.float32)");
            assert!(e.is_instance_of::<PyTypeError>(py), "{e}");
        });
    }

    #[test]
    fn rejects_shape_contiguity_and_empty() {
        Python::attach(|py| {
            let e = parse_err(py, c"np.zeros((2, 8), dtype=np.uint8)");
            assert!(e.is_instance_of::<PyValueError>(py), "{e}");
            assert_eq!(e.value(py).str().unwrap().to_string(), "expected shape (h, w, 4)");
            let e = parse_err(py, c"np.zeros((2, 8, 3), dtype=np.uint8)");
            assert_eq!(e.value(py).str().unwrap().to_string(), "expected shape (h, w, 4)");
            let e = parse_err(py, c"np.zeros((8, 8, 4), dtype=np.uint8)[:, ::-1]");
            assert_eq!(
                e.value(py).str().unwrap().to_string(),
                "img must be C-contiguous"
            );
            let e = parse_err(py, c"np.zeros((0, 4, 4), dtype=np.uint8)");
            assert_eq!(
                e.value(py).str().unwrap().to_string(),
                "empty or inconsistent buffer"
            );
        });
    }

    #[test]
    fn accepts_valid_rgba_and_copies() {
        Python::attach(|py| {
            let obj = eval_array(py, c"np.arange(2 * 3 * 4, dtype=np.uint8).reshape(2, 3, 4)");
            let img = parse_rgba(&obj).expect("valid buffer must be accepted");
            assert!(img.is_valid());
            assert_eq!((img.width(), img.height(), img.byte_len()), (3, 2, 24));
            assert_eq!(img.rgba(2, 1), [20, 21, 22, 23]);
            // The numpy temporary is dropped here; the engine-side copy must
            // keep the pixel intact (draft D4 F-5).
            drop(obj);
            assert_eq!(img.rgba(2, 1), [20, 21, 22, 23]);
        });
    }

    #[test]
    fn estimate_errors_map_to_frozen_classes() {
        Python::attach(|py| {
            let cases = [
                (EstimateError::NotCalibrated, "FidusNotCalibrated"),
                (EstimateError::NoTarget, "FidusNoTarget"),
                (EstimateError::TargetLost, "FidusTargetLost"),
            ];
            for (err, class) in cases {
                let msg = err.to_string();
                let e = estimate_error(err);
                let v = e.value(py);
                assert_eq!(v.get_type().name().unwrap().to_string(), class);
                assert_eq!(v.str().unwrap().to_string(), msg, "message must pass through verbatim");
            }
            assert!(estimate_error(EstimateError::InvalidMeasurement)
                .value(py)
                .is_instance(&py.get_type::<FidusEstimateError>())
                .unwrap());
        });
    }
}
