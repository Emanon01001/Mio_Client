//! Owned, read-only game data. No type contains a native pointer.
use crate::Vec3;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}
impl Bounds {
    /// Centre of the bottom face, in blocks.
    pub fn position(self) -> Vec3 {
        Vec3::new(
            (self.min.x + self.max.x) * 0.5,
            self.min.y,
            (self.min.z + self.max.z) * 0.5,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Health {
    pub current: f32,
    pub maximum: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    /// Registry entity ID including its generation. Valid only in this world.
    pub id: u32,
    /// Native identifier, usually without the `minecraft:` prefix.
    pub identifier: Arc<str>,
    pub display_name: Option<Arc<str>>,
    pub actor_type: u32,
    pub health: Option<Health>,
    pub bounds: Option<Bounds>,
}
impl Entity {
    pub fn name(&self) -> &str {
        self.display_name
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.identifier)
    }
    pub fn position(&self) -> Option<Vec3> {
        self.bounds.map(Bounds::position)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub identifier: Arc<str>,
    pub count: u32,
    pub damage: u16,
    /// Zero means the item has no durability counter.
    pub max_damage: u16,
}
impl ItemStack {
    pub fn remaining_durability(&self) -> Option<u16> {
        (self.max_damage != 0).then(|| self.max_damage.saturating_sub(self.damage))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inventory {
    /// Zero-based hotbar index (0..9).
    pub selected: usize,
    /// Slots 0..9 are the hotbar; 9..36 are the main inventory. Empty slots stay None.
    pub slots: Vec<Option<ItemStack>>,
    /// Helmet, chestplate, leggings, boots, in that order.
    pub armor: Vec<Option<ItemStack>>,
}
impl Inventory {
    pub fn held(&self) -> Option<&ItemStack> {
        (self.selected < 9)
            .then(|| self.slots.get(self.selected))
            .flatten()?
            .as_ref()
    }
    /// Exact identifier match; `minecraft:` is optional. Does not count armour.
    pub fn count(&self, identifier: &str) -> u32 {
        let identifier = identifier.strip_prefix("minecraft:").unwrap_or(identifier);
        self.slots
            .iter()
            .flatten()
            .filter(|s| {
                s.identifier
                    .strip_prefix("minecraft:")
                    .unwrap_or(&s.identifier)
                    == identifier
            })
            .fold(0u32, |n, s| n.saturating_add(s.count))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vitals {
    pub health: f32,
    pub max_health: f32,
    pub absorption: f32,
    pub hunger: f32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub level: f32,
    /// Fraction towards the next level, 0..1.
    pub experience: f32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    pub id: u32,
    /// Zero-based: 0 means level I.
    pub amplifier: u32,
    /// Remaining game ticks; None is infinite, not unavailable.
    pub ticks: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Air {
    pub current: i16,
    pub maximum: i16,
    pub underwater: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// None means unavailable; an empty slice means no active effects.
    pub effects: Option<Arc<[Effect]>>,
    pub air: Option<Air>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldInfo {
    pub ticks: i32,
    /// Strength 0..1. Weather does not apply in the Nether/End.
    pub rain: f32,
    pub thunder: f32,
    pub dimension: Dimension,
}
impl WorldInfo {
    pub fn day(self) -> i64 {
        i64::from(self.ticks).div_euclid(24_000) + 1
    }
    /// 0 ticks is 06:00; negative times wrap using Euclidean division.
    pub fn clock(self) -> (u32, u32) {
        let minutes = (i64::from(self.ticks).rem_euclid(24_000) * 1440 / 24_000 + 360) % 1440;
        ((minutes / 60) as u32, (minutes % 60) as u32)
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct BlockTarget {
    pub identifier: Arc<str>,
    pub position: [i32; 3],
    /// Eye to ray entry, in blocks.
    pub distance: f32,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Block(BlockTarget),
    Entity { entity: Entity, distance: f32 },
}
