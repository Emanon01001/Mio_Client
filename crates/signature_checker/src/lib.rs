//! Offline audit over the client's own signature catalog and static checks.
pub mod cli;
#[allow(dead_code)]
#[path = "../../shared/compatibility.rs"]
mod compatibility;
mod mapped;
mod pe;
#[allow(dead_code)]
#[path = "../../client/src/game/native/signatures.rs"]
mod signatures;
#[path = "../../shared/skin_layout_checks.rs"]
mod skin_layout_checks;
#[cfg(test)]
mod tests;
mod validate;

use compatibility::Code;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Instant,
};

pub const MATCH_LIMIT: usize = 256;
/// Catalog embedded in this checker; rebuilding follows the client's additions.
pub const SIGNATURE_COUNT: usize = signatures::CATALOG.len();
pub const NOTE: &str = "Static matches and cross-checks do not prove runtime ABI, object lifetime, or full feature compatibility.";
pub const DEFAULT_EXE: &str = "C:/XboxGames/Minecraft for Windows/Content/Minecraft.Windows.exe";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub machine: u16,
    pub timestamp: u32,
    pub image_size: u32,
    pub checksum: u32,
    pub image_base: u64,
    pub file_size: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Core,
    Address,
    Optional,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchState {
    Missing,
    Unique,
    Candidates,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignatureResult {
    pub name: String,
    pub purpose: String,
    pub pattern: String,
    pub role: Role,
    pub state: MatchState,
    pub capped: bool,
    pub rvas: Vec<u64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passed,
    Failed,
    Review,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub detail: String,
    pub rvas_or_values: BTreeMap<String, u64>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    StaticChecksPassed,
    ReviewRequired,
    IssuesFound,
}
impl Outcome {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::StaticChecksPassed => 0,
            Self::IssuesFound => 1,
            Self::ReviewRequired => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::StaticChecksPassed => "STATIC CHECKS PASSED",
            Self::IssuesFound => "ISSUES FOUND",
            Self::ReviewRequired => "REVIEW REQUIRED",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    pub name: String,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub exe: String,
    pub identity: Identity,
    /// PE header identity only: this is not a cryptographic file verification.
    pub recorded_build: Option<String>,
    pub elapsed_ms: u64,
    pub outcome: Outcome,
    pub note: String,
    pub signatures: Vec<SignatureResult>,
    pub checks: Vec<Check>,
    pub baseline: Option<String>,
    pub changes: Vec<Change>,
}
struct Scanned<'a> {
    pe: pe::Pe<'a>,
    hits: BTreeMap<&'static str, Vec<usize>>,
    /// The catalog's parsed patterns with their (sorted) RVAs.
    catalog: Vec<(sigscan::Pattern, Vec<usize>)>,
}
impl Scanned<'_> {
    /// The matches of `p` among the uncapped matches of the most selective
    /// catalog pattern that covers it (such as an option getter with its id
    /// filled in), instead of another pass over the code.
    /// Up to MATCH_LIMIT matches of `p`, from a pass over every executable
    /// section.
    fn find_by_scan(&self, p: &sigscan::Pattern) -> Vec<usize> {
        let mut found = Vec::new();
        for (section, bytes) in self.pe.executable_sections() {
            found.extend(
                p.find_all(bytes, MATCH_LIMIT - found.len())
                    .into_iter()
                    .map(|at| section.rva as usize + at),
            );
            if found.len() == MATCH_LIMIT {
                break;
            }
        }
        found.sort_unstable();
        found
    }
    fn find_covered(&self, p: &sigscan::Pattern) -> Option<Vec<usize>> {
        let (_, candidates) = self
            .catalog
            .iter()
            .filter(|(known, rvas)| rvas.len() < MATCH_LIMIT && known.covers(p))
            .min_by_key(|(_, rvas)| rvas.len())?;
        let sections: Vec<_> = self.pe.executable_sections().collect();
        Some(
            candidates
                .iter()
                .copied()
                .filter(|&rva| {
                    // Exactly what a scan of each executable section accepts.
                    sections.iter().any(|(s, bytes)| {
                        rva.checked_sub(s.rva as usize)
                            .and_then(|at| bytes.get(at..))
                            .is_some_and(|code| p.matches(code))
                    })
                })
                .collect(),
        )
    }
}
impl Code for Scanned<'_> {
    fn find(&self, pattern: &str) -> Vec<usize> {
        if let Some(v) = self.hits.get(pattern) {
            return v.clone();
        }
        let Ok(p) = pattern.parse::<sigscan::Pattern>() else {
            return Vec::new();
        };
        self.find_covered(&p)
            .unwrap_or_else(|| self.find_by_scan(&p))
    }
    fn bytes(&self, rva: usize, len: usize) -> Option<&[u8]> {
        self.pe.bytes(rva, len)
    }
    fn section(&self, name: &str) -> Option<(usize, usize)> {
        self.pe.section(name)
    }
    fn base(&self) -> usize {
        self.pe.identity.image_base as usize
    }
}
fn purposes() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut comment = Vec::new();
    for line in include_str!("../../client/src/game/native/signatures.rs").lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix("///") {
            comment.push(s.trim());
        } else if let Some(s) = line.strip_prefix("pub(crate) const ") {
            if let Some((name, _)) = s.split_once(':') {
                out.insert(
                    name.to_string(),
                    comment.join(" ").chars().take(1024).collect(),
                );
            }
            comment.clear();
        } else {
            comment.clear();
        }
    }
    out
}
const PREFAULT_THREADS: usize = 4;
/// Reads one byte of every page of `bytes`.
fn prefault(bytes: &[u8]) {
    let sum = bytes.iter().step_by(4096).fold(0u8, |sum, &b| sum ^ b);
    std::hint::black_box(sum);
}
/// The catalog searched in one parallel pass over the executable sections.
fn scan(pe: pe::Pe<'_>) -> Result<Scanned<'_>, String> {
    let patterns: Vec<sigscan::Pattern> = signatures::CATALOG
        .iter()
        .map(|e| {
            e.pattern
                .parse()
                .map_err(|error| format!("{}: {error}", e.name))
        })
        .collect::<Result<_, _>>()?;
    let set = sigscan::PatternSet::new(patterns);
    let mut found = vec![Vec::new(); signatures::CATALOG.len()];
    // On a mapped file the scan is bound by first-touch page faults, which
    // keep scaling with threads well past the memory bandwidth limit of a
    // copied buffer (measured on a 32-thread CPU: 8 → 56 ms, 32 → 41 ms).
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(32);
    let rdata = pe
        .sections
        .iter()
        .find(|s| s.name == ".rdata")
        .and_then(|s| pe.bytes(s.rva as usize, s.raw_size as usize))
        .unwrap_or_default();
    std::thread::scope(|scope| {
        // The cross-checks read .rdata after the scan (vtables). On a mapped
        // file, first touching its pages here overlaps their page faults with
        // the scan instead of adding them after it.
        for part in rdata.chunks(rdata.len().div_ceil(PREFAULT_THREADS).max(1)) {
            let _ = std::thread::Builder::new().spawn_scoped(scope, || prefault(part));
        }
        for (s, bytes) in pe.executable_sections() {
            let hits = set.find_all_parallel(bytes, MATCH_LIMIT, threads);
            for (all, hits) in found.iter_mut().zip(hits) {
                let room = MATCH_LIMIT - all.len();
                all.extend(hits.into_iter().take(room).map(|at| s.rva as usize + at));
            }
        }
    });
    for hits in &mut found {
        hits.sort_unstable();
    }
    Ok(Scanned {
        pe,
        hits: signatures::CATALOG
            .iter()
            .zip(&found)
            .map(|(e, hits)| (e.pattern, hits.clone()))
            .collect(),
        catalog: set.patterns().iter().cloned().zip(found).collect(),
    })
}
pub fn audit_bytes(data: &[u8], path: &Path) -> Result<Report, String> {
    let start = Instant::now();
    let source = scan(pe::Pe::parse(data)?)?;
    let found = source.catalog.iter().map(|(_, hits)| hits.clone());
    let descriptions = purposes();
    let signatures: Vec<_> = signatures::CATALOG
        .iter()
        .zip(found)
        .map(|(e, hits)| SignatureResult {
            name: e.name.into(),
            purpose: descriptions.get(e.name).cloned().unwrap_or_default(),
            pattern: e.pattern.into(),
            role: if signatures::DERIVATION.contains(&e.pattern) {
                Role::Address
            } else if signatures::baseline_required(e.pattern) {
                Role::Core
            } else {
                Role::Optional
            },
            state: match hits.len() {
                0 => MatchState::Missing,
                1 => MatchState::Unique,
                _ => MatchState::Candidates,
            },
            capped: hits.len() == MATCH_LIMIT,
            rvas: hits.into_iter().map(|at| at as u64).collect(),
        })
        .collect();
    let known = compatibility::BUILDS.iter().find(|b| {
        (b.timestamp, b.image_size, b.checksum)
            == (
                source.pe.identity.timestamp,
                source.pe.identity.image_size,
                source.pe.identity.checksum,
            )
    });
    let (checks, resolved) = validate::checks(&source, known);
    let missing = signatures.iter().any(|s| s.state == MatchState::Missing);
    let failed = checks.iter().any(|c| c.state == CheckState::Failed);
    let ambiguous = signatures
        .iter()
        .any(|s| s.state == MatchState::Candidates && !resolved.contains(&s.name));
    let review = checks.iter().any(|c| c.state == CheckState::Review);
    let outcome = if missing || failed {
        Outcome::IssuesFound
    } else if known.is_none() || ambiguous || review {
        Outcome::ReviewRequired
    } else {
        Outcome::StaticChecksPassed
    };
    Ok(Report {
        schema_version: 1,
        exe: path.to_string_lossy().into(),
        identity: source.pe.identity,
        recorded_build: known.map(|b| b.name.into()),
        elapsed_ms: start.elapsed().as_millis() as u64,
        outcome,
        note: NOTE.into(),
        signatures,
        checks,
        baseline: None,
        changes: Vec::new(),
    })
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let before = file.metadata().map_err(|e| e.to_string())?;
    if !before.is_file() || before.len() > limit {
        return Err(format!(
            "File is not regular or exceeds the {} MiB limit",
            limit / 1024 / 1024
        ));
    }
    // Sized up front: growing by doubling would copy a large EXE many times.
    let mut bytes = Vec::with_capacity(before.len() as usize + 1);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let after = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || bytes.len() as u64 != after.len()
    {
        return Err("File changed during reading; please retry".into());
    }
    Ok(bytes)
}
pub fn audit_file(path: &Path, baseline: Option<&Path>) -> Result<Report, String> {
    const LIMIT: u64 = 1024 * 1024 * 1024;
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let mut report = match mapped::Mapped::open(path, LIMIT)? {
        Some(view) => {
            let report = audit_bytes(view.bytes(), &canonical);
            // Unmapping a few hundred MB of touched pages takes tens of ms:
            // do it in the background rather than before the result is shown.
            let _ = std::thread::Builder::new()
                .name("unmap".into())
                .spawn(move || drop(view));
            report?
        }
        None => audit_bytes(&read_bounded(path, LIMIT)?, &canonical)?,
    };
    if let Some(path) = baseline {
        let previous: Report = serde_json::from_slice(&read_bounded(path, 8 * 1024 * 1024)?)
            .map_err(|e| format!("Invalid baseline JSON: {e}"))?;
        if previous.schema_version != 1 {
            return Err("Unsupported baseline schema".into());
        }
        report.changes = compare(&previous, &report);
        report.baseline = Some(path.to_string_lossy().into());
    }
    Ok(report)
}
pub fn compare(previous: &Report, current: &Report) -> Vec<Change> {
    let mut out = Vec::new();
    for new in &current.signatures {
        let detail = match previous.signatures.iter().find(|old| old.name == new.name) {
            None => Some("Added to catalog".into()),
            Some(old) if old.pattern != new.pattern => {
                Some("Catalog pattern changed; counts are not directly comparable".into())
            }
            Some(old) if old.rvas.len() != new.rvas.len() || old.capped != new.capped => {
                Some(format!(
                    "Matches: {}{} -> {}{}",
                    old.rvas.len(),
                    if old.capped { "+" } else { "" },
                    new.rvas.len(),
                    if new.capped { "+" } else { "" }
                ))
            }
            Some(old) if old.rvas != new.rvas => Some("Match RVAs moved".into()),
            _ => None,
        };
        if let Some(detail) = detail {
            out.push(Change {
                name: new.name.clone(),
                detail,
            });
        }
    }
    for old in &previous.signatures {
        if !current.signatures.iter().any(|s| s.name == old.name) {
            out.push(Change {
                name: old.name.clone(),
                detail: "Removed from catalog".into(),
            });
        }
    }
    // Wildcard operands can change while the pattern and match RVAs stay the
    // same. Include field/address cross-checks in baseline comparisons too.
    for new in &current.checks {
        let detail = match previous.checks.iter().find(|old| old.name == new.name) {
            None => Some("Added static check".into()),
            Some(old) if old.state != new.state => Some(format!(
                "Static check: {:?} -> {:?}; {}",
                old.state, new.state, new.detail
            )),
            Some(old) if old.rvas_or_values != new.rvas_or_values => {
                Some("Static check addresses/field values changed".into())
            }
            Some(old) if old.detail != new.detail => Some("Static check detail changed".into()),
            _ => None,
        };
        if let Some(detail) = detail {
            out.push(Change {
                name: new.name.clone(),
                detail,
            });
        }
    }
    for old in &previous.checks {
        if !current.checks.iter().any(|c| c.name == old.name) {
            out.push(Change {
                name: old.name.clone(),
                detail: "Removed static check".into(),
            });
        }
    }
    out
}
/// New-file-only export: never overwrites an EXE, baseline or existing report.
pub fn save_report(path: &Path, report: &Report) -> Result<(), String> {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("json"))
    {
        return Err("Report filename must end in .json".into());
    }
    let bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            format!(
                "Cannot create {} (existing files are not overwritten): {e}",
                path.display()
            )
        })?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| e.to_string())
}
pub fn default_exe() -> PathBuf {
    PathBuf::from(DEFAULT_EXE)
}
pub fn summary(report: &Report) -> String {
    let missing = report
        .signatures
        .iter()
        .filter(|s| s.state == MatchState::Missing)
        .count();
    let candidates = report
        .signatures
        .iter()
        .filter(|s| s.state == MatchState::Candidates)
        .count();
    let passed = report
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Passed)
        .count();
    let failed = report
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Failed)
        .count();
    let review = report
        .checks
        .iter()
        .filter(|c| c.state == CheckState::Review)
        .count();
    format!("{}\n{} signatures: {missing} missing, {candidates} with multiple candidates\n{} static checks: {passed} passed, {failed} failed, {review} review\nPE identity: {:?}; {} ms\n{}", report.outcome.label(), report.signatures.len(), report.checks.len(), report.recorded_build, report.elapsed_ms, NOTE)
}
