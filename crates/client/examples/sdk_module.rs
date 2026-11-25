//! Compile with `cargo build -p client --example sdk_module`.
//! Reusable module functions, called by an injected host after Runtime::attach.
use Mio_Client::api::{self, ChatDirection, Game, Observation, Receipt, Topic, Vec3};

/// Construct on a worker after Runtime::attach; retain across updates. Dropping
/// this value releases the samplers. These subscriptions do not enable HUDs.
pub struct Inspector {
    pub entities: Observation,
    pub inventory: Observation,
    pub vitals: Observation,
    pub status: Observation,
    pub world: Observation,
    pub target: Observation,
}
impl Inspector {
    pub fn new(game: &Game) -> api::Result<Self> {
        Ok(Self {
            entities: game.observe(Topic::Entities)?,
            inventory: game.observe(Topic::Inventory)?,
            vitals: game.observe(Topic::Vitals)?,
            status: game.observe(Topic::Status)?,
            world: game.observe(Topic::World)?,
            target: game.observe(Topic::Target)?,
        })
    }
    /// Call on a later update, after the native samplers have produced data.
    pub fn inspect(&self) -> api::Result<()> {
        let inventory = self.inventory.inventory()?;
        let world = self.world.world()?;
        println!(
            "entities={} held={:?} bread={} armor={:?}",
            self.entities.entities()?.len(),
            inventory.held(),
            inventory.count("bread"),
            inventory.armor
        );
        println!(
            "vitals={:?} status={:?} time={:?} target={:?}",
            self.vitals.vitals()?,
            self.status.status()?,
            world.clock(),
            self.target.target()?
        );
        Ok(())
    }
}

pub fn inspect(game: &Game, cursor: &mut u64) -> api::Result<()> {
    let player = game.player()?;
    println!(
        "position={:?}, speed={:.2} blocks/s, yaw={:.1}, pitch={:.1}, direction={:?}",
        player.position,
        player.horizontal_speed(),
        player.rotation.yaw,
        player.rotation.pitch,
        player.direction()
    );
    let batch = game.chat_since(*cursor)?;
    for message in batch.messages {
        let direction = match message.direction {
            ChatDirection::Sent => "sent",
            ChatDirection::Received => "received",
        };
        println!("[{direction}] {}", message.text);
    }
    *cursor = batch.cursor;
    Ok(())
}

pub fn attack_once(game: &Game) -> api::Result<Receipt> {
    game.click()
}

pub fn move_forward_once(game: &Game, blocks_per_second: f32) -> api::Result<Receipt> {
    let direction = game.player()?.direction();
    let horizontal = Vec3::new(direction.x, 0., direction.z)
        .normalized()
        .ok_or(api::Error::InvalidValue)?;
    game.set_velocity(Vec3::new(
        horizontal.x * blocks_per_second,
        0.,
        horizontal.z * blocks_per_second,
    ))
}
