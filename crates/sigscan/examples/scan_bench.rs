//! Measures signature scanning against a real executable on disk.
//!
//! ```text
//! cargo run --release -p sigscan --example scan_bench -- <Minecraft.Windows.exe> <crates/client/src/game/native/signatures.rs> [rounds]
//! ```
//!
//! Every `const …: &str` signature in the given Rust file is scanned over the
//! executable sections exactly as the client does at injection (all matches up
//! to two, so a unique match is proven by a full pass). Prints the median time
//! per round and the per-signature cost, so changes to the scanner can be
//! compared on the code they actually run against.

use std::time::{Duration, Instant};

use sigscan::Pattern;

/// Every string literal assigned to a `const NAME: &str` in `source`.
fn signatures(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("const ") {
        rest = &rest[at + 6..];
        let Some(colon) = rest.find(':') else { break };
        let name = rest[..colon].trim().to_string();
        let after = &rest[colon + 1..];
        let Some(eq) = after.find('=') else { break };
        if !after[..eq].contains("&str") {
            continue;
        }
        let value = &after[eq + 1..];
        let Some(open) = value.find('"') else { break };
        let Some(close) = value[open + 1..].find('"') else {
            break;
        };
        let literal = &value[open + 1..open + 1 + close];
        // `\` line continuations inside the literal.
        let text: String = literal
            .split('\\')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        if text.split_whitespace().all(|t| t.len() == 2 || t == "?") && !text.is_empty() {
            out.push((name, text));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: scan_bench <exe> <signatures.rs> [rounds]");
        std::process::exit(2);
    }
    let image = std::fs::read(&args[1]).expect("read exe");
    let source = std::fs::read_to_string(&args[2]).expect("read signatures");
    let rounds: usize = args.get(3).and_then(|r| r.parse().ok()).unwrap_or(5);

    let sections = sigscan::sections(&image[..image.len().min(0x1000)]).expect("PE headers");
    let code: Vec<&[u8]> = sections
        .iter()
        .filter(|s| s.executable && s.raw_size > 0)
        .map(|s| {
            let start = s.raw_offset as usize;
            &image[start..start + s.raw_size.min(s.virtual_size.max(s.raw_size)) as usize]
        })
        .collect();
    let code_bytes: usize = code.iter().map(|c| c.len()).sum();

    let mut list = signatures(&source);
    // The two id-parameterized getters, with this build's option ids.
    list.push((
        "gamma_signature(0x32)".into(),
        "48 83 EC 38 48 8B 05 ?? ?? ?? ?? 48 31 E0 48 89 44 24 30 48 8B 01 48 8B 40 08 48 8D 54 24 28 41 B8 32 00 00 00 FF 15 ?? ?? ?? ?? 48 8B 4C 24 28 48 89 C8 48 8B 49 08 48 8B 89 ?? ?? 00 00 48 85 C9 75 ?? F3 0F 10 40 ??".into(),
    ));
    let patterns: Vec<(String, Pattern)> = list
        .into_iter()
        .filter_map(|(name, text)| text.parse().ok().map(|p| (name, p)))
        .collect();

    let scan = |p: &Pattern| -> usize { code.iter().map(|c| p.find_all(c, 2).len()).sum() };

    // Per-signature cost (median of rounds) and match counts.
    let mut rows = Vec::new();
    for (name, p) in &patterns {
        let mut times = Vec::new();
        let mut hits = 0;
        for _ in 0..rounds {
            let start = Instant::now();
            hits = scan(p);
            times.push(start.elapsed());
        }
        times.sort();
        rows.push((times[times.len() / 2], name.clone(), hits));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));

    let mut totals = Vec::new();
    for _ in 0..rounds {
        let start = Instant::now();
        let mut hits = 0;
        for (_, p) in &patterns {
            hits += scan(p);
        }
        std::hint::black_box(hits);
        totals.push(start.elapsed());
    }
    totals.sort();
    let total: Duration = totals[totals.len() / 2];

    println!(
        "{} signatures over {:.1} MB of code; median full pass {:.1} ms ({:.2} ms/signature, {:.2} GB/s)",
        patterns.len(),
        code_bytes as f64 / 1e6,
        total.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3 / patterns.len() as f64,
        code_bytes as f64 * patterns.len() as f64 / total.as_secs_f64() / 1e9
    );
    println!("slowest:");
    for (t, name, hits) in rows
        .iter()
        .take(if std::env::var_os("SCAN_BENCH_ALL").is_some() {
            usize::MAX
        } else {
            12
        })
    {
        println!(
            "  {:>8.2} ms  {hits} match(es)  {name}",
            t.as_secs_f64() * 1e3
        );
    }

    // How often each pattern's index key occurs: every occurrence costs a
    // full comparison in the single pass.
    if std::env::var_os("SCAN_BENCH_KEYS").is_some() {
        let sampled = sigscan::PatternSet::with_sample(
            patterns.iter().map(|(_, p)| p.clone()).collect(),
            &code[0][..code[0].len().min(4 << 20)],
        );
        let keys: std::collections::HashMap<u32, Vec<&str>> = sampled
            .keys()
            .map(|(index, _, k)| (k, patterns[index].0.as_str()))
            .fold(Default::default(), |mut map, (k, name)| {
                map.entry(k).or_insert_with(Vec::new).push(name);
                map
            });
        let mut counts: std::collections::HashMap<u32, usize> = Default::default();
        for c in &code {
            for w in c.windows(4) {
                let k = u32::from_le_bytes(w.try_into().unwrap());
                if keys.contains_key(&k) {
                    *counts.entry(k).or_default() += 1;
                }
            }
        }
        let mut rows: Vec<_> = counts.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        println!("most frequent index keys:");
        for (k, n) in rows.iter().take(10) {
            println!("  {n:>9}  {:02X?}  {:?}", k.to_le_bytes(), keys[k]);
        }
        let none: Vec<_> = patterns
            .iter()
            .filter(|(_, p)| p.key().is_none())
            .map(|(n, _)| n.as_str())
            .collect();
        println!("without a 4-byte key (scanned alone): {none:?}");
    }

    // The same signatures in a single pass, as the client resolves them:
    // keys chosen on a sample of the code, then one pass over all of it.
    let limit = 256;
    let sample_len = 4 << 20;
    let mut set_times = Vec::new();
    let mut set_results = Vec::new();
    let mut tune = Duration::ZERO;
    for _ in 0..rounds {
        let start = Instant::now();
        let sample = &code[0][..code[0].len().min(sample_len)];
        let set = sigscan::PatternSet::with_sample(
            patterns.iter().map(|(_, p)| p.clone()).collect(),
            sample,
        );
        tune = start.elapsed();
        set_results = code
            .iter()
            .map(|c| set.find_all(c, limit))
            .collect::<Vec<_>>();
        set_times.push(start.elapsed());
    }
    println!(
        "key selection on a {} MB sample: {:.1} ms (included below)",
        sample_len >> 20,
        tune.as_secs_f64() * 1e3
    );
    let plain = sigscan::PatternSet::new(patterns.iter().map(|(_, p)| p.clone()).collect());
    let start = Instant::now();
    let plain_results: Vec<_> = code.iter().map(|c| plain.find_all(c, limit)).collect();
    println!(
        "  (static key choice instead: {:.1} ms)",
        start.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(plain_results, set_results);
    set_times.sort();
    let set_total = set_times[set_times.len() / 2];
    // Identical to scanning each pattern on its own, for every section.
    for (section, (results, c)) in set_results.iter().zip(&code).enumerate() {
        for ((name, p), found) in patterns.iter().zip(results) {
            assert_eq!(found, &p.find_all(c, limit), "{name} in section {section}");
        }
    }
    if std::env::var_os("SCAN_BENCH_COUNTS").is_some() {
        println!("matches (up to {limit} per section):");
        for (index, (name, _)) in patterns.iter().enumerate() {
            let n: usize = set_results.iter().map(|r| r[index].len()).sum();
            println!("  {n:>5}  {name}");
        }
    }
    println!(
        "single pass (PatternSet, up to {limit} matches each): median {:.1} ms; {:.1}x faster than separate scans; results identical",
        set_total.as_secs_f64() * 1e3,
        total.as_secs_f64() / set_total.as_secs_f64()
    );

    // The same pass divided among threads (key selection included, as above).
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts = vec![2, 4, 8, cores];
    counts.sort_unstable();
    counts.dedup();
    for threads in counts.into_iter().filter(|&t| t <= cores.max(2)) {
        let mut times = Vec::new();
        for _ in 0..rounds {
            let start = Instant::now();
            let sample = &code[0][..code[0].len().min(sample_len)];
            let set = sigscan::PatternSet::with_sample(
                patterns.iter().map(|(_, p)| p.clone()).collect(),
                sample,
            );
            let results: Vec<_> = code
                .iter()
                .map(|c| set.find_all_parallel(c, limit, threads))
                .collect();
            times.push(start.elapsed());
            assert_eq!(results, set_results, "{threads} threads");
        }
        times.sort();
        let median = times[times.len() / 2];
        println!(
            "  {threads:>2} threads: median {:.1} ms ({:.1}x the single pass, {:.1}x separate scans); results identical",
            median.as_secs_f64() * 1e3,
            set_total.as_secs_f64() / median.as_secs_f64(),
            total.as_secs_f64() / median.as_secs_f64()
        );
    }
}
