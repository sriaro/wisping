use super::*;

#[test]
fn security_dash_rejects_patterns_without_usable_intervals() {
    for dash in [vec![f32::EPSILON; 2], vec![f32::EPSILON; 7], vec![0.0, f32::EPSILON, 0.0]] {
        assert!(normalize_dash_pattern(&dash).is_empty());
        let mut path = Path::new();
        path.move_to(0.0, 0.0);
        path.line_to(10.0, 0.0);
        assert_eq!(format!("{:?}", path.dashed(&dash, 0.0).verbs().collect::<Vec<_>>()),
            format!("{:?}", path.verbs().collect::<Vec<_>>()));
    }
    assert_eq!(normalize_dash_pattern(&[f32::EPSILON, 2.0]), [f32::EPSILON, 2.0]);
}

#[test]
fn security_dash_rejects_overflow_and_nonfinite_geometry() {
    assert!(normalize_dash_pattern(&[f32::MAX, f32::MAX]).is_empty());
    assert!(normalize_dash_pattern(&[f32::MAX]).is_empty());
    let mut path = Path::new();
    path.move_to(0.0, 0.0);
    path.line_to(f32::MAX, f32::MAX);
    assert_eq!(format!("{:?}", path.dashed(&[2.0, 1.0], 0.0).verbs().collect::<Vec<_>>()),
        format!("{:?}", path.verbs().collect::<Vec<_>>()));
}

#[cfg(feature = "serde")]
#[test]
fn security_path_deserialize_rejects_coordinate_mismatch() {
    for (verb, count) in [("MoveTo", 1), ("LineTo", 1), ("BezierTo", 3)] {
        for supplied in 0..count {
            let coords = vec![serde_json::json!({"x": 1.0, "y": 2.0}); supplied];
            let input = serde_json::json!({"verbs": [verb], "coords": coords, "last_pos": {"x": 0.0, "y": 0.0}, "dist_tol": 0.01});
            assert!(serde_json::from_value::<Path>(input).is_err(), "{verb}: {supplied}");
        }
    }
    for verbs in [vec![], vec!["Close"], vec!["Solid", "Hole"]] {
        let input = serde_json::json!({"verbs": verbs, "coords": [{"x": 1.0, "y": 2.0}], "last_pos": {"x": 0.0, "y": 0.0}, "dist_tol": 0.01});
        assert!(serde_json::from_value::<Path>(input).is_err());
    }
}

#[cfg(feature = "serde")]
#[test]
fn security_path_roundtrip_preserves_wire_shape_and_metadata() {
    let mut path = Path::new();
    path.move_to(1.0, 2.0);
    path.line_to(3.0, 4.0);
    path.bezier_to(5.0, 6.0, 7.0, 8.0, 9.0, 10.0);
    path.close();
    path.solidity(Solidity::Solid);
    path.solidity(Solidity::Hole);
    path.set_distance_tolerance(0.125);
    path.last_pos = Position { x: 42.0, y: -17.0 };
    let wire = serde_json::to_value(&path).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 4);
    let decoded: Path = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
    assert_eq!(format!("{:?}", decoded.verbs().collect::<Vec<_>>()), format!("{:?}", path.verbs().collect::<Vec<_>>()));
    assert!(decoded.cache.borrow().is_none());
    let zero = serde_json::json!({"verbs": ["Solid", "Hole", "Close"], "coords": [], "last_pos": {"x": 0.0, "y": 0.0}, "dist_tol": 0.0});
    assert_eq!(serde_json::from_value::<Path>(zero).unwrap().verbs().count(), 3);
}
