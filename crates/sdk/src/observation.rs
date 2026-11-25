//! Opt-in observation handles. Dropping the handle releases its subscription.
use crate::{bridge, lock, Error, Game, Result, SNAPSHOT_AGE_MS};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    Entities,
    Inventory,
    Vitals,
    Status,
    World,
    Target,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ObservationData {
    /// Local player excluded; sorted by entity ID.
    Entities(Arc<Vec<crate::Entity>>),
    Inventory(Arc<crate::Inventory>),
    Vitals(Arc<crate::Vitals>),
    Status(Arc<crate::Status>),
    World(Arc<crate::WorldInfo>),
    /// None means a fresh ray hit nothing; it is not missing data.
    Target(Arc<Option<crate::Target>>),
}
impl ObservationData {
    pub fn topic(&self) -> Topic {
        match self {
            Self::Entities(_) => Topic::Entities,
            Self::Inventory(_) => Topic::Inventory,
            Self::Vitals(_) => Topic::Vitals,
            Self::Status(_) => Topic::Status,
            Self::World(_) => Topic::World,
            Self::Target(_) => Topic::Target,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub data: ObservationData,
    /// Age of the original game sample, not the time of this getter call.
    pub age_ms: u64,
}

/// Thread-safe read-only subscription. Create once, retain across frames, and
/// drop when unused. Creating a new handle may initialise a native sampler.
pub struct Observation {
    game: Game,
    topic: Topic,
    source: Box<dyn bridge::ObservationSource>,
}
impl Game {
    /// Subscribe before reading. Works before entering a world; read() returns
    /// NoWorld until a valid player sample exists. Does not enable any HUD.
    pub fn observe(&self, topic: Topic) -> Result<Observation> {
        let backend = {
            let shared = lock(&self.0);
            if !shared.attached {
                return Err(Error::Detached);
            }
            shared
                .observations
                .clone()
                .ok_or_else(|| Error::Unavailable("observation backend has not started".into()))?
        };
        // Never call a native backend while holding the SDK state lock.
        let source = backend.subscribe(topic)?;
        if !lock(&self.0).attached {
            return Err(Error::Detached);
        }
        Ok(Observation {
            game: self.clone(),
            topic,
            source,
        })
    }
}
impl Observation {
    pub fn topic(&self) -> Topic {
        self.topic
    }
    /// Old worlds, stale samples and shutdown are errors, including while
    /// another thread is reading or changing worlds. Paused commands do not
    /// prevent read-only access to still-fresh data.
    pub fn read(&self) -> Result<Reading> {
        let (world, epoch) = {
            let shared = lock(&self.game.0);
            shared.readable()?;
            (shared.world.ok_or(Error::NoWorld)?, shared.world_epoch)
        };
        let sample = self.source.read(world)?;
        let shared = lock(&self.game.0);
        shared.readable()?;
        if shared.world != Some(world) || shared.world_epoch != epoch {
            return Err(Error::WorldChanged);
        }
        if sample.data.topic() != self.topic {
            return Err(Error::InvalidValue);
        }
        let now = (shared.clock)();
        let age_ms = now.checked_sub(sample.at_ms).ok_or(Error::StaleState)?;
        if age_ms > SNAPSHOT_AGE_MS {
            return Err(Error::StaleState);
        }
        Ok(Reading {
            data: sample.data,
            age_ms,
        })
    }
}

macro_rules! getter {
    ($method:ident, $variant:ident, $type:ty) => {
        impl Observation {
            pub fn $method(&self) -> Result<Arc<$type>> {
                if self.topic != Topic::$variant {
                    return Err(Error::InvalidValue);
                }
                match self.read()?.data {
                    ObservationData::$variant(value) => Ok(value),
                    _ => Err(Error::InvalidValue),
                }
            }
        }
    };
}
getter!(entities, Entities, Vec<crate::Entity>);
getter!(inventory, Inventory, crate::Inventory);
getter!(vitals, Vitals, crate::Vitals);
getter!(status, Status, crate::Status);
getter!(world, World, crate::WorldInfo);
getter!(target, Target, Option<crate::Target>);
