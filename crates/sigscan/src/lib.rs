//! Byte-pattern ("signature") scanning and minimal PE section parsing.
//!
//! Shared by the injected client (scans the live image) and the injector's
//! offline `scan` command (scans the exe on disk).

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    Empty,
    AllWildcards,
    BadToken(String),
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "pattern is empty"),
            Self::AllWildcards => write!(f, "pattern has no concrete byte"),
            Self::BadToken(t) => write!(f, "invalid token `{t}` (expected 2 hex digits or ??)"),
        }
    }
}

impl std::error::Error for PatternError {}

fn parse_byte(tok: &str) -> Result<u8, PatternError> {
    if tok.len() != 2 {
        return Err(PatternError::BadToken(tok.to_string()));
    }
    u8::from_str_radix(tok, 16).map_err(|_| PatternError::BadToken(tok.to_string()))
}

/// Parses `"90 90 EB"` into raw bytes (no wildcards allowed).
pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, PatternError> {
    let bytes = s
        .split_whitespace()
        .map(parse_byte)
        .collect::<Result<Vec<_>, _>>()?;
    if bytes.is_empty() {
        return Err(PatternError::Empty);
    }
    Ok(bytes)
}

/// An IDA-style signature such as `"48 8B 05 ?? ?? ?? ?? F3 0F"`.
#[derive(Debug, Clone)]
pub struct Pattern {
    bytes: Vec<Option<u8>>,
    /// A longest literal run (capped at 255 bytes), searched with SIMD
    /// (`memmem`); all bytes outside it are checked at each occurrence.
    anchor_start: usize,
    anchor: memchr::memmem::Finder<'static>,
}

/// Byte frequencies of x86-64 code for `memmem`'s choice of the two anchor
/// bytes it scans for (the rarer, the fewer false candidates). At most
/// 225, below the rank above which `memmem` gives up its SIMD prefilter.
struct CodeRank;

impl memchr::arch::all::packedpair::HeuristicFrequencyRank for CodeRank {
    fn rank(&self, byte: u8) -> u8 {
        commonness(byte) as u8 * 25
    }
}

impl FromStr for Pattern {
    type Err = PatternError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = Vec::new();
        for tok in s.split_whitespace() {
            if tok == "?" || tok == "??" {
                bytes.push(None);
            } else {
                bytes.push(Some(parse_byte(tok)?));
            }
        }
        if bytes.is_empty() {
            return Err(PatternError::Empty);
        }
        let (mut anchor_start, mut anchor_len, mut run_start, mut run_len) = (0, 0, 0, 0);
        for (i, byte) in bytes.iter().enumerate() {
            if byte.is_some() {
                if run_len == 0 {
                    run_start = i;
                }
                run_len += 1;
                if run_len.min(255) > anchor_len {
                    anchor_start = run_start;
                    anchor_len = run_len.min(255);
                }
            } else {
                run_len = 0;
            }
        }
        if anchor_len == 0 {
            return Err(PatternError::AllWildcards);
        }
        let anchor: Vec<u8> = bytes[anchor_start..anchor_start + anchor_len]
            .iter()
            .map(|b| b.unwrap())
            .collect();
        let anchor =
            memchr::memmem::FinderBuilder::new().build_forward_with_ranker_owned(CodeRank, anchor);
        Ok(Self {
            bytes,
            anchor_start,
            anchor,
        })
    }
}

impl Pattern {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    fn matches_at(&self, hay: &[u8], at: usize) -> bool {
        let end = self.anchor_start + self.anchor.needle().len();
        self.bytes[..self.anchor_start]
            .iter()
            .zip(&hay[at..at + self.anchor_start])
            .chain(
                self.bytes[end..]
                    .iter()
                    .zip(&hay[at + end..at + self.bytes.len()]),
            )
            .all(|(p, &b)| p.is_none_or(|p| p == b))
    }

    /// Calls `hit` in ascending order; false stops without allocating a result.
    fn search(&self, hay: &[u8], mut hit: impl FnMut(usize) -> bool) {
        if hay.len() < self.bytes.len() {
            return;
        }
        // Anchor occurrences that leave room for the whole pattern: the
        // pattern starting at `at` has its anchor at `at + anchor_start`.
        let anchor_len = self.anchor.needle().len();
        let region = &hay
            [self.anchor_start..hay.len() - (self.bytes.len() - self.anchor_start - anchor_len)];
        let mut from = 0;
        // Restarting one byte past each occurrence finds overlapping ones too.
        while let Some(found) = self.anchor.find(&region[from..]) {
            let at = from + found;
            if self.matches_at(hay, at) && !hit(at) {
                return;
            }
            from = at + 1;
        }
    }

