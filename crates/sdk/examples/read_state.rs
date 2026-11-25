//! Uses a simulated host so the SDK example can run without Minecraft.
use mio_sdk::{
    bridge::{Bridge, World},
    Player, Rotation, Vec3,
};
fn main() -> mio_sdk::Result<()> {
    let (game, backend) = Bridge::new();
    backend.publish(
        Some(World(1, 1)),
        Some(Player {
            position: Vec3::new(10., 64., 20.),
            eye_position: Vec3::new(10., 65.62, 20.),
            velocity: Vec3::new(0., 0., 4.3),
            rotation: Rotation { yaw: 0., pitch: 0. },
        }),
        true,
    );
    let player = game.player()?;
    println!(
        "position={:?} speed={} blocks/s direction={:?}",
        player.position,
        player.speed(),
        player.direction()
    );
    Ok(())
}
