//! Resumable sphere traversal, with no per-block division or allocation.

pub(super) struct Scan {
    pub center: [i32; 3],
    pub radius: i32,
    pub cursor: usize,
    next_y: i32,
    next_z: i32,
    offset: [i32; 3],
    end_x: i32,
}

impl Scan {
    pub fn new(center: [i32; 3], radius: i32) -> Self {
        Self {
            center,
            radius,
            cursor: 0,
            next_y: -radius,
            next_z: -radius,
            offset: [1, 0, 0],
            end_x: 0,
        }
    }

    pub fn next(&mut self) -> Option<[i32; 3]> {
        if self.offset[0] > self.end_x {
            loop {
                if self.next_z > self.radius {
                    return None;
                }
                let (y, z) = (self.next_y, self.next_z);
                self.next_y += 1;
                if self.next_y > self.radius {
                    self.next_y = -self.radius;
                    self.next_z += 1;
                }
                let remaining = self.radius * self.radius - y * y - z * z;
                if remaining >= 0 {
                    // These small integers (radius <= 64) are exact in f64.
                    let extent = (remaining as f64).sqrt() as i32;
                    self.offset = [-extent, y, z];
                    self.end_x = extent;
                    break;
                }
            }
        }
        let position = std::array::from_fn(|k| self.center[k] + self.offset[k]);
        self.offset[0] += 1;
        self.cursor += 1;
        Some(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_cover_exactly_the_sphere_in_the_original_order() {
        for radius in [0, 1, 4, 24, 64] {
            let center = [-17, -64, -1];
            let mut scan = Scan::new(center, radius);
            let mut count = 0;
            for z in -radius..=radius {
                for y in -radius..=radius {
                    for x in -radius..=radius {
                        if x * x + y * y + z * z <= radius * radius {
                            assert_eq!(
                                scan.next(),
                                Some([center[0] + x, center[1] + y, center[2] + z])
                            );
                            count += 1;
                        }
                    }
                }
            }
            assert_eq!(scan.cursor, count);
            assert_eq!(scan.next(), None);
            assert_eq!(scan.next(), None);
        }
    }
}