    /// Returns the offsets of up to `limit` matches in `hay`.
    pub fn find_all(&self, hay: &[u8], limit: usize) -> Vec<usize> {
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        self.search(hay, |at| {
            out.push(at);
            out.len() < limit
        });
        out
    }

    pub fn find(&self, hay: &[u8]) -> Option<usize> {
        let mut first = None;
        self.search(hay, |at| {
            first = Some(at);
            false
        });
        first
    }

    /// Whether `hay` begins with a match of the pattern (it may be longer).
    pub fn matches(&self, hay: &[u8]) -> bool {
        hay.len() >= self.bytes.len() && self.matches_whole(hay, 0)
    }

    /// Whether every match of `other` is also a match of this pattern at the
    /// same offset: `other` is at least as long and fixes every byte this
    /// pattern fixes, to the same value. Its matches can then be found by
    /// checking only this pattern's matches.
    pub fn covers(&self, other: &Pattern) -> bool {
        other.bytes.len() >= self.bytes.len()
            && self
                .bytes
                .iter()
                .zip(&other.bytes)
                .all(|(mine, theirs)| mine.is_none() || mine == theirs)
    }

    /// Whether the whole pattern matches at `at` (the caller guarantees
    /// `at + len() <= hay.len()`).
    fn matches_whole(&self, hay: &[u8], at: usize) -> bool {
        self.bytes
            .iter()
            .zip(&hay[at..at + self.bytes.len()])
            .all(|(p, &b)| p.is_none_or(|p| p == b))
    }

    /// Every 4-byte literal window of the pattern, as (offset, key).
    fn windows(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        (0..self.bytes.len().saturating_sub(3)).filter_map(|at| {
            let window = &self.bytes[at..at + 4];
            window.iter().all(Option::is_some).then(|| {
                (
                    at,
                    u32::from_le_bytes(std::array::from_fn(|k| window[k].unwrap_or(0))),
                )
            })
        })
    }

    /// A 4-byte literal window to index the pattern by, as (offset, key), or
    /// None when no four consecutive bytes are literal. Without knowledge of
    /// the code being searched, the window made of the least common
    /// machine-code bytes is taken (see [`PatternSet::with_sample`] for a
    /// choice measured on the code itself).
    pub fn key(&self) -> Option<(usize, u32)> {
        self.windows()
            .min_by_key(|&(at, key)| (static_cost(key), at))
    }
}

/// How common a byte is in x86-64 code, on a rough 0-9 scale: opcodes, REX
/// prefixes, register pushes of function prologues, ModRM/SIB values of the
/// usual register moves, padding and small immediates. Only steers which
/// literal window keys a pattern; any choice gives the same matches.
fn commonness(byte: u8) -> u32 {
    match byte {
        0x00 | 0x48 | 0xFF | 0xCC => 9,
        0x8B | 0x89 | 0x24 | 0x0F | 0x01 | 0x4C | 0x8D | 0x44 | 0x83 | 0xE8 | 0x41 | 0x53
        | 0x55 | 0x56 | 0x57 => 7,
        0x08 | 0x10 | 0x20 | 0x28 | 0x30 | 0x38 | 0x40 | 0x49 | 0x85 | 0xC0 | 0xC3 | 0x74
        | 0x75 | 0xEB | 0x02 | 0x04 | 0x05 | 0x18 | 0x90 | 0xC7 | 0xF3 | 0x15 | 0x54 | 0xEC
        | 0xC4 | 0x5E | 0x5F | 0x5B | 0x5D | 0xF1 | 0xCE | 0xD9 | 0xC1 => 5,
        _ => 1,
    }
}

fn static_cost(key: u32) -> u32 {
    key.to_le_bytes().map(commonness).iter().sum()
}

/// Several patterns searched in a single pass over the haystack.
///
/// Each pattern is indexed by one 4-byte literal window; the haystack is read
/// once, and the full pattern is only compared where that window's bytes
/// occur. Patterns without four consecutive literal bytes are searched on
/// their own. Results are exactly those of [`Pattern::find_all`] for each
/// pattern, in the same (ascending) order.
#[derive(Debug, Clone)]
pub struct PatternSet {
    patterns: Vec<Pattern>,
    /// Index entries grouped by `bucket(key)`: (key, pattern, window offset).
    entries: Vec<(u32, u32, u32)>,
    /// `entries[starts[b]..starts[b + 1]]` belong to bucket `b`.
    starts: Vec<u32>,
    /// One bit per bucket that has any entry, to skip most positions with a
    /// single load.
    occupied: Box<Occupied>,
    /// Patterns searched individually.
    fallback: Vec<usize>,
}

