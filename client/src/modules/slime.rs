//! Slime chunks. In Bedrock Edition they do not depend on the world seed:
//! a chunk is a slime chunk when the first output of MT19937 seeded with
//! `(chunk_x * 0x1f1f1f1f) ^ chunk_z` (as 32-bit unsigned) is a multiple of
//! 10. Slimes spawn there below y = 40 in the Overworld. Pure arithmetic:
//! nothing is read from the game.

/// Whether the chunk (chunk coordinates: block >> 4) is a slime chunk.
pub fn is_slime_chunk(chunk_x: i32, chunk_z: i32) -> bool {
    let seed = (chunk_x as u32).wrapping_mul(0x1f1f_1f1f) ^ chunk_z as u32;
    first_mt19937(seed).is_multiple_of(10)
}

/// Whether the block column (x, z) lies in a slime chunk.
pub fn at_block(x: i32, z: i32) -> bool {
    is_slime_chunk(x.div_euclid(16), z.div_euclid(16))
}

/// The first output of MT19937 seeded with `seed`. The first output needs
/// only state words 0, 1 and 397 after seeding, so the other 226 of the 624
/// words are never computed.
fn first_mt19937(seed: u32) -> u32 {
    const M: usize = 397;
    let mut state = [0u32; M + 1];
    state[0] = seed;
    for i in 1..=M {
        let previous = state[i - 1];
        state[i] = 1_812_433_253u32
            .wrapping_mul(previous ^ (previous >> 30))
            .wrapping_add(i as u32);
    }
    let y = (state[0] & 0x8000_0000) | (state[1] & 0x7fff_ffff);
    let mut x = state[M] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
    x ^= x >> 11;
    x ^= (x << 7) & 0x9d2c_5680;
    x ^= (x << 15) & 0xefc6_0000;
    x ^ (x >> 18)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full MT19937, as in the reference implementation, to check the
    /// shortened first output against.
    fn reference(seed: u32, count: usize) -> Vec<u32> {
        let mut mt = [0u32; 624];
        mt[0] = seed;
        for i in 1..624 {
            mt[i] = 1_812_433_253u32
                .wrapping_mul(mt[i - 1] ^ (mt[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        let mut out = Vec::new();
        let mut index = 624;
        while out.len() < count {
            if index >= 624 {
                for i in 0..624 {
                    let y = (mt[i] & 0x8000_0000) | (mt[(i + 1) % 624] & 0x7fff_ffff);
                    mt[i] =
                        mt[(i + 397) % 624] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
                }
                index = 0;
            }
            let mut y = mt[index];
            y ^= y >> 11;
            y ^= (y << 7) & 0x9d2c_5680;
            y ^= (y << 15) & 0xefc6_0000;
            y ^= y >> 18;
            out.push(y);
            index += 1;
        }
        out
    }

    #[test]
    fn mersenne_twister_matches_the_reference_outputs() {
        // The reference implementation's default seed: first outputs.
        assert_eq!(reference(5489, 2), [3_499_211_612, 581_869_302]);
        assert_eq!(first_mt19937(5489), 3_499_211_612);
        let mut seed = 1u32;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
            assert_eq!(first_mt19937(seed), reference(seed, 1)[0], "{seed}");
        }
    }

    #[test]
    fn about_one_chunk_in_ten_is_a_slime_chunk_and_blocks_map_to_chunks() {
        let slime = (-100..100)
            .flat_map(|x| (-100..100).map(move |z| (x, z)))
            .filter(|&(x, z)| is_slime_chunk(x, z))
            .count();
        // 40000 chunks: 10% expected.
        assert!((3600..4400).contains(&slime), "{slime}");
        for (x, z) in [(0, 0), (-1, -1), (15, 16), (-17, 33)] {
            assert_eq!(at_block(x, z), is_slime_chunk(x >> 4, z >> 4));
        }
    }
}
