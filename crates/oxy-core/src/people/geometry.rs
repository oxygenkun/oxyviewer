//! Shared geometry contract for cached detections and manual instances.

pub(crate) fn valid_box(value: Option<[f64; 4]>) -> bool {
    value.is_none_or(|[x, y, width, height]| {
        // Normalization and JSON persistence can move an edge a few ulps past
        // one. Preserve the stored geometry/identity; only tolerate roundoff,
        // never clamp a genuinely out-of-bounds box into a valid one.
        const EDGE_LIMIT: f64 = 1.0 + 8.0 * f64::EPSILON;
        [x, y, width, height].iter().all(|part| part.is_finite())
            && (0.0..=1.0).contains(&x)
            && (0.0..=1.0).contains(&y)
            && width > 0.0
            && height > 0.0
            && x + width <= EDGE_LIMIT
            && y + height <= EDGE_LIMIT
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_roundoff_survives_json_but_invalid_geometry_is_rejected() {
        let edge = [0.9499269104003908, 0.5, 0.05007308959960938, 0.5];
        assert!(edge[0] + edge[2] > 1.0);
        for mut region in [edge, [edge[1], edge[0], edge[3], edge[2]]] {
            for _ in 0..8 {
                region = serde_json::from_str(&serde_json::to_string(&region).unwrap()).unwrap();
                assert!(valid_box(Some(region)));
            }
        }
        assert!(valid_box(None));
        assert!(valid_box(Some([0.0, 0.0, 1.0, 1.0])));
        for invalid in [
            [-f64::EPSILON, 0.0, 0.5, 0.5],
            [0.0, -f64::EPSILON, 0.5, 0.5],
            [0.5, 0.0, 0.5 + 1e-12, 0.5],
            [0.0, 0.5, 0.5, 0.5 + 1e-12],
            [0.0, 0.0, 0.0, 0.5],
            [0.0, 0.0, 0.5, -0.1],
            [f64::NAN, 0.0, 0.5, 0.5],
            [0.0, 0.0, f64::INFINITY, 0.5],
        ] {
            assert!(!valid_box(Some(invalid)), "accepted {invalid:?}");
        }
    }
}
