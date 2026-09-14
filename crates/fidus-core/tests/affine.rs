// Copyright 2026 Flexiatom
// SPDX-License-Identifier: Apache-2.0

use fidus_core::coord::{AffineTransform, BoundingBox, LogicalPoint, PhysicalPoint, SolveError};
use fidus_core::frame::{CalibrationQuality, CoordinateFrame};

fn corr_list(t: AffineTransform) -> Vec<(LogicalPoint, PhysicalPoint)> {
    [
        LogicalPoint::new(0.0, 0.0),
        LogicalPoint::new(100.0, 0.0),
        LogicalPoint::new(0.0, 100.0),
        LogicalPoint::new(312.5, 87.25),
        LogicalPoint::new(987.5, 543.75),
    ]
    .into_iter()
    .map(|p| (p, t.apply(p)))
    .collect()
}

/// Builds correspondences for `physical = (a·x + c, e·y + f)` directly, for
/// the cases that need a specific map without one already in hand.
fn corr_from(a: f64, c: f64, e: f64, f: f64) -> Vec<(LogicalPoint, PhysicalPoint)> {
    [
        LogicalPoint::new(0.0, 0.0),
        LogicalPoint::new(100.0, 0.0),
        LogicalPoint::new(0.0, 100.0),
        LogicalPoint::new(312.5, 87.25),
        LogicalPoint::new(987.5, 543.75),
    ]
    .into_iter()
    .map(|p| (p, PhysicalPoint::new(a * p.x + c, e * p.y + f)))
    .collect()
}

