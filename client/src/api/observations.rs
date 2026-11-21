//! Read-only SDK adapters over existing owned native snapshots.
use super::subscriptions::{Lease, Pool, Release};
use crate::modules::{game as native, waila};
use mcbe_sdk::{
    bridge::{ObservationBackend, ObservationSample, ObservationSource, World},
    *,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Resources {
    actors: bool,
    vitals: bool,
    items: bool,
    status: bool,
    look: bool,
    world: Option<native::world_info::Subscription>,
}
impl Resources {
    fn acquire(&mut self, topic: Topic) -> Result<()> {
        match topic {
            Topic::Entities | Topic::Target => {
                native::actors::use_actors(true).map_err(Error::Unavailable)?;
                self.actors = true;
                if topic == Topic::Target {
                    native::blocks::available().map_err(Error::Unavailable)?;
                    native::blocks::set_look(true);
                    self.look = true;
                }
            }
            Topic::Vitals => {
                native::actors::use_vitals(true).map_err(Error::Unavailable)?;
                self.vitals = true;
            }
            Topic::Inventory => {
                native::items::use_items(true).map_err(Error::Unavailable)?;
                self.items = true;
            }
            Topic::Status => {
                native::status::use_status(true).map_err(Error::Unavailable)?;
                self.status = true;
            }
            Topic::World => {
                self.world =
                    Some(native::world_info::subscribe_clock().map_err(Error::Unavailable)?)
            }
        }
        Ok(())
    }
}
impl Release for Resources {
    fn release(&mut self) -> bool {
        if std::mem::take(&mut self.look) {
            native::blocks::set_look(false);
        }
        self.world.take();
        // Failed release retains its flag for Runtime::shutdown's next retry.
        if self.actors && native::actors::use_actors(false).is_ok() {
            self.actors = false;
        }
        if self.vitals && native::actors::use_vitals(false).is_ok() {
            self.vitals = false;
        }
        if self.items && native::items::use_items(false).is_ok() {
            self.items = false;
        }
        if self.status && native::status::use_status(false).is_ok() {
            self.status = false;
        }
        !self.actors && !self.vitals && !self.items && !self.status
    }
}
pub(super) struct NativeObservations {
    pool: Pool<Resources>,
}
impl NativeObservations {
    pub fn new() -> Self {
        Self { pool: Pool::new() }
    }
    pub fn close(&self) -> bool {
        self.pool.close()
    }
}
impl ObservationBackend for NativeObservations {
    fn subscribe(&self, topic: Topic) -> Result<Box<dyn ObservationSource>> {
        let lease = self
            .pool
            .acquire(Resources::default(), |r| r.acquire(topic))?;
        Ok(Box::new(Source {
            topic,
            lease,
            cache: Mutex::new(None),
        }))
    }
}
type CacheKey = (World, u64, u64);
struct Source {
    topic: Topic,
    lease: Lease<Resources>,
    cache: Mutex<Option<(CacheKey, ObservationSample)>>,
}
impl Source {
    fn cached(
        &self,
        key: CacheKey,
        at_ms: u64,
        make: impl FnOnce() -> Result<ObservationData>,
    ) -> Result<ObservationSample> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((old, sample)) = cache.as_ref() {
            if *old == key {
                return Ok(sample.clone());
            }
        }
        let sample = ObservationSample {
            at_ms,
            data: make()?,
        };
        *cache = Some((key, sample.clone()));
        Ok(sample)
    }
}
fn entity(id: u32, value: &native::actors::Actor) -> Entity {
    Entity {
        id,
        identifier: Arc::clone(&value.name),
        display_name: value.display_name.clone(),
        actor_type: value.actor_type,
        health: value
            .health
            .map(|(current, maximum)| Health { current, maximum }),
        bounds: value.bounds.map(|b| Bounds {
            min: Vec3::new(b[0], b[1], b[2]),
            max: Vec3::new(b[3], b[4], b[5]),
        }),
    }
}
fn item(value: &native::items::Item) -> ItemStack {
    ItemStack {
        identifier: value.name.as_str().into(),
        count: u32::from(value.count),
        damage: value.damage,
        max_damage: value.max_damage,
    }
}
impl ObservationSource for Source {
    fn read(&self, world: World) -> Result<ObservationSample> {
        if !self.lease.open() {
            return Err(Error::Detached);
        }
        let owner = (world.0, world.1);
        let sample = match self.topic {
            Topic::Entities => {
                let (at, actors, _) =
                    native::actors::reading_for_local(owner).ok_or(Error::StaleState)?;
                self.cached((world, at, 0), at, || {
                    let mut out: Vec<_> = actors
                        .iter()
                        .filter(|(_, a)| !a.local)
                        .map(|(&id, a)| entity(id, a))
                        .collect();
                    out.sort_unstable_by_key(|e| e.id);
                    Ok(ObservationData::Entities(Arc::new(out)))
                })?
            }
            Topic::Vitals => {
                let (at, v) = native::actors::vitals_for_local(owner).ok_or(Error::StaleState)?;
                let v =
                    v.ok_or_else(|| Error::Unavailable("local attributes are unavailable".into()))?;
                self.cached((world, at, 0), at, || {
                    Ok(ObservationData::Vitals(Arc::new(Vitals {
                        health: v.health,
                        max_health: v.max_health,
                        absorption: v.absorption,
                        hunger: v.hunger,
                        saturation: v.saturation,
                        exhaustion: v.exhaustion,
                        level: v.level,
                        experience: v.experience,
                    })))
                })?
            }
            Topic::Inventory => {
                let (at, v) = native::items::reading_for_local(owner).ok_or(Error::StaleState)?;
                self.cached((world, at, 0), at, || {
                    Ok(ObservationData::Inventory(Arc::new(Inventory {
                        selected: v.selected,
                        slots: v.slots.iter().map(|s| s.as_ref().map(item)).collect(),
                        armor: v.armor.iter().map(|s| s.as_ref().map(item)).collect(),
                    })))
                })?
            }
            Topic::Status => {
                let (at, v) = native::status::reading_for_local(owner).ok_or(Error::StaleState)?;
                self.cached((world, at, 0), at, || {
                    Ok(ObservationData::Status(Arc::new(Status {
                        effects: v.effects.map(|effects| {
                            effects
                                .iter()
                                .map(|e| Effect {
                                    id: e.id,
                                    amplifier: e.amplifier,
                                    ticks: e.ticks,
                                })
                                .collect()
                        }),
                        air: v.air.map(|a| Air {
                            current: a.current,
                            maximum: a.max,
                            underwater: a.underwater,
                        }),
                    })))
                })?
            }
            Topic::World => {
                let (at, v) =
                    native::world_info::reading_for_local(owner).ok_or(Error::StaleState)?;
                self.cached((world, at, 0), at, || {
                    Ok(ObservationData::World(Arc::new(WorldInfo {
                        ticks: v.ticks,
                        rain: v.rain,
                        thunder: v.thunder,
                        dimension: match v.realm {
                            native::world::Realm::Overworld => Dimension::Overworld,
                            native::world::Realm::Nether => Dimension::Nether,
                            native::world::Realm::End => Dimension::End,
                            native::world::Realm::Other => Dimension::Other,
                        },
                    })))
                })?
            }
            Topic::Target => {
                let (at, ray) = native::blocks::look_for_local(owner).ok_or(Error::StaleState)?;
                let (actors_at, actors, _) =
                    native::actors::reading_for_local(owner).ok_or(Error::StaleState)?;
                self.cached((world, at, actors_at), at.min(actors_at), || {
                    let hit =
                        waila::picking::pick(&ray, Some(&actors), ray.reach).map(|t| match t {
                            waila::Target::Block(b) => Target::Block(BlockTarget {
                                identifier: b.name.into(),
                                position: b.position,
                                distance: b.distance,
                            }),
                            waila::Target::Entity {
                                id,
                                actor,
                                distance,
                            } => Target::Entity {
                                entity: entity(id, &actor),
                                distance,
                            },
                        });
                    Ok(ObservationData::Target(Arc::new(hit)))
                })?
            }
        };
        if !self.lease.open() {
            return Err(Error::Detached);
        }
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_keeps_original_age_and_arc_until_the_game_sample_changes() {
        let pool = Pool::new();
        let source = Source {
            topic: Topic::Entities,
            lease: pool.acquire(Resources::default(), |_| Ok(())).unwrap(),
            cache: Mutex::new(None),
        };
        let key = (World(1, 2), 100, 0);
        let first = source
            .cached(key, 100, || {
                Ok(ObservationData::Entities(Arc::new(Vec::new())))
            })
            .unwrap();
        let second = source
            .cached(key, 200, || {
                panic!("unchanged samples must not allocate or convert again")
            })
            .unwrap();
        assert_eq!(second.at_ms, 100);
        if let (ObservationData::Entities(a), ObservationData::Entities(b)) =
            (first.data, second.data)
        {
            assert!(Arc::ptr_eq(&a, &b));
        } else {
            panic!("wrong sample kind");
        }
        let new_world = source
            .cached((World(3, 4), 100, 0), 100, || {
                Ok(ObservationData::Entities(Arc::new(Vec::new())))
            })
            .unwrap();
        assert_eq!(new_world.at_ms, 100);
        assert!(pool.close());
        assert!(matches!(source.read(World(3, 4)), Err(Error::Detached)));
    }
    #[test]
    fn conversion_keeps_entity_generation_names_and_item_slots_owned() {
        let actor = native::actors::Actor {
            name: "cow".into(),
            display_name: Some("名前".into()),
            actor_type: 11,
            health: Some((5., 10.)),
            bounds: Some([-1., 64., 2., 1., 66., 4.]),
            local: false,
        };
        let value = entity(0x40002, &actor);
        assert_eq!(value.id, 0x40002);
        assert_eq!(value.name(), "名前");
        assert_eq!(value.position(), Some(Vec3::new(0., 64., 3.)));
        assert_eq!(
            value.health,
            Some(Health {
                current: 5.,
                maximum: 10.
            })
        );
        let stack = item(&native::items::Item {
            name: "stone_pickaxe".into(),
            count: 1,
            damage: 30,
            max_damage: 131,
        });
        assert_eq!(stack.remaining_durability(), Some(101));
        drop(actor);
        assert_eq!(&*value.identifier, "cow");
    }
}
