//! Stable bounded selection for BlockESP tracers. No heap allocation or full sort.
pub type Match<T> = (T, f32, [f32; 3]);
pub struct Nearest<T: Copy, const N: usize> {
    entries: [Option<Match<T>>; N],
    len: usize,
    limit: usize,
}
impl<T: Copy, const N: usize> Nearest<T, N> {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: [None; N],
            len: 0,
            limit: limit.min(N),
        }
    }
    pub fn push(&mut self, entry: Match<T>) {
        if self.limit == 0 {
            return;
        }
        if self.len == self.limit
            && !entry
                .1
                .total_cmp(&self.entries[self.len - 1].unwrap().1)
                .is_lt()
        {
            return;
        }
        let mut at = self.len.min(self.limit - 1);
        while at > 0 && entry.1.total_cmp(&self.entries[at - 1].unwrap().1).is_lt() {
            self.entries[at] = self.entries[at - 1];
            at -= 1;
        }
        self.entries[at] = Some(entry);
        self.len = (self.len + 1).min(self.limit);
    }
    pub fn iter(&self) -> impl Iterator<Item = &Match<T>> {
        self.entries[..self.len].iter().map(|v| v.as_ref().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_selection_matches_full_stable_sort_including_ties_and_float_edges() {
        let mut seed = 123456u32;
        for count in [0, 1, 12, 100, 4096] {
            let input: Vec<_> = (0..count)
                .map(|id| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    let d = match id % 23 {
                        0 => f32::NAN,
                        1 => -0.,
                        2 => 0.,
                        3 => f32::INFINITY,
                        _ => (seed % 100) as f32,
                    };
                    (id, d, [0.; 3])
                })
                .collect();
            let mut expected = input.clone();
            expected.sort_by(|a, b| a.1.total_cmp(&b.1));
            for limit in [0, 1, 5, 12, 100] {
                let mut nearest = Nearest::<_, 12>::new(limit);
                for v in &input {
                    nearest.push(*v);
                }
                let actual: Vec<_> = nearest.iter().map(|v| v.0).collect();
                let expected: Vec<_> = expected.iter().take(limit.min(12)).map(|v| v.0).collect();
                assert_eq!(actual, expected);
            }
        }
    }
}
