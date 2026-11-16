//! The little vector maths the flight needs, on `[f64; 3]`.

pub type V3 = [f64; 3];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

pub fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

/// `a` with length 1; the zero vector stays zero.
pub fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l < 1.0e-12 {
        [0.0; 3]
    } else {
        scale(a, 1.0 / l)
    }
}

/// The direction a player looks, from the yaw and pitch in degrees (`Entity.calculateViewVector`).
pub fn view(yaw: f32, pitch: f32) -> V3 {
    let (yaw, pitch) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    ]
}

/// Where the segment from `from` along the unit vector `dir`, `length` long, first meets the box
/// `[min, max]`: the distance along it, 0 when it starts inside. None when it misses.
pub fn ray_box(from: V3, dir: V3, length: f64, min: V3, max: V3) -> Option<f64> {
    let (mut near, mut far) = (0.0_f64, length);
    for i in 0..3 {
        if dir[i].abs() < 1.0e-12 {
            if from[i] < min[i] || from[i] > max[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((min[i] - from[i]) / dir[i], (max[i] - from[i]) / dir[i]);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        near = near.max(a);
        far = far.min(b);
        if near > far {
            return None;
        }
    }
    Some(near)
}

pub fn inside(p: V3, min: V3, max: V3) -> bool {
    (0..3).all(|i| p[i] >= min[i] && p[i] <= max[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_facing_south_looks_along_plus_z_and_down_is_minus_y() {
        // Yaw 0 faces +z in Minecraft, and a positive pitch looks down.
        let south = view(0.0, 0.0);
        assert!((south[2] - 1.0).abs() < 1e-9 && south[0].abs() < 1e-9);
        let west = view(90.0, 0.0);
        assert!((west[0] + 1.0).abs() < 1e-9);
        let down = view(0.0, 90.0);
        assert!((down[1] + 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_segment_meets_a_box_where_it_enters_it() {
        let (min, max) = ([2.0, -1.0, -1.0], [4.0, 1.0, 1.0]);
        assert_eq!(
            ray_box([0.0; 3], [1.0, 0.0, 0.0], 10.0, min, max),
            Some(2.0)
        );
        // Too short to reach it, pointing away, or passing beside it.
        assert_eq!(ray_box([0.0; 3], [1.0, 0.0, 0.0], 1.5, min, max), None);
        assert_eq!(ray_box([0.0; 3], [-1.0, 0.0, 0.0], 10.0, min, max), None);
        assert_eq!(
            ray_box([0.0, 5.0, 0.0], [1.0, 0.0, 0.0], 10.0, min, max),
            None
        );
        // Starting inside is a hit at once.
        assert_eq!(
            ray_box([3.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.1, min, max),
            Some(0.0)
        );
    }

    #[test]
    fn the_zero_vector_normalizes_to_itself() {
        assert_eq!(normalize([0.0; 3]), [0.0; 3]);
        assert!((len(normalize([3.0, 4.0, 0.0])) - 1.0).abs() < 1e-12);
    }
}