const BUCKET_BITS: u32 = 16;
const OCCUPIED_WORDS: usize = (1 << BUCKET_BITS) / 64;
/// One bit per bucket.
type Occupied = [u64; OCCUPIED_WORDS];
/// Candidate positions collected before they are verified.
const CANDIDATES: usize = 256;

/// The least haystack [`PatternSet::find_all_parallel`] gives each thread;
/// below this a thread costs more than it saves.
pub const MIN_CHUNK: usize = 4 << 20;

fn bucket(key: u32) -> usize {
    (key.wrapping_mul(0x9E37_79B1) >> (32 - BUCKET_BITS)) as usize
}

impl PatternSet {
    pub fn new(patterns: Vec<Pattern>) -> Self {
        let keys = patterns.iter().map(Pattern::key).collect();
        Self::build(patterns, keys)
    }

    /// Like [`PatternSet::new`], but each pattern is keyed by its literal
    /// window that occurs least often in `sample`, a part of the code that
    /// will be searched. Fewer key occurrences mean fewer full comparisons;
    /// the matches found are the same either way.
    pub fn with_sample(patterns: Vec<Pattern>, sample: &[u8]) -> Self {
        let mut candidates: Vec<u32> = patterns
            .iter()
            .flat_map(|p| p.windows().map(|(_, key)| key))
            .collect();
        // Grouped by bucket, like the index: a key is found by scanning its
        // bucket's few entries (a binary search per occurrence was most of
        // the cost on code, where common windows occur very often).
        candidates.sort_unstable_by_key(|&key| (bucket(key), key));
        candidates.dedup();
        let mut starts = vec![0u32; (1 << BUCKET_BITS) + 1];
        let mut filter = Box::new([0u64; OCCUPIED_WORDS]);
        for &key in &candidates {
            let b = bucket(key);
            starts[b + 1] += 1;
            filter[b / 64] |= 1 << (b % 64);
        }
        for b in 0..1 << BUCKET_BITS {
            starts[b + 1] += starts[b];
        }
        let find = |key: u32| {
            let b = bucket(key);
            (starts[b] as usize..starts[b + 1] as usize).find(|&i| candidates[i] == key)
        };
        let mut counts = vec![0u32; candidates.len()];
        let mut found = [0usize; CANDIDATES];
        let mut pos = 0;
        while pos + 4 <= sample.len() {
            let (n, next) = self::candidates(sample, pos, &filter, &mut found);
            for &at in &found[..n] {
                if let Some(i) = find(u32::from_le_bytes(sample[at..at + 4].try_into().unwrap())) {
                    counts[i] = counts[i].saturating_add(1);
                }
            }
            pos = next;
        }
        let count = |key: u32| find(key).map_or(0, |i| counts[i]);
        let keys = patterns
            .iter()
            .map(|p| {
                p.windows()
                    .min_by_key(|&(at, key)| (count(key), static_cost(key), at))
            })
            .collect();
        Self::build(patterns, keys)
    }

    fn build(patterns: Vec<Pattern>, keys: Vec<Option<(usize, u32)>>) -> Self {
        let mut keyed = Vec::new();
        let mut fallback = Vec::new();
        for (index, key) in keys.into_iter().enumerate() {
            match key {
                Some((offset, key)) if index <= u32::MAX as usize => {
                    keyed.push((key, index as u32, offset as u32));
                }
                _ => fallback.push(index),
            }
        }
        let buckets = 1usize << BUCKET_BITS;
        keyed.sort_by_key(|&(key, index, _)| (bucket(key), index));
        let mut starts = vec![0u32; buckets + 1];
        for &(key, _, _) in &keyed {
            starts[bucket(key) + 1] += 1;
        }
        for b in 0..buckets {
            starts[b + 1] += starts[b];
        }
        let mut occupied = Box::new([0u64; OCCUPIED_WORDS]);
        for &(key, _, _) in &keyed {
            let b = bucket(key);
            occupied[b / 64] |= 1 << (b % 64);
        }
        Self {
            patterns,
            entries: keyed,
            starts,
            occupied,
            fallback,
        }
    }

    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn patterns(&self) -> &[Pattern] {
        &self.patterns
    }

