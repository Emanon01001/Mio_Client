use mcbe_sdk::{
    bridge::{Bridge, ObservationBackend, ObservationSample, ObservationSource, World},
    *,
};
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};

type Read = Arc<dyn Fn(World) -> Result<ObservationSample> + Send + Sync>;
struct Backend {
    read: Read,
    drops: Arc<AtomicUsize>,
}
struct Source {
    read: Read,
    drops: Arc<AtomicUsize>,
}
impl ObservationBackend for Backend {
    fn subscribe(&self, _: Topic) -> Result<Box<dyn ObservationSource>> {
        Ok(Box::new(Source {
            read: self.read.clone(),
            drops: self.drops.clone(),
        }))
    }
}
impl ObservationSource for Source {
    fn read(&self, world: World) -> Result<ObservationSample> {
        (self.read)(world)
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
fn player() -> Player {
    Player {
        position: Vec3::new(1., 64., 2.),
        eye_position: Vec3::new(1., 65.62, 2.),
        velocity: Vec3::default(),
        rotation: Rotation::default(),
    }
}
fn backend(
    bridge: &Bridge,
    read: impl Fn(World) -> Result<ObservationSample> + Send + Sync + 'static,
) -> Arc<AtomicUsize> {
    let drops = Arc::new(AtomicUsize::new(0));
    bridge.set_observation_backend(Arc::new(Backend {
        read: Arc::new(read),
        drops: drops.clone(),
    }));
    drops
}
fn data(topic: Topic) -> ObservationData {
    match topic {
        Topic::Entities => ObservationData::Entities(Arc::new(Vec::new())),
        Topic::Inventory => ObservationData::Inventory(Arc::new(Inventory {
            selected: 0,
            slots: vec![None; 36],
            armor: vec![None; 4],
        })),
        Topic::Vitals => ObservationData::Vitals(Arc::new(Vitals::default())),
        Topic::Status => ObservationData::Status(Arc::new(Status {
            effects: Some(Arc::from([])),
            air: None,
        })),
        Topic::World => ObservationData::World(Arc::new(WorldInfo {
            ticks: 0,
            rain: 0.,
            thunder: 0.,
            dimension: Dimension::Overworld,
        })),
        Topic::Target => ObservationData::Target(Arc::new(None)),
    }
}

#[test]
fn optional_backend_does_not_change_old_player_or_command_api() {
    let (game, bridge) = Bridge::new();
    assert!(matches!(
        game.observe(Topic::Inventory),
        Err(Error::Unavailable(_))
    ));
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(game.player().unwrap(), player());
    assert!(game.click().is_ok());
    bridge.close();
    assert!(matches!(
        game.observe(Topic::Inventory),
        Err(Error::Detached)
    ));
}
#[test]
fn subscribe_before_world_and_read_while_commands_are_paused() {
    let (game, bridge) = Bridge::new();
    let drops = backend(&bridge, |w| {
        assert_eq!(w, World(1, 2));
        Ok(ObservationSample {
            at_ms: 0,
            data: data(Topic::Inventory),
        })
    });
    let inventory = game.observe(Topic::Inventory).unwrap();
    assert_eq!(inventory.inventory(), Err(Error::NoWorld));
    bridge.publish(Some(World(1, 2)), Some(player()), false);
    assert!(inventory.inventory().is_ok());
    assert!(matches!(game.click(), Err(Error::Paused)));
    drop(inventory);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
#[test]
fn all_six_topics_are_typed_and_preserve_empty_vs_unavailable() {
    let (game, bridge) = Bridge::with_clock(|| 100);
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    for topic in [
        Topic::Entities,
        Topic::Inventory,
        Topic::Vitals,
        Topic::Status,
        Topic::World,
        Topic::Target,
    ] {
        backend(&bridge, move |_| {
            Ok(ObservationSample {
                at_ms: 75,
                data: data(topic),
            })
        });
        let reader = game.observe(topic).unwrap();
        assert_eq!(reader.topic(), topic);
        assert_eq!(reader.read().unwrap().age_ms, 25);
        assert_eq!(reader.read().unwrap().data.topic(), topic);
        match topic {
            Topic::Entities => assert!(reader.entities().unwrap().is_empty()),
            Topic::Inventory => assert_eq!(reader.inventory().unwrap().slots.len(), 36),
            Topic::Vitals => assert_eq!(reader.vitals().unwrap().health, 0.),
            Topic::Status => {
                let s = reader.status().unwrap();
                assert!(s.effects.as_ref().unwrap().is_empty());
                assert!(s.air.is_none());
            }
            Topic::World => assert_eq!(reader.world().unwrap().clock(), (6, 0)),
            Topic::Target => assert!(reader.target().unwrap().is_none()),
        }
        if topic != Topic::Inventory {
            assert_eq!(reader.inventory(), Err(Error::InvalidValue));
        }
    }
}
#[test]
fn malformed_backend_topic_is_rejected() {
    let (game, bridge) = Bridge::with_clock(|| 100);
    backend(&bridge, |_| {
        Ok(ObservationSample {
            at_ms: 100,
            data: data(Topic::World),
        })
    });
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(
        game.observe(Topic::Vitals).unwrap().vitals(),
        Err(Error::InvalidValue)
    );
}
#[test]
fn snapshot_age_uses_original_timestamp_and_rejects_future_clock() {
    static NOW: AtomicU64 = AtomicU64::new(500);
    let (game, bridge) = Bridge::with_clock(|| NOW.load(Ordering::SeqCst));
    backend(&bridge, |_| {
        Ok(ObservationSample {
            at_ms: 500,
            data: data(Topic::Entities),
        })
    });
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    let reader = game.observe(Topic::Entities).unwrap();
    assert_eq!(reader.read().unwrap().age_ms, 0);
    NOW.store(1500, Ordering::SeqCst);
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(reader.read().unwrap().age_ms, 1000);
    NOW.store(1501, Ordering::SeqCst);
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(reader.read(), Err(Error::StaleState));
    NOW.store(499, Ordering::SeqCst);
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(reader.read(), Err(Error::StaleState));
}
#[test]
fn world_leaving_and_returning_during_a_read_is_detected() {
    let (game, bridge) = Bridge::with_clock(|| 100);
    let producer = bridge.clone();
    backend(&bridge, move |_| {
        // Re-entering publish is also a regression check for lock inversion.
        producer.publish(None, None, false);
        producer.publish(Some(World(1, 2)), Some(player()), true);
        Ok(ObservationSample {
            at_ms: 100,
            data: data(Topic::Entities),
        })
    });
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    assert_eq!(
        game.observe(Topic::Entities).unwrap().read(),
        Err(Error::WorldChanged)
    );
    bridge.close(); // Break the mock backend's intentionally cyclic producer handle.
}
#[test]
fn concurrent_world_transition_during_a_read_is_detected() {
    let (game, bridge) = Bridge::with_clock(|| 100);
    let entered = Arc::new(std::sync::Barrier::new(2));
    let resumed = Arc::new(std::sync::Barrier::new(2));
    let (start, end) = (entered.clone(), resumed.clone());
    backend(&bridge, move |_| {
        start.wait();
        end.wait();
        Ok(ObservationSample {
            at_ms: 100,
            data: data(Topic::Target),
        })
    });
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    let reader = game.observe(Topic::Target).unwrap();
    let worker = std::thread::spawn(move || reader.read());
    entered.wait();
    bridge.publish(Some(World(3, 4)), Some(player()), true);
    resumed.wait();
    assert_eq!(worker.join().unwrap(), Err(Error::WorldChanged));
}
#[test]
fn detached_observer_cannot_read_but_owned_samples_remain_valid() {
    let (game, bridge) = Bridge::with_clock(|| 100);
    backend(&bridge, |_| {
        Ok(ObservationSample {
            at_ms: 100,
            data: data(Topic::Inventory),
        })
    });
    bridge.publish(Some(World(1, 2)), Some(player()), true);
    let reader = game.observe(Topic::Inventory).unwrap();
    let inventory = reader.inventory().unwrap();
    bridge.close();
    assert_eq!(reader.inventory(), Err(Error::Detached));
    assert_eq!(inventory.slots.len(), 36);
}
#[test]
fn backend_destructors_may_reenter_sdk_when_replaced_or_closed() {
    struct Reentrant {
        game: Game,
        drops: Arc<AtomicUsize>,
    }
    impl ObservationBackend for Reentrant {
        fn subscribe(&self, _: Topic) -> Result<Box<dyn ObservationSource>> {
            Err(Error::InvalidValue)
        }
    }
    impl Drop for Reentrant {
        fn drop(&mut self) {
            let _ = self.game.player();
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (game, bridge) = Bridge::new();
    let drops = Arc::new(AtomicUsize::new(0));
    bridge.set_observation_backend(Arc::new(Reentrant {
        game: game.clone(),
        drops: drops.clone(),
    }));
    bridge.set_observation_backend(Arc::new(Reentrant {
        game: game.clone(),
        drops: drops.clone(),
    }));
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    bridge.close();
    assert_eq!(drops.load(Ordering::SeqCst), 2);
    bridge.set_observation_backend(Arc::new(Reentrant {
        game,
        drops: drops.clone(),
    }));
    assert_eq!(drops.load(Ordering::SeqCst), 3);
}
#[test]
fn item_helpers_preserve_namespaces_slots_and_durability_semantics() {
    let stack = |id: &str, count, damage, max_damage| {
        Some(ItemStack {
            identifier: id.into(),
            count,
            damage,
            max_damage,
        })
    };
    let inventory = Inventory {
        selected: 0,
        slots: vec![
            stack("bread", 10, 0, 0),
            None,
            stack("minecraft:bread", 5, 0, 0),
            stack("custom:bread", 4, 0, 0),
        ],
        armor: vec![stack("bread", 99, 0, 0)],
    };
    assert_eq!(inventory.count("bread"), 15);
    assert_eq!(inventory.count("minecraft:bread"), 15);
    assert_eq!(inventory.count("custom:bread"), 4);
    assert_eq!(inventory.held().unwrap().remaining_durability(), None);
    let damaged = stack("wooden_hoe", 1, 70, 59).unwrap();
    assert_eq!(damaged.remaining_durability(), Some(0));
    assert!(Inventory {
        selected: 9,
        ..inventory
    }
    .held()
    .is_none());
}
#[test]
fn world_clock_and_entity_positions_use_bedrock_units() {
    let world = WorldInfo {
        ticks: -1,
        rain: 0.,
        thunder: 0.,
        dimension: Dimension::Overworld,
    };
    assert_eq!(world.day(), 0);
    assert_eq!(world.clock(), (5, 59));
    assert_eq!(
        WorldInfo {
            ticks: 18000,
            ..world
        }
        .clock(),
        (0, 0)
    );
    let bounds = Bounds {
        min: Vec3::new(-2., 64., -4.),
        max: Vec3::new(0., 66., 0.),
    };
    assert_eq!(bounds.position(), Vec3::new(-1., 64., -2.));
}
