//! Human-like cursor movement: a curved, eased path with a short settle at the end.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathPoint {
    pub x: i32,
    pub y: i32,
    /// Pause before moving to this point.
    pub dt_ms: u32,
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Points from `from` to `to`; the last point is exactly `to`. Deterministic.
pub fn cursor_path(from: (i32, i32), to: (i32, i32)) -> Vec<PathPoint> {
    let (fx, fy) = (f64::from(from.0), f64::from(from.1));
    let (tx, ty) = (f64::from(to.0), f64::from(to.1));
    let (dx, dy) = (tx - fx, ty - fy);
    let dist = (dx * dx + dy * dy).sqrt();
    if !dist.is_finite() || dist < 2.0 {
        return vec![PathPoint {
            x: to.0,
            y: to.1,
            dt_ms: 20,
        }];
    }
    let n = ((dist / 10.0) as usize).clamp(6, 90);
    let total = (250.0 + dist * 0.5).clamp(250.0, 1100.0);
    let dt = ((total / n as f64) as u32).max(1);
    let sign = if (from.0 ^ from.1 ^ to.0 ^ to.1) & 1 == 0 {
        1.0
    } else {
        -1.0
    };
    let off = dist * 0.12 * sign;
    let (px, py) = (-dy / dist, dx / dist);
    let c1 = (fx + dx * 0.3 + px * off, fy + dy * 0.3 + py * off);
    let c2 = (
        fx + dx * 0.7 + px * off * 0.5,
        fy + dy * 0.7 + py * off * 0.5,
    );
    let mut out = Vec::with_capacity(n + 1);
    for i in 1..=n {
        let t = smooth(i as f64 / n as f64);
        let u = 1.0 - t;
        let x = u * u * u * fx + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * tx;
        let y = u * u * u * fy + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * ty;
        out.push(PathPoint {
            x: x.round().clamp(-1.0e9, 1.0e9) as i32,
            y: y.round().clamp(-1.0e9, 1.0e9) as i32,
            dt_ms: dt,
        });
    }
    if let Some(l) = out.last_mut() {
        l.x = to.0;
        l.y = to.1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_exactly_on_target_and_is_deterministic() {
        let a = cursor_path((10, 10), (800, 400));
        assert_eq!(a, cursor_path((10, 10), (800, 400)));
        let l = a.last().unwrap();
        assert_eq!((l.x, l.y), (800, 400));
        assert!(a.len() >= 6 && a.len() <= 90);
        let total: u32 = a.iter().map(|p| p.dt_ms).sum();
        assert!((200..=1300).contains(&total), "{total}");
    }

    #[test]
    fn short_and_extreme_moves_are_safe() {
        assert_eq!(cursor_path((5, 5), (5, 6)).len(), 1);
        let far = cursor_path((i32::MIN, i32::MIN), (i32::MAX, i32::MAX));
        assert!(far.len() <= 90);
        let l = far.last().unwrap();
        assert_eq!((l.x, l.y), (i32::MAX, i32::MAX));
    }

    #[test]
    fn path_curves_off_the_straight_line() {
        let a = cursor_path((0, 0), (1000, 0));
        assert!(a.iter().any(|p| p.y.abs() > 20));
    }
}
