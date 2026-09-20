//! Detector-neutral face boxes and non-maximum suppression.

/// One detection in detector-input pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectedFace {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

pub(crate) fn non_maximum_suppression(
    mut faces: Vec<DetectedFace>,
    threshold: f32,
    top_k: usize,
) -> Vec<DetectedFace> {
    faces.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let limit = top_k.max(1).min(faces.len());
    let mut suppressed = vec![false; limit];
    let mut kept = Vec::new();
    for index in 0..limit {
        if suppressed[index] {
            continue;
        }
        kept.push(faces[index]);
        for candidate in index + 1..limit {
            if !suppressed[candidate] && box_iou(&faces[index], &faces[candidate]) > threshold {
                suppressed[candidate] = true;
            }
        }
    }
    kept
}

fn box_iou(left: &DetectedFace, right: &DetectedFace) -> f32 {
    let x1 = left.x.max(right.x);
    let y1 = left.y.max(right.y);
    let x2 = (left.x + left.width).min(right.x + right.width);
    let y2 = (left.y + left.height).min(right.y + right.height);
    let intersection = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = left.width * left.height + right.width * right.height - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, size: f32, score: f32) -> DetectedFace {
        DetectedFace {
            x,
            y,
            width: size,
            height: size,
            landmarks: [[0.0; 2]; 5],
            score,
        }
    }

    #[test]
    fn nms_keeps_score_order_and_suppresses_overlap() {
        let kept = non_maximum_suppression(
            vec![
                face(0.0, 0.0, 10.0, 0.7),
                face(1.0, 1.0, 10.0, 0.95),
                face(100.0, 100.0, 10.0, 0.8),
            ],
            0.3,
            64,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.95);
        assert_eq!(kept[1].score, 0.8);
    }

    #[test]
    fn nms_applies_top_k_before_suppression() {
        let kept = non_maximum_suppression(
            vec![
                face(0.0, 0.0, 10.0, 0.2),
                face(50.0, 50.0, 10.0, 0.5),
                face(100.0, 100.0, 10.0, 0.9),
            ],
            0.3,
            1,
        );
        assert_eq!(kept, vec![face(100.0, 100.0, 10.0, 0.9)]);
    }
}
