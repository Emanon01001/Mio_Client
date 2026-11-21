use mcbe_sdk::{
    bridge::{Bridge, Command, World},
    *,
};
use std::sync::atomic::{AtomicU64, Ordering};

fn player() -> Player {
    Player {
        position: Vec3::new(1., 64., 2.),
        eye_position: Vec3::new(1., 65.62, 2.),
        velocity: Vec3::new(3., 2., 4.),
        rotation: Rotation {
            yaw: 90.,
            pitch: 0.,
        },
    }
}
#[test]
fn state_units_and_view_convention() {
    let (game, backend) = Bridge::new();
    assert_eq!(game.player(), Err(Error::NoWorld));
    backend.publish(Some(World(10, 1)), Some(player()), true);
    let p = game.player().unwrap();
    assert_eq!(p.horizontal_speed(), 5.);
    assert!((p.speed() - 29f32.sqrt()).abs() < 1e-6);
    assert!((p.direction().x + 1.).abs() < 1e-6);
    for yaw in [-179., -90., 0., 90., 179.] {
        for pitch in [-60., 0., 60.] {
            let r = Rotation { yaw, pitch };
            let back = Rotation::from_direction(r.direction()).unwrap();
            assert!((back.yaw - yaw).abs() < 1e-3);
            assert!((back.pitch - pitch).abs() < 1e-3);
        }
    }
    assert!(Vec3::default().normalized().is_none());
}
#[test]
fn queued_click_and_velocity_have_explicit_completion() {
    let (game, backend) = Bridge::new();
    backend.publish(Some(World(10, 1)), Some(player()), true);
    let click = game.click().unwrap();
    let velocity = game.set_velocity(Vec3::new(0., 5., 0.)).unwrap();
    assert_eq!(click.status(), Completion::Pending);
    let mut work = backend.drain().into_iter();
    let first = work.next().unwrap();
    assert_eq!(first.command, Command::Click);
    first.complete(Ok(()));
    let second = work.next().unwrap();
    assert_eq!(second.command, Command::SetVelocity(Vec3::new(0., 5., 0.)));
    second.complete(Err(Error::Unavailable("native failure".into())));
    assert_eq!(click.status(), Completion::Done);
    assert_eq!(
        velocity.status(),
        Completion::Failed(Error::Unavailable("native failure".into()))
    );
    assert!(backend.drain().is_empty());
}
#[test]
fn pause_world_change_and_detach_cancel_old_work() {
    let (game, backend) = Bridge::new();
    backend.publish(Some(World(10, 1)), Some(player()), true);
    let old = game.click().unwrap();
    backend.publish(Some(World(10, 2)), Some(player()), true);
    assert_eq!(old.status(), Completion::Failed(Error::WorldChanged));
    let paused = game.click().unwrap();
    backend.publish(Some(World(10, 2)), Some(player()), false);
    assert_eq!(paused.status(), Completion::Failed(Error::Paused));
    assert!(matches!(game.click(), Err(Error::Paused)));
    backend.publish(Some(World(10, 2)), Some(player()), true);
    let pending = game.click().unwrap();
    backend.close();
    assert_eq!(pending.status(), Completion::Failed(Error::Detached));
    assert_eq!(game.player(), Err(Error::Detached));
    assert!(matches!(game.click(), Err(Error::Detached)));
}
#[test]
fn bounded_work_and_abandoned_native_commands() {
    let (game, backend) = Bridge::new();
    backend.publish(Some(World(10, 1)), Some(player()), true);
    let first = game.click().unwrap();
    for _ in 1..128 {
        game.click().unwrap();
    }
    assert!(matches!(game.click(), Err(Error::QueueFull)));
    drop(backend.drain());
    assert_eq!(first.status(), Completion::Failed(Error::Abandoned));
    assert!(matches!(
        game.set_velocity(Vec3::new(f32::NAN, 0., 0.)),
        Err(Error::InvalidValue)
    ));
}
#[test]
fn native_clock_controls_expiry_and_snapshot_age() {
    static NOW: AtomicU64 = AtomicU64::new(10);
    let (game, backend) = Bridge::with_clock(|| NOW.load(Ordering::Relaxed));
    backend.publish(Some(World(10, 1)), Some(player()), true);
    let receipt = game.click().unwrap();
    NOW.store(511, Ordering::Relaxed);
    assert!(backend.drain().is_empty());
    assert_eq!(receipt.status(), Completion::Failed(Error::Expired));
    NOW.store(1011, Ordering::Relaxed);
    assert_eq!(game.player(), Err(Error::StaleState));
}
#[test]
fn chat_cursors_are_independent_and_report_overflow() {
    let (game, backend) = Bridge::new();
    assert!(matches!(game.chat_since(0), Err(Error::Unavailable(_))));
    backend.chat_available(Ok(()));
    backend.push_chat(ChatDirection::Sent, None, "こんにちは".into());
    backend.push_chat(ChatDirection::Received, Some("Alex".into()), "hello".into());
    let a = game.chat_since(0).unwrap();
    let b = game.clone().chat_since(0).unwrap();
    assert_eq!(a.messages, b.messages);
    assert_eq!(a.messages[0].text, "こんにちは");
    assert!(game.chat_since(a.cursor).unwrap().messages.is_empty());
    for i in 0..512 {
        backend.push_chat(ChatDirection::Received, None, i.to_string());
    }
    let overflow = game.chat_since(0).unwrap();
    assert!(overflow.missed);
    assert_eq!(overflow.messages.len(), 512);
    backend.close();
    assert!(matches!(game.chat_since(0), Err(Error::Detached)));
}
#[test]
fn atomic_frame_preserves_world_pause_and_completion_rules() {
    let (game, backend) = Bridge::new();
    assert!(backend
        .publish_and_drain(Some(World(1, 1)), Some(player()), true)
        .is_empty());
    let old = game.click().unwrap();
    assert!(backend
        .publish_and_drain(Some(World(1, 2)), Some(player()), true)
        .is_empty());
    assert_eq!(old.status(), Completion::Failed(Error::WorldChanged));
    let ok = game.click().unwrap();
    let mut batch = backend.publish_and_drain(Some(World(1, 2)), Some(player()), true);
    assert_eq!(batch.len(), 1);
    batch.pop().unwrap().complete(Ok(()));
    assert_eq!(ok.status(), Completion::Done);
    let pending = game.click().unwrap();
    assert!(backend
        .publish_and_drain(Some(World(1, 2)), Some(player()), false)
        .is_empty());
    assert_eq!(pending.status(), Completion::Failed(Error::Paused));
    backend.close();
    assert!(backend
        .publish_and_drain(Some(World(1, 2)), Some(player()), true)
        .is_empty());
    assert!(matches!(game.click(), Err(Error::Detached)));
}
