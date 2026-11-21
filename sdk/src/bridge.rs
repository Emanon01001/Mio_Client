//! Host/backend integration. Module authors should use `Game` instead.
use super::*;

/// Host extension for optional read-only samplers. Subscribe outside DllMain.
pub trait ObservationBackend: Send + Sync {
    fn subscribe(&self, topic: Topic) -> Result<Box<dyn ObservationSource>>;
}
/// Implementations return owned data and verify its registry/player identity.
/// A source must release its sampler when dropped, or retain failed cleanup
/// for the host's shutdown retry. No native calls on the render/game thread
/// are required to read a sample.
pub trait ObservationSource: Send + Sync {
    fn read(&self, world: World) -> Result<ObservationSample>;
}
#[derive(Clone, Debug)]
pub struct ObservationSample {
    pub at_ms: u64,
    pub data: ObservationData,
}

/// A world/player identity; never dereferenced by the SDK.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct World(pub usize, pub u32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Click,
    SetVelocity(Vec3),
}
pub struct Pending {
    pub command: Command,
    pub(crate) receipt: Receipt,
    pub(crate) world: World,
    pub(crate) queued: u64,
}
impl Pending {
    pub fn complete(self, result: Result<()>) {
        *lock(&self.receipt.0) = match result {
            Ok(()) => Completion::Done,
            Err(error) => Completion::Failed(error),
        };
    }
    pub(crate) fn fail(self, error: Error) {
        self.complete(Err(error));
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let mut status = lock(&self.receipt.0);
        if *status == Completion::Pending {
            *status = Completion::Failed(Error::Abandoned);
        }
    }
}

#[derive(Clone)]
pub struct Bridge(Arc<Mutex<Shared>>);
impl Bridge {
    pub fn new() -> (Game, Self) {
        Self::with_clock(|| {
            static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            START.get_or_init(Instant::now).elapsed().as_millis() as u64
        })
    }
    /// Supply an unscaled monotonic clock so game timer hooks do not affect expiry.
    pub fn with_clock(clock: fn() -> u64) -> (Game, Self) {
        let shared = Arc::new(Mutex::new(Shared {
            attached: true,
            active: false,
            world: None,
            player: None,
            commands: VecDeque::new(),
            messages: VecDeque::new(),
            message_id: 0,
            chat_error: Some("chat backend has not started".into()),
            command_error: None,
            observations: None,
            world_epoch: 0,
            clock,
        }));
        (Game(Arc::clone(&shared)), Self(shared))
    }
    /// Publish from the native game thread, before draining commands.
    pub fn publish(&self, world: Option<World>, player: Option<Player>, active: bool) {
        publish(&mut lock(&self.0), world, player, active);
    }
    /// Publish and collect commands under one lock, with the same validation as
    /// `publish` followed by `drain`. Useful on every native input tick.
    pub fn publish_and_drain(
        &self,
        world: Option<World>,
        player: Option<Player>,
        active: bool,
    ) -> Vec<Pending> {
        let mut shared = lock(&self.0);
        publish(&mut shared, world, player, active);
        drain(&mut shared)
    }
    pub fn drain(&self) -> Vec<Pending> {
        drain(&mut lock(&self.0))
    }

    pub fn chat_available(&self, result: std::result::Result<(), String>) {
        lock(&self.0).chat_error = result.err();
    }
    pub fn commands_available(&self, result: std::result::Result<(), String>) {
        lock(&self.0).command_error = result.err();
    }
    /// Install an optional host backend. Existing SDK-only hosts need not
    /// provide one; player(), commands and chat keep their previous behaviour.
    pub fn set_observation_backend(&self, backend: Arc<dyn ObservationBackend>) {
        let retired = {
            let mut shared = lock(&self.0);
            if shared.attached {
                shared.observations.replace(backend)
            } else {
                Some(backend)
            }
        };
        // A backend destructor may call the SDK, just like subscribe/read.
        drop(retired);
    }
    pub fn push_chat(&self, direction: ChatDirection, sender: Option<String>, text: String) {
        let mut shared = lock(&self.0);
        if !shared.attached {
            return;
        }
        shared.message_id += 1;
        let id = shared.message_id;
        if shared.messages.len() >= MAX_MESSAGES {
            shared.messages.pop_front();
        }
        shared.messages.push_back(ChatMessage {
            id,
            direction,
            sender,
            text,
        });
    }
    pub fn close(&self) {
        let retired = {
            let mut shared = lock(&self.0);
            shared.attached = false;
            shared.player = None;
            shared.reject_pending(Error::Detached);
            shared.commands = VecDeque::new();
            shared.messages = VecDeque::new();
            shared.observations.take()
        };
        drop(retired);
    }
}

fn publish(shared: &mut Shared, world: Option<World>, player: Option<Player>, active: bool) {
    if !shared.attached {
        return;
    }
    if shared.world != world {
        shared.world_epoch = shared.world_epoch.wrapping_add(1);
        shared.reject_pending(Error::WorldChanged);
    }
    shared.world = world;
    shared.player = player
        .filter(|p| p.is_finite())
        .map(|p| (p, (shared.clock)()));
    shared.active = active;
    if !active {
        shared.reject_pending(Error::Paused);
    }
}
fn drain(shared: &mut Shared) -> Vec<Pending> {
    if let Err(error) = shared.ready() {
        shared.reject_pending(error);
        return Vec::new();
    }
    let mut ready = Vec::new();
    while let Some(pending) = shared.commands.pop_front() {
        if Some(pending.world) != shared.world {
            pending.fail(Error::WorldChanged);
        } else if (shared.clock)().saturating_sub(pending.queued) > COMMAND_AGE_MS {
            pending.fail(Error::Expired);
        } else {
            ready.push(pending);
        }
    }
    ready
}
