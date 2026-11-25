//! World facts read on the game thread. Currently: which dimension the local
//! player is in. Read-only; see analysis/notes/blocks.md for the pointer path.
use crate::game::{known_build, layout::image, memory, read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

/// Actor -> its dimension (the same field `blocks.rs` starts from).
const ACTOR_DIMENSION: usize = 0x1F0;
/// Dimension -> display name ("Overworld") and identifier ("minecraft:overworld").
const DIMENSION_NAME: usize = 0x20;
const DIMENSION_ID: usize = 0x180;
/// The name is re-read this often; it only changes when travelling.
const INTERVAL_MS: u64 = 1000;
/// A reading older than this is not reported (the HUD hook stopped).
const MAX_AGE_MS: u64 = 3000;

/// The dimensions that matter for coordinate conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realm {
    Overworld,
    Nether,
    End,
    Other,
}

/// Classifies a dimension by its identifier or display name.
pub fn classify(text: &str) -> Realm {
    let text = text.to_ascii_lowercase();
    let text = text.strip_prefix("minecraft:").unwrap_or(&text);
    if text.contains("nether") {
        Realm::Nether
    } else if text.contains("overworld") {
        Realm::Overworld
    } else if text.contains("end") {
        Realm::End
    } else {
        Realm::Other
    }
}

struct Reading {
    at: u64,
    name: String,
    realm: Realm,
}

static LAST_SAMPLE: AtomicU64 = AtomicU64::new(0);
static READING: Mutex<Option<Reading>> = Mutex::new(None);

unsafe fn dimension_fields(player: usize) -> Option<(String, Option<String>)> {
    let mut batch = memory::ReadBatch::new();
    if !batch.readable(player.checked_add(ACTOR_DIMENSION)?, 8) {
        return None;
    }
    let dimension: usize = read(player + ACTOR_DIMENSION);
    if !batch.readable(dimension, DIMENSION_ID + 32) {
        return None;
    }
    // A live C++ object: its vtable lies inside the game image.
    let (base, end) = image();
    if !(base..end).contains(&read::<usize>(dimension)) {
        return None;
    }
    let name = batch
        .string(dimension + DIMENSION_NAME, 64)
        .filter(|s| !s.is_empty())?;
    let id = batch
        .string(dimension + DIMENSION_ID, 64)
        .filter(|s| !s.is_empty());
    Some((name, id))
}
pub(in crate::game) unsafe fn read_dimension(player: usize) -> Option<(String, Realm)> {
    let (name, id) = dimension_fields(player)?;
    let realm = classify(id.as_deref().unwrap_or(&name));
    Some((name, realm))
}
/// Persistence uses the identifier itself, never a substring classification
/// of a custom dimension's display name.
pub(in crate::game) unsafe fn storage_dimension(player: usize) -> Option<String> {
    let (name, id) = dimension_fields(player)?;
    canonical_dimension(id.as_deref().unwrap_or(&name))
}
fn canonical_dimension(id: &str) -> Option<String> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return None;
    }
    Some(match id.to_ascii_lowercase().as_str() {
        "overworld" | "minecraft:overworld" => "minecraft:overworld".into(),
        "nether" | "minecraft:nether" => "minecraft:nether".into(),
        "end" | "theend" | "the end" | "the_end" | "minecraft:the_end" => {
            "minecraft:the_end".into()
        }
        _ => format!("custom:{id}"),
    })
}

/// Only called inside the HUD hook, while the local player is live.
pub(in crate::game) unsafe fn sample(player: usize, now: u64) {
    if now.saturating_sub(LAST_SAMPLE.load(Ordering::Relaxed)) < INTERVAL_MS {
        return;
    }
    LAST_SAMPLE.store(now, Ordering::Relaxed);
    // The offsets above were taken from this build only.
    if !known_build() {
        return;
    }
    let reading = read_dimension(player).map(|(name, realm)| Reading {
        at: now,
        name,
        realm,
    });
    if let Ok(mut slot) = READING.try_lock() {
        *slot = reading;
    }
}

/// The local player's dimension (display name, kind), if in a world.
pub fn dimension() -> Option<(String, Realm)> {
    let slot = READING.lock().unwrap_or_else(|e| e.into_inner());
    let reading = slot.as_ref()?;
    (unsafe { GetTickCount64() }.saturating_sub(reading.at) <= MAX_AGE_MS)
        .then(|| (reading.name.clone(), reading.realm))
}

/// Just the kind, for callers that only convert coordinates.
pub fn realm() -> Option<Realm> {
    dimension().map(|(_, realm)| realm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_keys_do_not_alias_custom_dimensions_containing_builtin_names() {
        assert_eq!(
            canonical_dimension("TheEnd").as_deref(),
            Some("minecraft:the_end")
        );
        assert_eq!(
            canonical_dimension("Overworld").as_deref(),
            Some("minecraft:overworld")
        );
        assert_eq!(
            canonical_dimension("my_mod:weekend").as_deref(),
            Some("custom:my_mod:weekend")
        );
        assert_ne!(
            canonical_dimension("my_mod:nether_moon"),
            canonical_dimension("minecraft:nether")
        );
        assert!(canonical_dimension("").is_none());
        assert!(canonical_dimension("bad\nkey").is_none());
    }

    #[test]
    fn classifies_identifiers_and_display_names() {
        assert_eq!(classify("minecraft:overworld"), Realm::Overworld);
        assert_eq!(classify("Overworld"), Realm::Overworld);
        assert_eq!(classify("minecraft:nether"), Realm::Nether);
        assert_eq!(classify("Nether"), Realm::Nether);
        assert_eq!(classify("minecraft:the_end"), Realm::End);
        assert_eq!(classify("TheEnd"), Realm::End);
        assert_eq!(classify("custom:moon"), Realm::Other);
        assert_eq!(classify(""), Realm::Other);
    }

    #[test]
    fn nothing_is_reported_outside_the_game() {
        // No sample has run in the test process.
        assert_eq!(dimension(), None);
        assert_eq!(realm(), None);
        // A null player is rejected before any read.
        assert_eq!(unsafe { read_dimension(0) }, None);
    }
}