    /// The index key chosen for each pattern, as (pattern, window offset,
    /// key); patterns without one are searched on their own. For diagnostics.
    pub fn keys(&self) -> impl Iterator<Item = (usize, usize, u32)> + '_ {
        self.entries
            .iter()
            .map(|&(key, index, offset)| (index as usize, offset as usize, key))
    }

    /// For each pattern (in the order given to [`PatternSet::new`]), the
    /// offsets of up to `limit` matches in `hay`, ascending.
    pub fn find_all(&self, hay: &[u8], limit: usize) -> Vec<Vec<usize>> {
        let mut out = vec![Vec::new(); self.patterns.len()];
        if limit == 0 {
            return out;
        }
        for &index in &self.fallback {
            out[index] = self.patterns[index].find_all(hay, limit);
        }
        let mut open = self.entries.len();
        if open == 0 || hay.len() < 4 {
            return out;
        }
        // Two phases: a loop without calls collects the positions whose key's
        // bucket is occupied (few: most code bytes are skipped), then they
        // are verified. Verifying inside the loop made it about 1.6 times
        // slower, though only one position in about 1,400 is a candidate.
        let mut found = [0usize; CANDIDATES];
        let end = hay.len() - 3;
        let mut pos = 0;
        while pos < end {
            let (n, next) = candidates(hay, pos, &self.occupied, &mut found);
            for &at in &found[..n] {
                let key = u32::from_le_bytes(hay[at..at + 4].try_into().unwrap());
                if self.verify(hay, at, key, limit, &mut out, &mut open) {
                    return out;
                }
            }
            pos = next;
        }
        out
    }

    /// [`PatternSet::find_all`] with the haystack divided among up to
    /// `threads` threads (none for less than [`MIN_CHUNK`] per thread). The
    /// results are identical: each thread reports the matches that start in
    /// its part, and the parts are joined in order and cut to `limit`.
    pub fn find_all_parallel(&self, hay: &[u8], limit: usize, threads: usize) -> Vec<Vec<usize>> {
        let parts = threads.min(hay.len() / MIN_CHUNK).max(1);
        self.find_all_split(hay, limit, parts)
    }

    /// `find_all` over `parts` consecutive parts of `hay`, each on its own
    /// thread (on the calling thread for one part, or if a thread cannot be
    /// created).
    fn find_all_split(&self, hay: &[u8], limit: usize, parts: usize) -> Vec<Vec<usize>> {
        let parts = parts.clamp(1, hay.len().max(1));
        let longest = self.patterns.iter().map(Pattern::len).max().unwrap_or(0);
        if parts == 1 || limit == 0 || longest == 0 {
            return self.find_all(hay, limit);
        }
        let size = hay.len().div_ceil(parts);
        // Matches starting in `start..end`, in `hay` coordinates. A match that
        // starts before `end` ends within `longest - 1` bytes past it, and it
        // is found before any match starting later, so the first `limit`
        // matches of the slice include every one that belongs to this part.
        let part = |start: usize| {
            let end = (start + size).min(hay.len());
            let slice = &hay[start..(end + longest - 1).min(hay.len())];
            let mut found = self.find_all(slice, limit);
            for offsets in &mut found {
                offsets.retain(|&at| at < end - start);
                offsets.iter_mut().for_each(|at| *at += start);
            }
            found
        };
        let results: Vec<Vec<Vec<usize>>> = std::thread::scope(|scope| {
            let part = &part;
            let pending: Vec<_> = (0..parts)
                .map(|i| i * size)
                .filter(|&start| start < hay.len())
                .map(|start| {
                    std::thread::Builder::new()
                        .spawn_scoped(scope, move || part(start))
                        .map_err(|_| start)
                })
                .collect();
            pending
                .into_iter()
                .map(|handle| match handle {
                    Ok(handle) => handle
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
                    Err(start) => part(start),
                })
                .collect()
        });
        let mut out = vec![Vec::new(); self.patterns.len()];
        for found in results {
            for (all, offsets) in out.iter_mut().zip(found) {
                let room = limit - all.len().min(limit);
                all.extend(offsets.into_iter().take(room));
            }
        }
        out
    }

    /// Checks the patterns keyed by `key` at `pos`; records matches. Returns
    /// true once every keyed pattern has reached `limit` (nothing left to find).
    #[cold]
    fn verify(
        &self,
        hay: &[u8],
        pos: usize,
        key: u32,
        limit: usize,
        out: &mut [Vec<usize>],
        open: &mut usize,
    ) -> bool {
        let b = bucket(key);
        let range = self.starts[b] as usize..self.starts[b + 1] as usize;
        for &(entry_key, index, offset) in &self.entries[range] {
            if entry_key != key {
                continue;
            }
            let (index, offset) = (index as usize, offset as usize);
            let found = &mut out[index];
            if found.len() >= limit || pos < offset {
                continue;
            }
            let at = pos - offset;
            let pattern = &self.patterns[index];
            if at + pattern.len() <= hay.len() && pattern.matches_whole(hay, at) {
                found.push(at);
                if found.len() == limit {
                    *open -= 1;
                    if *open == 0 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[inline(always)]
fn occupied(bits: &Occupied, key: u32) -> bool {
    let b = bucket(key);
    bits[b / 64] & (1 << (b % 64)) != 0
}

/// The positions from `start` (up to `hay.len() - 4`) whose key's bucket is
/// occupied, ascending, as many as fit in `out`. Returns their number and
/// the position to continue from.
fn candidates(
    hay: &[u8],
    start: usize,
    bits: &Occupied,
    out: &mut [usize; CANDIDATES],
) -> (usize, usize) {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: the CPU supports AVX2.
        return unsafe { candidates_avx2(hay, start, bits, out) };
    }
    candidates_scalar(hay, start, bits, out, 0)
}

/// [`candidates`] after `n` already in `out`.
#[inline(always)]
fn candidates_scalar(
    hay: &[u8],
    start: usize,
    bits: &Occupied,
    out: &mut [usize; CANDIDATES],
    mut n: usize,
) -> (usize, usize) {
    let mut pos = start;
    // Each 8-byte load yields the keys at five consecutive positions; room
    // for five more candidates is kept before each.
    while pos + 8 <= hay.len() && n + 5 <= CANDIDATES {
        let word = u64::from_le_bytes(hay[pos..pos + 8].try_into().unwrap());
        for k in 0..5 {
            if occupied(bits, (word >> (8 * k)) as u32) {
                out[n] = pos + k;
                n += 1;
            }
        }
        pos += 5;
    }
    while pos + 4 <= hay.len() && n < CANDIDATES {
        if occupied(
            bits,
            u32::from_le_bytes(hay[pos..pos + 4].try_into().unwrap()),
        ) {
            out[n] = pos;
            n += 1;
        }
        pos += 1;
    }
    (n, pos)
}

/// [`candidates`] 32 positions at a time: the keys are hashed eight at once
/// and their bits gathered from the bitmap. Measured on the game's 250 MB
/// of code: 1.7 times the scalar loop's speed on one thread.
///
/// # Safety
/// The CPU must support AVX2.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn candidates_avx2(
    hay: &[u8],
    start: usize,
    bits: &Occupied,
    out: &mut [usize; CANDIDATES],
) -> (usize, usize) {
    use std::arch::x86_64::*;
    let mul = _mm256_set1_epi32(0x9E37_79B1u32 as i32);
    let low5 = _mm256_set1_epi32(31);
    // Little-endian: bit b of the u64 bitmap is bit b % 32 of u32 word b / 32.
    let words = bits.as_ptr().cast::<i32>();
    let mut n = 0;
    let mut pos = start;
    // Lane j of the load at `pos + k` holds the key at `pos + 4j + k`; the
    // load at `pos + 3` reads bytes up to `pos + 35`.
    while pos + 35 <= hay.len() && n + 32 <= CANDIDATES {
        let mut hits = 0u32;
        for k in 0..4 {
            // SAFETY: pos + k + 32 <= hay.len().
            let keys = unsafe { _mm256_loadu_si256(hay.as_ptr().add(pos + k).cast()) };
            let b = _mm256_srli_epi32::<{ 32 - BUCKET_BITS as i32 }>(_mm256_mullo_epi32(keys, mul));
            // SAFETY: b < 2^BUCKET_BITS, so each word index b / 32 is below
            // 2 * OCCUPIED_WORDS, the bitmap's length in u32 words.
            let w = unsafe { _mm256_i32gather_epi32::<4>(words, _mm256_srli_epi32::<5>(b)) };
            let bit = _mm256_slli_epi32::<31>(_mm256_srlv_epi32(w, _mm256_and_si256(b, low5)));
            hits |= (_mm256_movemask_ps(_mm256_castsi256_ps(bit)) as u32) << (8 * k);
        }
        if hits != 0 {
            // Bit 8k + j is position pos + 4j + k: report in ascending order.
            for i in 0..32 {
                if hits >> (8 * (i & 3) + (i >> 2)) & 1 != 0 {
                    out[n] = pos + i;
                    n += 1;
                }
            }
        }
        pos += 32;
    }
    candidates_scalar(hay, pos, bits, out, n)
}

/// A PE section header (only the fields we need).
#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub rva: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub executable: bool,
}

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*b.get(off..)?.first_chunk()?))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*b.get(off..)?.first_chunk()?))
}

