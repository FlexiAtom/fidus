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
    /// # Choosing a template that will register
    /// The appearance gate scores the template's self-similarity on its
    /// **luma** plane, and the alpha channel is **not** a mask there: an
    /// `alpha == 0` transparent margin still enters as its stored RGB (often
    /// flat black), so a mostly-transparent sprite — e.g. a Live2D window with
    /// padded margins — can be refused as `FidusUntrackable` even when its
    /// visible content is genuinely trackable. Crop the template to the opaque
    /// content, or composite it over a representative background, first.
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
    /// thinly — measured, `600×400` keeps its vertical axis and `900×600` keeps
    /// neither, falling back to the fine radii. Whether a given template fits is
    /// computable from its size; see [`Self::confidence_ceiling`] and
    /// `docs/spec.md` §11.1.
    #[pyo3(signature = (img, ambiguous = false))]
    fn register_target(&mut self, img: &Bound<'_, PyAny>, ambiguous: bool) -> PyResult<()> {
        let image = parse_rgba(img)?;
        let engine = self
            .engine
            .as_mut()
            .ok_or_else(|| FidusError::new_err("engine closed after a previous panic"))?;
        let mut target = TargetDescription::new(image);
        if ambiguous {
            target = target.tracking_ambiguous_appearance();
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
    ///   lightly overshooting response over roughly three-to-five estimates, so
    ///   those readings trail the real position even though the on-screen target
    ///   is already where you put it. To tell "settled" from "still converging,"
    ///   require two adjacent matches to differ by no more than your tolerance;
    ///   a full miss (a following `coasting`/`exception`) then re-acquisition
    ///   instead resets the track and snaps to the new position in one step, and
    ///   so does registering the target again — which is how to ask for one
    ///   clean reading immediately rather than waiting out the settle.
    /// * **coasting** — a prior fix exists but this frame matched nothing:
    ///   returns the Kalman-extrapolated track position with
    ///   `confidence == 0.0` and does **not** raise. Treat `confidence == 0.0`
    ///   as "no measurement this frame — discard the coordinates," never as a
    ///   weak-but-real reading.
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
    /// It is a **constant for the lifetime of one `register_target`** — read
    /// it once after registering to know how far below `1.0` every subsequent
    /// `estimate` on this target is capped — and
    /// **not** a per-frame health signal (that is `estimate`'s own return).
    /// This is the value that was previously only observable by watching
    /// `estimate` plateau; exposing it lets a host distinguish "capped by a
    /// weak appearance" from "genuinely low this frame."
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