#[test]
fn identity_recovers_exactly() {
    let solved =
        AffineTransform::from_correspondences(&corr_list(AffineTransform::IDENTITY)).unwrap();
    let (t, res) = (solved.map(), solved.residuals());
    // Normal equations + Gaussian elimination round off at ~1e-15 for
    // screen-sized coordinates; bit-exact recovery is not a property of any
    // floating-point solver. 1e-9 still pins correctness six orders below
    // the 1 px calibration budget.
    for (got, want) in t
        .coefficients()
        .into_iter()
        .zip([1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
    {
        assert!((got - want).abs() < 1e-9, "coefficient {got} vs {want}");
    }
    assert!(res.rms < 1e-9, "rms {}", res.rms);
    assert!(res.max < 1e-9);
    assert_eq!(solved.point_count(), 5);
}

#[test]
fn scale_and_translate_recovers_exactly() {
    // 1.5× fractional scale with a usable-area offset (panel reserved space).
    let solved = AffineTransform::from_correspondences(&corr_from(1.5, 0.0, 1.5, 40.0)).unwrap();
    let (got, res) = (solved.map(), solved.residuals());
    let [a, _b, _c, _d, _e, f] = got.coefficients();
    assert!((a - 1.5).abs() < 1e-9);
    assert!((f - 40.0).abs() < 1e-9);
    assert!(res.rms < 1e-9);
    assert!((got.linear_scale() - 1.5).abs() < 1e-9);
}

#[test]
fn rotated_output_recovers_exactly() {
    // 90° output transform: physical = (H - y*scale, x*scale); modelled as a
    // full affine with swapped axes.
    let corr: Vec<_> = [
        LogicalPoint::new(0.0, 0.0),
        LogicalPoint::new(100.0, 0.0),
        LogicalPoint::new(0.0, 100.0),
        LogicalPoint::new(312.5, 87.25),
        LogicalPoint::new(987.5, 543.75),
    ]
    .into_iter()
    .map(|p| (p, PhysicalPoint::new(-2.0 * p.y + 2160.0, 2.0 * p.x)))
    .collect();
    let solved = AffineTransform::from_correspondences(&corr).unwrap();
    let [_a, b, c, _d, _e, _f] = solved.map().coefficients();
    assert!((b - (-2.0)).abs() < 1e-9);
    assert!((c - 2160.0).abs() < 1e-9);
    assert!(solved.residuals().rms < 1e-9);
}

#[test]
fn noisy_correspondences_fit_within_subpixel() {
    let corr: Vec<_> = corr_from(1.25, 0.0, 1.25, 31.5)
        .into_iter()
        .map(|(l, p)| {
            // ±0.5 px integer-centroid quantization noise. Note the noise
            // structure: integer coordinates always give fract() == 0, so the
            // first three points share (-0.5, -0.5) — translation-shaped and
            // only partially absorbable by the fit.
            let n = ((l.x * 7919.0).fract() - 0.5, (l.y * 104729.0).fract() - 0.5);
            (l, PhysicalPoint::new(p.x + n.0, p.y + n.1))
        })
        .collect();
    let solved = AffineTransform::from_correspondences(&corr).unwrap();
    let (got, res) = (solved.map(), solved.residuals());
    // Subpixel noise on 5 points cannot vanish into the parameters: the
    // statistical floor for the slope is σ/√(Σx²) ≈ 1e-3, and the
    // translation-shaped noise component moves `f` by ~0.5 px. These bounds
    // pin the solver's actual noise response for this seed (≈1.2e-3 slope
    // error, ≈0.52 px translation error, rms ≈ 0.13 px) — 1e-6 was never
    // achievable with 5 points.
    let [a, b, _c, d, e, f] = got.coefficients();
    assert!((a - 1.25).abs() < 5e-3, "a {a}");
    assert!((e - 1.25).abs() < 5e-3, "e {e}");
    assert!(b.abs() < 5e-3, "b {b}");
    assert!(d.abs() < 5e-3, "d {d}");
    assert!((f - 31.5).abs() < 0.75, "f {f}");
    // The property that matters downstream (1 px residual budget):
    assert!(res.rms < 0.5, "rms {}", res.rms);
    assert!(res.max < 1.0, "max {}", res.max);
}

#[test]
fn degenerate_and_singular_inputs_rejected() {
    // Fewer than 3 points.
    let two = corr_list(AffineTransform::IDENTITY);
    assert_eq!(
        AffineTransform::from_correspondences(&two[..2]).map(|s| s.map()),
        Err(SolveError::Degenerate)
    );
    // Three collinear points.
    let collinear = [
        (LogicalPoint::new(0.0, 0.0), PhysicalPoint::new(0.0, 0.0)),
        (
            LogicalPoint::new(10.0, 10.0),
            PhysicalPoint::new(10.0, 10.0),
        ),
        (
            LogicalPoint::new(20.0, 20.0),
            PhysicalPoint::new(20.0, 20.0),
        ),
    ];
    assert!(matches!(
        AffineTransform::from_correspondences(&collinear).map(|s| s.map()),
        Err(SolveError::Singular) | Err(SolveError::Degenerate)
    ));
}

#[test]
fn non_finite_correspondences_are_rejected_without_nan_results() {
    let base = corr_list(AffineTransform::IDENTITY);
    for index in 0..4 {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut corr = base.clone();
            match index {
                0 => corr[0].0.x = value,
                1 => corr[0].0.y = value,
                2 => corr[0].1.x = value,
                _ => corr[0].1.y = value,
            }
            assert_eq!(
                AffineTransform::from_correspondences(&corr),
                Err(SolveError::NonFinite),
                "field {index}, value {value:?}"
            );
        }
    }
}

#[test]
fn finite_but_overflowing_fit_is_rejected() {
    let corr = [
        (LogicalPoint::new(0.0, 0.0), PhysicalPoint::new(0.0, 0.0)),
        (
            LogicalPoint::new(f64::MAX, 0.0),
            PhysicalPoint::new(1.0, 0.0),
        ),
        (
            LogicalPoint::new(0.0, f64::MAX),
            PhysicalPoint::new(0.0, 1.0),
        ),
    ];
    assert_eq!(
        AffineTransform::from_correspondences(&corr),
        Err(SolveError::NonFinite)
    );
}

#[test]
fn inverse_roundtrips() {
    let t = AffineTransform::from_correspondences(&corr_from(1.25, 7.0, 1.25, 40.0))
        .unwrap()
        .map();
    let inv = t.inverse().unwrap();
    for p in [
        LogicalPoint::new(0.0, 0.0),
        LogicalPoint::new(1919.0, 1079.0),
    ] {
        let back = inv.apply_physical(t.apply(p));
        assert!((back.x - p.x).abs() < 1e-9);
        assert!((back.y - p.y).abs() < 1e-9);
    }
}

#[test]
fn singular_map_has_no_inverse() {
    // A map collapsing every logical point onto one physical point: all
    // correspondences share the same physical value, so the solve yields a
    // zero linear part. Degeneracy must surface as an error, never as a
    // usable map (project convention 4: reject, do not paper over).
    let corr: Vec<_> = [(0.0, 0.0), (100.0, 0.0), (0.0, 100.0), (50.0, 50.0)]
        .into_iter()
        .map(|(x, y)| (LogicalPoint::new(x, y), PhysicalPoint::new(10.0, 10.0)))
        .collect();
    let solved = AffineTransform::from_correspondences(&corr).unwrap();
    assert_eq!(solved.map().inverse(), Err(SolveError::Singular));
}

#[test]
fn max_difference_compares_in_physical_pixels() {
    let t1 = AffineTransform::IDENTITY;
    let t2 = AffineTransform::from_correspondences(&corr_from(1.0, 2.0, 1.0, 3.0))
        .unwrap()
        .map();
    let probes = [LogicalPoint::new(0.0, 0.0), LogicalPoint::new(10.0, 20.0)];
    let d = t1.max_difference(&t2, &probes);
    assert!((d - (2.0f64).hypot(3.0)).abs() < 1e-9, "d = {d}");
}

#[test]
fn coordinate_frame_converts_both_ways() {
    let map = AffineTransform::from_correspondences(&corr_from(2.0, 0.0, 2.0, 80.0)).unwrap();
    let quality = CalibrationQuality {
        rms_residual_px: 0.1,
        max_residual_px: 0.2,
        verification_max_err_px: 0.5,
        consistency_max_err_px: 0.5,
        sample_count: 5,
        independent_passes: 2,
    };
    let frame = CoordinateFrame::new(
        map,
        (3840, 2160),
        fidus_core::calibration::CalibrationMethod::Crosshair,
        quality,
        std::time::SystemTime::now(),
    )
    .unwrap();

    let p = LogicalPoint::new(100.0, 50.0);
    let phys = frame.logical_to_physical(p);
    assert!(
        (phys.x - 200.0).abs() < 1e-9 && (phys.y - 180.0).abs() < 1e-9,
        "{phys:?}"
    );
    let back = frame.physical_to_logical(phys);
    assert!((back.x - 100.0).abs() < 1e-9 && (back.y - 50.0).abs() < 1e-9);
    assert_eq!(frame.capture_size(), (3840, 2160));
    assert_eq!(frame.quality(), &quality);

    let invalid_quality = CalibrationQuality { rms_residual_px: f64::NAN, ..quality };
    let invalid_map = AffineTransform::from_correspondences(&corr_from(2.0, 0.0, 2.0, 80.0)).unwrap();
    assert!(CoordinateFrame::new(
        invalid_map,
        (3840, 2160),
        fidus_core::calibration::CalibrationMethod::Crosshair,
        invalid_quality,
        std::time::SystemTime::now(),
    )
    .is_err());
    assert!(BoundingBox::try_new(1, 1, 1, 2).is_none());
}

/// Principle 1, enforced by the compiler rather than by review.
///
/// These are *documentation of a type-level invariant*, not behavioral
/// assertions — they exist so that anyone loosening the encapsulation sees
/// what it was protecting. The real enforcement is the `compile_fail`
/// doctest in `fidus-core/src/coord.rs`; this test states the positive side:
/// the only route to a map is measurement.
#[test]
fn a_map_can_only_be_obtained_by_solving_or_inverting() {
    // Route 1: solve from correspondences (what calibrators do).
    let solved = AffineTransform::from_correspondences(&corr_from(1.25, 3.0, 1.25, 7.0)).unwrap();
    assert!((solved.map().linear_scale() - 1.25).abs() < 1e-9);

    // Route 2: invert a solved map (what CoordinateFrame does internally).
    assert!(solved.map().inverse().is_ok());

    // Route 3: the identity constant — reachable, but it carries neither a
    // translation nor a scale, so it cannot smuggle a platform position in…
    assert_eq!(
        AffineTransform::IDENTITY.coefficients(),
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
    );
    // …and on its own it cannot become a CoordinateFrame, because that needs
    // a SolvedMap. `CoordinateFrame::new(AffineTransform::IDENTITY, ...)`
    // does not compile — the "1:1 default frame" mistake is unexpressible.

    // There is no route 4: the coefficients are private, so a native rect
    // cannot be spelled as a map.
}
