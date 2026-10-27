use super::*;

#[test]
fn security_round_stroke_subdivision_is_bounded() {
    for radius in [1.0, 1_000_000.0, 134_217_728.0, f32::MAX, f32::INFINITY, f32::NAN, -1.0, 0.0] {
        for tolerance in [0.25, f32::MIN_POSITIVE, f32::from_bits(1), 0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!((2..=1024).contains(&curve_divisions(radius, std::f32::consts::PI, tolerance)), "{radius}, {tolerance}");
        }
    }
    for (radius, tolerance, expected) in [(1.0, 0.25, 3), (8.0, 0.25, 7), (100.0, 0.25, 23), (4096.0, 0.03125, 403)] {
        assert_eq!(curve_divisions(radius, std::f32::consts::PI, tolerance), expected);
    }
    let mut path = crate::Path::new();
    path.move_to(0.0, 0.0);
    path.line_to(1.0, 0.0);
    path.line_to(1.0, 1.0);
    let mut cache = PathCache::new(path.verbs(), &Transform2D::identity(), 0.25, 0.01);
    cache.expand_stroke(134_217_728.0, 1.0, LineCap::Round, LineCap::Round, LineJoin::Round, 10.0, 0.25);
    assert!(cache.contours.iter().map(|c| c.stroke.len()).sum::<usize>() < 10_000);
}
