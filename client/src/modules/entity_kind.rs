//! What kind of entity an actor is, shared by the overlays: ESP groups its
//! boxes by it, the radar and the minimap colour their marks by it.

use std::collections::HashMap;

use super::game::actors::Actor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Player,
    /// Attacks players on sight.
    Hostile,
    /// Attacks only when provoked (wolves, bees, endermen, piglins, ...).
    Neutral,
    /// Never attacks (animals, villagers, fish, ...).
    Passive,
    /// Not a known mob: items, projectiles, vehicles, modded entities.
    Other,
}

/// The kind of an actor from its identifier (with or without the
/// `minecraft:` namespace) and numeric type (63 is a player).
pub fn kind(name: &str, actor_type: u32) -> Kind {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    if name == "player" || actor_type & 255 == 63 {
        return Kind::Player;
    }
    match name {
        "zombie" | "husk" | "drowned" | "skeleton" | "stray" | "bogged" | "wither_skeleton"
        | "creeper" | "spider" | "cave_spider" | "slime" | "magma_cube" | "blaze" | "ghast"
        | "phantom" | "silverfish" | "endermite" | "guardian" | "elder_guardian" | "witch"
        | "pillager" | "vindicator" | "evocation_illager" | "evoker" | "ravager" | "vex"
        | "wither" | "ender_dragon" | "warden" | "breeze" | "creaking" | "piglin_brute"
        | "zoglin" | "hoglin" | "shulker" | "zombie_villager" | "zombie_villager_v2" => {
            Kind::Hostile
        }
        "wolf" | "bee" | "enderman" | "piglin" | "zombie_pigman" | "zombified_piglin"
        | "iron_golem" | "polar_bear" | "panda" | "llama" | "trader_llama" | "goat" | "dolphin" => {
            Kind::Neutral
        }
        "cow" | "mooshroom" | "pig" | "sheep" | "chicken" | "rabbit" | "horse" | "donkey"
        | "mule" | "skeleton_horse" | "zombie_horse" | "camel" | "turtle" | "fox" | "cat"
        | "ocelot" | "parrot" | "bat" | "squid" | "glow_squid" | "cod" | "salmon"
        | "tropicalfish" | "tropical_fish" | "pufferfish" | "axolotl" | "frog" | "tadpole"
        | "sniffer" | "armadillo" | "allay" | "strider" | "villager" | "villager_v2"
        | "wandering_trader" | "snow_golem" => Kind::Passive,
        _ => Kind::Other,
    }
}

/// The kind of the entity `id`, from the actor facts (Other without them).
pub fn of(facts: Option<&HashMap<u32, Actor>>, id: u32) -> Kind {
    facts
        .and_then(|facts| facts.get(&id))
        .map_or(Kind::Other, |a| kind(&a.name, a.actor_type))
}

/// The colour of an entity's mark on the radar and the minimap: players
/// green, hostile red, neutral yellow, passive blue, others grey.
pub fn mark(kind: Kind) -> [f32; 4] {
    match kind {
        Kind::Player => [0.35, 0.9, 0.4, 1.],
        Kind::Hostile => [1., 0.3, 0.3, 1.],
        Kind::Neutral => [1., 0.82, 0.25, 1.],
        Kind::Passive => [0.35, 0.65, 1., 1.],
        Kind::Other => [0.75, 0.75, 0.78, 1.],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobs_are_sorted_by_how_they_treat_players() {
        assert_eq!(kind("minecraft:player", 0), Kind::Player);
        assert_eq!(kind("anything", 63), Kind::Player);
        assert_eq!(kind("minecraft:creeper", 0), Kind::Hostile);
        assert_eq!(kind("hoglin", 0), Kind::Hostile);
        assert_eq!(kind("wolf", 0), Kind::Neutral);
        assert_eq!(kind("enderman", 0), Kind::Neutral);
        assert_eq!(kind("zombie_pigman", 0), Kind::Neutral);
        assert_eq!(kind("cow", 0), Kind::Passive);
        assert_eq!(kind("villager_v2", 0), Kind::Passive);
        assert_eq!(kind("item", 0), Kind::Other);
        assert_eq!(kind("modded:beast", 0), Kind::Other);
        assert_eq!(of(None, 7), Kind::Other);
    }

    #[test]
    fn every_kind_has_its_own_mark_colour() {
        let all = [
            Kind::Player,
            Kind::Hostile,
            Kind::Neutral,
            Kind::Passive,
            Kind::Other,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(mark(*a), mark(*b), "{a:?} {b:?}");
            }
        }
        assert_eq!(mark(Kind::Player)[1], 0.9, "players are green");
        assert_eq!(mark(Kind::Hostile)[0], 1., "hostile mobs are red");
    }
}
