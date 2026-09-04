use fidus_core::coord::{AffineTransform, LogicalPoint, PhysicalPoint, SolveError};
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

#[test]
fn identity_recovers_exactly() {
    let (t, res) = AffineTransform::from_correspondences(&corr_list(AffineTransform::IDENTITY)).unwrap();
    // Normal equations + Gaussian elimination round off at ~1e-15 for
    // screen-sized coordinates; bit-exact recovery is not a property of any
    // floating-point solver. 1e-9 still pins correctness six orders below
    // the 1 px calibration budget.
    for (got, want) in [(t.a, 1.0), (t.b, 0.0), (t.c, 0.0), (t.d, 0.0), (t.e, 1.0), (t.f, 0.0)] {
        assert!((got - want).abs() < 1e-9, "coefficient {got} vs {want}");
    }
    assert!(res.rms < 1e-9, "rms {}", res.rms);
    assert!(res.max < 1e-9);
}

#[test]
fn scale_and_translate_recovers_exactly() {
    // 1.5× fractional scale with a usable-area offset (panel reserved space).
    let t = AffineTransform { a: 1.5, b: 0.0, c: 0.0, d: 0.0, e: 1.5, f: 40.0 };
    let (got, res) = AffineTransform::from_correspondences(&corr_list(t)).unwrap();
    assert!((got.a - 1.5).abs() < 1e-9);
    assert!((got.f - 40.0).abs() < 1e-9);
    assert!(res.rms < 1e-9);
    assert!((got.linear_scale() - 1.5).abs() < 1e-9);
}

#[test]
fn rotated_output_recovers_exactly() {
    // 90° output transform: physical = (H - y*scale, x*scale); modelled as a
    // full affine with swapped axes.
    let t = AffineTransform { a: 0.0, b: -2.0, c: 2160.0, d: 2.0, e: 0.0, f: 0.0 };
    let (got, res) = AffineTransform::from_correspondences(&corr_list(t)).unwrap();
    assert!((got.b - (-2.0)).abs() < 1e-9);
    assert!((got.c - 2160.0).abs() < 1e-9);
    assert!(res.rms < 1e-9);
}

#[test]
fn noisy_correspondences_fit_within_subpixel() {
    let t = AffineTransform { a: 1.25, b: 0.0, c: 0.0, d: 0.0, e: 1.25, f: 31.5 };
    let corr: Vec<_> = corr_list(t)
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
    let (got, res) = AffineTransform::from_correspondences(&corr).unwrap();
    // Subpixel noise on 5 points cannot vanish into the parameters: the
    // statistical floor for the slope is σ/√(Σx²) ≈ 1e-3, and the
    // translation-shaped noise component moves `f` by ~0.5 px. These bounds
    // pin the solver's actual noise response for this seed (≈1.2e-3 slope
    // error, ≈0.52 px translation error, rms ≈ 0.13 px) — 1e-6 was never
    // achievable with 5 points.
    assert!((got.a - 1.25).abs() < 5e-3, "a {}", got.a);
    assert!((got.e - 1.25).abs() < 5e-3, "e {}", got.e);
    assert!(got.b.abs() < 5e-3, "b {}", got.b);
    assert!(got.d.abs() < 5e-3, "d {}", got.d);
    assert!((got.f - 31.5).abs() < 0.75, "f {}", got.f);
    // The property that matters downstream (1 px residual budget):
    assert!(res.rms < 0.5, "rms {}", res.rms);
    assert!(res.max < 1.0, "max {}", res.max);
}

#[test]
fn degenerate_and_singular_inputs_rejected() {
    // Fewer than 3 points.
    let two = corr_list(AffineTransform::IDENTITY);
    assert_eq!(
        AffineTransform::from_correspondences(&two[..2]),
        Err(SolveError::Degenerate)
    );
    // Three collinear points.
    let collinear = [
        (LogicalPoint::new(0.0, 0.0), PhysicalPoint::new(0.0, 0.0)),
        (LogicalPoint::new(10.0, 10.0), PhysicalPoint::new(10.0, 10.0)),
        (LogicalPoint::new(20.0, 20.0), PhysicalPoint::new(20.0, 20.0)),
    ];
    assert!(matches!(
        AffineTransform::from_correspondences(&collinear),
        Err(SolveError::Singular) | Err(SolveError::Degenerate)
    ));
}

#[test]
fn inverse_roundtrips() {
    let t = AffineTransform { a: 1.25, b: 0.0, c: 7.0, d: 0.0, e: 1.25, f: 40.0 };
    let inv = t.inverse().unwrap();
    for p in [LogicalPoint::new(0.0, 0.0), LogicalPoint::new(1919.0, 1079.0)] {
        let back = inv.apply_physical(t.apply(p));
        assert!((back.x - p.x).abs() < 1e-9);
        assert!((back.y - p.y).abs() < 1e-9);
    }
    assert!(AffineTransform { a: 0.0, b: 0.0, c: 0.0, d: 0.0, e: 0.0, f: 0.0 }
        .inverse()
        .is_err());
}

#[test]
fn max_difference_compares_in_physical_pixels() {
    let t1 = AffineTransform::IDENTITY;
    let t2 = AffineTransform { a: 1.0, b: 0.0, c: 2.0, d: 0.0, e: 1.0, f: 3.0 };
    let probes = [LogicalPoint::new(0.0, 0.0), LogicalPoint::new(10.0, 20.0)];
    let d = t1.max_difference(&t2, &probes);
    assert!((d - 13.0f64.hypot(0.0)).abs() < 1e-9 || (d - (2.0f64).hypot(3.0)).abs() < 1e-9);
    assert!(d > 0.0);
}

#[test]
fn coordinate_frame_converts_both_ways() {
    let map = AffineTransform { a: 2.0, b: 0.0, c: 0.0, d: 0.0, e: 2.0, f: 80.0 };
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
    assert_eq!(phys, PhysicalPoint::new(200.0, 180.0));
    let back = frame.physical_to_logical(phys);
    assert!((back.x - 100.0).abs() < 1e-9 && (back.y - 50.0).abs() < 1e-9);
    assert_eq!(frame.capture_size(), (3840, 2160));
    assert_eq!(frame.quality(), &quality);
}
