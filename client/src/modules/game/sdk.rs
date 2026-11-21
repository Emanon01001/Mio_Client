//! Publish safe SDK snapshots and execute queued velocity commands.
use super::{
    layout,
    movement::{STATE_VELOCITY, TICKS_PER_SECOND},
    read, readable,
};
use std::ffi::c_void;
use windows_sys::Win32::System::Memory::{
    VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_READWRITE,
};

/// Copy typed SDK data on the game thread. Invalid/retired worlds invalidate
/// queued commands before any write or native action can run.
pub(super) unsafe fn sdk_frame(
    instance: usize,
    player: usize,
    active: bool,
) -> Vec<mcbe_sdk::bridge::Pending> {
    use mcbe_sdk::{bridge::World, Player, Rotation, Vec3};
    let found = (|| {
        let l = layout().ok()?;
        if player == 0
            || !readable(
                player,
                l.actor_state.max(l.actor_aabb).max(l.actor_registry) + 8,
            )
        {
            return None;
        }
        let state: usize = read(player + l.actor_state);
        let bounds: usize = read(player + l.actor_aabb);
        if !readable(state, 36) || !readable(bounds, 24) || !readable(instance + l.view_matrix, 48)
        {
            return None;
        }
        let eye: [f32; 3] = read(state);
        let feet: f32 = read(bounds + 4);
        let velocity: [f32; 3] = read(state + STATE_VELOCITY);
        let view: [f32; 12] = read(instance + l.view_matrix);
        let rotation = Rotation::from_direction(Vec3::new(-view[8], -view[9], -view[10]))?;
        let registry: usize = read(player + l.actor_registry);
        let entity: u32 = read(player + 0x18);
        Some((
            World(registry, entity),
            Player {
                position: Vec3::new(eye[0], feet, eye[2]),
                eye_position: eye.into(),
                velocity: velocity.map(|v| v * TICKS_PER_SECOND).into(),
                rotation,
            },
        ))
    })();
    let (world, snapshot) = match found {
        Some((world, snapshot)) => (Some(world), Some(snapshot)),
        None => (None, None),
    };
    crate::api::frame(world, snapshot, active)
}

pub(super) unsafe fn sdk_velocity(player: usize, velocity: mcbe_sdk::Vec3) -> mcbe_sdk::Result<()> {
    let l = layout().map_err(mcbe_sdk::Error::Unavailable)?;
    if player == 0 || !readable(player + l.actor_state, 8) {
        return Err(mcbe_sdk::Error::NoWorld);
    }
    let state: usize = read(player + l.actor_state);
    let target = state + STATE_VELOCITY;
    let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
    if VirtualQuery(
        target as *const c_void,
        &mut info,
        std::mem::size_of_val(&info),
    ) == 0
        || info.State != MEM_COMMIT
        || info.Protect & PAGE_READWRITE == 0
        || !readable(target, 12)
    {
        return Err(mcbe_sdk::Error::Unavailable(
            "velocity is not writable".into(),
        ));
    }
    (target as *mut [f32; 3])
        .write_unaligned(<[f32; 3]>::from(velocity).map(|v| v / TICKS_PER_SECOND));
    Ok(())
}
