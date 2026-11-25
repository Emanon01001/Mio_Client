//! Live render camera and renderer-object validation.
use crate::game::{
    fresh, layout,
    layout::{image, Layout},
    read, readable, CAMERA_AT, CLIENT,
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The render camera: position, basis vectors in world space, and the
/// projection's X/Y scale (1 / tan of half the field of view).
pub struct Camera {
    pub origin: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub back: [f32; 3],
    pub scale: [f32; 2],
}

/// Vtables of the LevelRenderer and LevelRendererPlayer, learned on the game
/// thread so the render thread can tell a live object from a stale pointer.
static RENDERER_VTABLE: AtomicUsize = AtomicUsize::new(0);
static RENDERER_PLAYER_VTABLE: AtomicUsize = AtomicUsize::new(0);

/// The LevelRendererPlayer vtable learned on the game thread (0 until then).
pub(in crate::game) fn renderer_player_vtable() -> usize {
    RENDERER_PLAYER_VTABLE.load(Ordering::Relaxed)
}

/// A pointer to a live C++ object: readable, with a vtable inside the game image.
unsafe fn object_vtable(addr: usize, len: usize) -> Option<usize> {
    if !readable(addr, len) {
        return None;
    }
    let vtable: usize = read(addr);
    let (base, end) = image();
    (base..end).contains(&vtable).then_some(vtable)
}

/// Records the renderer objects' vtables for `camera()`.
pub(in crate::game) unsafe fn learn_renderer(l: &Layout, instance: usize) {
    let found = (|| {
        if !readable(instance + l.level_renderer, 8) {
            return None;
        }
        let renderer: usize = read(instance + l.level_renderer);
        let renderer_vtable = object_vtable(renderer, l.renderer_player + 8)?;
        let player: usize = read(renderer + l.renderer_player);
        let len = l.camera_origin.max(l.projection_x).max(l.projection_y) + 12;
        Some((renderer_vtable, object_vtable(player, len)?))
    })();
    let (renderer, player) = found.unwrap_or((0, 0));
    RENDERER_VTABLE.store(renderer, Ordering::Relaxed);
    RENDERER_PLAYER_VTABLE.store(player, Ordering::Relaxed);
}

/// Reads an object pointer and checks it is alive: readable for `len` bytes
/// and still carrying the vtable learned on the game thread.
fn object(slot: usize, vtable: usize, len: usize) -> Option<usize> {
    let addr: usize = unsafe { read(slot) };
    (vtable != 0 && readable(addr, len) && unsafe { read::<usize>(addr) } == vtable).then_some(addr)
}

/// The camera as of this frame, if in a world.
///
/// Called from the render thread, so nothing here may be assumed alive: the
/// renderer objects are re-validated on every call.
pub fn camera() -> Option<Camera> {
    let l = layout().ok()?;
    // Camera-only overlays also keep renderer validation up to date.
    if !fresh(CAMERA_AT.load(Ordering::Relaxed)) {
        return None;
    }
    let instance = CLIENT.load(Ordering::Relaxed);
    if !readable(instance, l.view_matrix.max(l.level_renderer) + 48) {
        return None;
    }
    let renderer = object(
        instance + l.level_renderer,
        RENDERER_VTABLE.load(Ordering::Relaxed),
        l.renderer_player + 8,
    )?;
    let player = object(
        renderer + l.renderer_player,
        RENDERER_PLAYER_VTABLE.load(Ordering::Relaxed),
        l.camera_origin.max(l.projection_x).max(l.projection_y) + 12,
    )?;
    // A torn read of these plain floats only skews one frame.
    let (m, origin, scale_x, scale_y): ([f32; 12], [f32; 3], f32, f32) = unsafe {
        (
            read(instance + l.view_matrix),
            read(player + l.camera_origin),
            read(player + l.projection_x),
            read(player + l.projection_y),
        )
    };
    let camera = Camera {
        origin,
        right: [m[0], m[1], m[2]],
        up: [m[4], m[5], m[6]],
        back: [m[8], m[9], m[10]],
        scale: [scale_x, scale_y],
    };
    // A wrong offset shows up here: these would not be unit vectors.
    let unit = |v: &[f32; 3]| (v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 0.01;
    let sane = unit(&camera.right)
        && unit(&camera.up)
        && unit(&camera.back)
        && origin.iter().all(|v| v.is_finite())
        && (0.05..20.0).contains(&scale_x)
        && (0.05..20.0).contains(&scale_y);
    sane.then_some(camera)
}
