//! Frame-local radar transform; trigonometry and scale are shared by all dots.
pub struct Projection {
    sine: f32,
    cosine: f32,
    range_squared: f32,
    scale: f32,
}
impl Projection {
    pub fn new(yaw: f32, range: f32, half: f32) -> Option<Self> {
        if ![yaw, range, half].iter().all(|v| v.is_finite()) || range <= 0. {
            return None;
        }
        let (sine, cosine) = yaw.to_radians().sin_cos();
        Some(Self {
            sine,
            cosine,
            range_squared: range * range,
            scale: half / range,
        })
    }
    pub fn point(&self, delta: [f32; 2]) -> Option<[f32; 2]> {
        if !delta.iter().all(|v| v.is_finite())
            || delta[0] * delta[0] + delta[1] * delta[1] > self.range_squared
        {
            return None;
        }
        Some([
            (-delta[0] * self.cosine - delta[1] * self.sine) * self.scale,
            (delta[0] * self.sine - delta[1] * self.cosine) * self.scale,
        ])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_transform_preserves_rotation_range_and_invalid_input_rejection() {
        for yaw in [-360., -90., 0., 37., 90., 360.] {
            let p = Projection::new(yaw, 48., 76.).unwrap();
            let (s, c) = yaw.to_radians().sin_cos();
            for x in -50..=50 {
                for z in -50..=50 {
                    let (x, z) = (x as f32, z as f32);
                    let expected = (x * x + z * z <= 48. * 48.)
                        .then(|| [(-x * c - z * s) * 76. / 48., (x * s - z * c) * 76. / 48.]);
                    match (p.point([x, z]), expected) {
                        (Some(a), Some(b)) => {
                            assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.0001))
                        }
                        (None, None) => (),
                        other => panic!("{other:?}"),
                    }
                }
            }
            assert!(p.point([f32::NAN, 0.]).is_none());
            assert!(p.point([0., f32::INFINITY]).is_none());
        }
        assert!(Projection::new(f32::NAN, 48., 76.).is_none());
        assert!(Projection::new(0., 0., 76.).is_none());
        assert!(Projection::new(0., -1., 76.).is_none());
        assert!(Projection::new(0., 48., f32::INFINITY).is_none());
    }
}