/// Parses the section table out of the PE headers (the first page or so of an
/// image, either from disk or from memory). Returns `None` if malformed.
pub fn sections(headers: &[u8]) -> Option<Vec<Section>> {
    if headers.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(headers, 0x3C)? as usize;
    if headers.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let count = u16_at(headers, pe.checked_add(6)?)? as usize;
    let opt_size = u16_at(headers, pe.checked_add(20)?)? as usize;
    let table = pe.checked_add(24)?.checked_add(opt_size)?;
    // Check the entire table before reserving memory for an untrusted count.
    headers.get(table..table.checked_add(count.checked_mul(40)?)?)?;

    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let s = table + i * 40;
        let raw_name = headers.get(s..s + 8)?;
        let end = raw_name.iter().position(|&b| b == 0).unwrap_or(8);
        out.push(Section {
            name: String::from_utf8_lossy(&raw_name[..end]).into_owned(),
            virtual_size: u32_at(headers, s + 8)?,
            rva: u32_at(headers, s + 12)?,
            raw_size: u32_at(headers, s + 16)?,
            raw_offset: u32_at(headers, s + 20)?,
            executable: u32_at(headers, s + 36)? & 0x2000_0000 != 0,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_covering_pattern_matches_wherever_the_covered_one_does() {
        // Every pattern of up to 3 tokens over {00, 01, ??}.
        let mut patterns = Vec::new();
        for length in 1..=3u32 {
            for code in 0..3usize.pow(length) {
                let tokens: Vec<_> = (0..length as usize)
                    .map(|i| ["00", "01", "??"][code / 3usize.pow(i as u32) % 3])
                    .collect();
                if let Ok(p) = tokens.join(" ").parse::<Pattern>() {
                    patterns.push(p);
                }
            }
        }
        let hays: Vec<Vec<u8>> = (0..1usize << 6)
            .map(|code| (0..6).map(|i| ((code >> i) & 1) as u8).collect())
            .collect();
        for a in &patterns {
            for b in &patterns {
                // `covers` holds exactly when no input matches b without a.
                let counterexample = hays.iter().any(|h| b.matches(h) && !a.matches(h));
                let longer_b = b.len() >= a.len();
                assert_eq!(a.covers(b), longer_b && !counterexample, "{a:?} {b:?}");
                for h in &hays {
                    assert_eq!(b.matches(h), b.find_all(&h[..b.len()], 1) == [0]);
                }
            }
        }
        assert!(!patterns[0].matches(&[]));
    }
    #[test]
    fn scan_matches_naive_reference_for_wildcards_overlaps_and_limits() {
        for length in 1..=5 {
            for code in 0..3usize.pow(length) {
                let mut v = code;
                let bytes: Vec<_> = (0..length)
                    .map(|_| {
                        let b = match v % 3 {
                            0 => Some(0),
                            1 => Some(1),
                            _ => None,
                        };
                        v /= 3;
                        b
                    })
                    .collect();
                if bytes.iter().all(Option::is_none) {
                    continue;
                }
                let text = bytes
                    .iter()
                    .map(|b| b.map_or_else(|| "??".into(), |v| format!("{v:02X}")))
                    .collect::<Vec<_>>()
                    .join(" ");
                let p: Pattern = text.parse().unwrap();
                for n in 0..=8 {
                    for code in 0..1usize << n {
                        let hay: Vec<_> = (0..n).map(|i| ((code >> i) & 1) as u8).collect();
                        let expected: Vec<_> = if hay.len() < bytes.len() {
                            Vec::new()
                        } else {
                            hay.windows(bytes.len())
                                .enumerate()
                                .filter(|(_, w)| {
                                    bytes.iter().zip(*w).all(|(p, b)| p.is_none_or(|v| v == *b))
                                })
                                .map(|(i, _)| i)
                                .collect()
                        };
                        assert_eq!(p.find(&hay), expected.first().copied(), "{text} {hay:?}");
                        for limit in [0, 1, 2, usize::MAX] {
                            assert_eq!(
                                p.find_all(&hay, limit),
                                expected.iter().take(limit).copied().collect::<Vec<_>>(),
                                "{text} {hay:?} limit={limit}"
                            );
                        }
                    }
                }
            }
        }
    }
    /// A small deterministic generator, so the cases are reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    /// Every candidate from `start`, collected through `step` as `find_all` does.
    fn all_candidates(
        hay: &[u8],
        bits: &Occupied,
        step: impl Fn(&[u8], usize, &Occupied, &mut [usize; CANDIDATES]) -> (usize, usize),
    ) -> Vec<usize> {
        let mut all = Vec::new();
        let mut found = [0; CANDIDATES];
        let mut pos = 0;
        while pos + 4 <= hay.len() {
            let (n, next) = step(hay, pos, bits, &mut found);
            assert!(next > pos || n > 0, "no progress at {pos}");
            all.extend_from_slice(&found[..n]);
            pos = next;
        }
        all
    }

    #[test]
    fn candidate_positions_are_the_same_with_and_without_simd() {
        let mut rng = Lcg(11);
        for case in 0..300 {
            // Bitmaps from nearly empty to full, so the buffer fills mid-step.
            let density = [1u64, 8, 64, 512, 4096][case % 5];
            let mut bits = Box::new([0u64; OCCUPIED_WORDS]);
            for word in bits.iter_mut() {
                for bit in 0..64 {
                    if rng.next() % 4096 < density {
                        *word |= 1 << bit;
                    }
                }
            }
            let len = match case % 3 {
                0 => (rng.next() % 80) as usize,
                1 => 30 + (rng.next() % 10) as usize,
                _ => (rng.next() % 5000) as usize,
            };
            let hay: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let expected: Vec<usize> = (0..len.saturating_sub(3))
                .filter(|&at| {
                    occupied(
                        &bits,
                        u32::from_le_bytes(hay[at..at + 4].try_into().unwrap()),
                    )
                })
                .collect();
            let scalar = all_candidates(&hay, &bits, |h, s, b, o| candidates_scalar(h, s, b, o, 0));
            assert_eq!(scalar, expected, "scalar, case {case}");
            #[cfg(target_arch = "x86_64")]
            if std::arch::is_x86_feature_detected!("avx2") {
                // SAFETY: AVX2 is available.
                let simd = all_candidates(&hay, &bits, |h, s, b, o| unsafe {
                    candidates_avx2(h, s, b, o)
                });
                assert_eq!(simd, expected, "avx2, case {case}");
            }
        }
    }

    #[test]
    fn pattern_set_finds_exactly_what_each_pattern_finds_alone() {
        let mut rng = Lcg(7);
        for case in 0..400 {
            // A 3-symbol alphabet makes keys collide and matches overlap often.
            let alphabet = if case % 2 == 0 { 3 } else { 256 };
            let hay: Vec<u8> = (0..(rng.next() % 600) as usize)
                .map(|_| (rng.next() % alphabet) as u8)
                .collect();
            let mut texts = Vec::new();
            for _ in 0..1 + rng.next() % 12 {
                let len = 1 + (rng.next() % 9) as usize;
                // Patterns cut from the haystack match at least once.
                let start = if hay.len() > len {
                    (rng.next() as usize) % (hay.len() - len)
                } else {
                    0
                };
                let tokens: Vec<String> = (0..len)
                    .map(|k| {
                        if rng.next().is_multiple_of(4) {
                            "??".to_string()
                        } else {
                            let byte = hay
                                .get(start + k)
                                .copied()
                                .unwrap_or((rng.next() % alphabet) as u8);
                            format!("{byte:02X}")
                        }
                    })
                    .collect();
                if tokens.iter().any(|t| t != "??") {
                    texts.push(tokens.join(" "));
                }
            }
            if texts.is_empty() {
                continue;
            }
            // The same pattern twice, too.
            texts.push(texts[0].clone());
            let patterns: Vec<Pattern> = texts.iter().map(|t| t.parse().unwrap()).collect();
            let set = PatternSet::new(patterns.clone());
            for limit in [0, 1, 2, 3, usize::MAX] {
                let all = set.find_all(&hay, limit);
                for (pattern, (found, text)) in patterns.iter().zip(all.iter().zip(&texts)) {
                    assert_eq!(
                        found,
                        &pattern.find_all(&hay, limit),
                        "case {case} `{text}` limit {limit}"
                    );
                }
                // Split among threads: parts shorter than a pattern and, for
                // short haystacks, a part per byte and more parts than bytes.
                let parts = 2 + (rng.next() % 7) as usize;
                let tiny = if hay.len() <= 16 { hay.len() + 3 } else { 1 };
                for parts in [parts, tiny] {
                    assert_eq!(
                        set.find_all_split(&hay, limit, parts),
                        all,
                        "case {case} limit {limit} parts {parts}"
                    );
                }
            }
        }
    }

    #[test]
    fn pattern_set_handles_keys_at_the_edges_and_patterns_without_a_key() {
        let hay = [1u8, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4];
        let texts = [
            // Key at offset 0, matches at both ends of the haystack.
            "01 02 03 04",
            // Key after a wildcard prefix: the first window would start before 0.
            "?? 02 03 04 05",
            "?? ?? ?? 01 02 03 04",
            // No four consecutive literal bytes: searched on its own.
            "01 ?? 03 ?? 05",
            "07 08 ??",
            // Longer than the haystack.
            "01 02 03 04 05 06 07 08 01 02 03 04 05",
        ];
        let patterns: Vec<Pattern> = texts.iter().map(|t| t.parse().unwrap()).collect();
        let set = PatternSet::new(patterns.clone());
        assert_eq!(set.len(), texts.len());
        let all = set.find_all(&hay, 10);
        for (pattern, found) in patterns.iter().zip(&all) {
            assert_eq!(found, &pattern.find_all(&hay, 10));
        }
        assert_eq!(all[0], vec![0, 8]);
        assert_eq!(all[2], vec![5]);
        assert_eq!(all[3], vec![0]);
        assert!(all[5].is_empty());
        assert!(PatternSet::new(Vec::new()).find_all(&hay, 2).is_empty());
        let empty = PatternSet::new(patterns);
        assert!(empty.find_all(&[], 2).iter().all(Vec::is_empty));
        assert!(empty.find_all(&[1, 2, 3], 2).iter().all(Vec::is_empty));
    }

    #[test]
    fn keys_prefer_rare_bytes_but_any_choice_gives_the_same_matches() {
        // The run "48 8B 05 1D" is common; "B8 67 66 78" is not.
        let p: Pattern = "48 8B 05 1D ?? 48 B8 67 66 78".parse().unwrap();
        let (offset, key) = p.key().unwrap();
        assert_eq!(offset, 6);
        assert_eq!(key, u32::from_le_bytes([0xB8, 0x67, 0x66, 0x78]));
        assert!("01 ?? 02 03 04 ?? 05"
            .parse::<Pattern>()
            .unwrap()
            .key()
            .is_none());
    }

    #[test]
    fn long_literals_beyond_the_anchor_cap_are_still_checked() {
        let mut bytes = vec![0x41; 600];
        bytes[599] = 0x42;
        let text = std::iter::once("??".to_string())
            .chain(bytes.iter().map(|b| format!("{b:02X}")))
            .chain(std::iter::once("??".into()))
            .collect::<Vec<_>>()
            .join(" ");
        let p: Pattern = text.parse().unwrap();
        let mut hay = vec![0x41; 1804];
        hay[600] = 0x42;
        hay[1202] = 0x42;
        assert_eq!(p.find_all(&hay, 10), vec![0, 602]);
        assert_eq!(p.len(), 602);
    }
    #[test]
    fn malformed_pe_tables_are_rejected_without_unchecked_arithmetic() {
        assert!(sections(&[]).is_none());
        let mut b = vec![0; 128];
        b[..2].copy_from_slice(b"MZ");
        b[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(sections(&b).is_none());
        b[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        b[64..68].copy_from_slice(b"PE\0\0");
        b[70..72].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(sections(&b).is_none());
        b[70..72].copy_from_slice(&1u16.to_le_bytes());
        assert!(sections(&b).is_some());
        b[84..86].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(sections(&b).is_none());
    }

    #[test]
    fn finds_with_wildcards() {
        let hay = [
            0x00, 0x48, 0x8B, 0x05, 0x11, 0x22, 0xF3, 0x48, 0x8B, 0x05, 0x33, 0x44, 0xF3,
        ];
        let p: Pattern = "48 8B 05 ?? ?? F3".parse().unwrap();
        assert_eq!(p.find_all(&hay, 10), vec![1, 7]);
        assert_eq!(p.find(&hay), Some(1));
    }

    #[test]
    fn leading_wildcard_and_tail_match() {
        let hay = [0xAA, 0xBB, 0xCC];
        let p: Pattern = "?? BB CC".parse().unwrap();
        assert_eq!(p.find_all(&hay, 10), vec![0]);
        let p: Pattern = "?? AA".parse().unwrap();
        assert!(p.find_all(&hay, 10).is_empty());
    }

    #[test]
    fn rejects_bad_patterns() {
        assert_eq!("".parse::<Pattern>().unwrap_err(), PatternError::Empty);
        assert_eq!(
            "?? ??".parse::<Pattern>().unwrap_err(),
            PatternError::AllWildcards
        );
        assert!(matches!(
            "4G".parse::<Pattern>(),
            Err(PatternError::BadToken(_))
        ));
        assert!(parse_hex_bytes("90 ??").is_err());
        assert_eq!(parse_hex_bytes("90 eb").unwrap(), vec![0x90, 0xEB]);
    }

    #[test]
    fn haystack_shorter_than_pattern() {
        let p: Pattern = "01 02 03".parse().unwrap();
        assert!(p.find_all(&[0x01, 0x02], 10).is_empty());
    }
}
