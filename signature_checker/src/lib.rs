//! Offline audit over the client's own signature catalog and static checks.
pub mod cli;
#[allow(dead_code)]
#[path = "../../shared/compatibility.rs"]
mod compatibility;
mod pe;
#[allow(dead_code)]
#[path = "../../client/src/signatures.rs"]
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
}
impl Code for Scanned<'_> {
    fn find(&self, pattern: &str) -> Vec<usize> {
        if let Some(v) = self.hits.get(pattern) {
            return v.clone();
        }
        let Ok(p) = pattern.parse::<sigscan::Pattern>() else {
            return Vec::new();
        };
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
    for line in include_str!("../../client/src/signatures.rs").lines() {
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
pub fn audit_bytes(data: &[u8], path: &Path) -> Result<Report, String> {
    let start = Instant::now();
    let pe = pe::Pe::parse(data)?;
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
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(8);
    for (s, bytes) in pe.executable_sections() {
        let hits = set.find_all_parallel(bytes, MATCH_LIMIT, threads);
        for (all, hits) in found.iter_mut().zip(hits) {
            let room = MATCH_LIMIT - all.len();
            all.extend(hits.into_iter().take(room).map(|at| s.rva as usize + at));
        }
    }
    for hits in &mut found {
        hits.sort_unstable();
    }
    let source = Scanned {
        pe,
        hits: signatures::CATALOG
            .iter()
            .zip(&found)
            .map(|(e, hits)| (e.pattern, hits.clone()))
            .collect(),
    };
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
    let mut bytes = Vec::new();
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
    let data = read_bounded(path, 1024 * 1024 * 1024)?;
    let mut report = audit_bytes(&data, &path.canonicalize().map_err(|e| e.to_string())?)?;
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
