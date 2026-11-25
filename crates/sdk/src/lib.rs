//! Thread-safe Minecraft state and commands. The injected host supplies the
//! native backend through [`bridge`]; callers never receive native pointers.
//! Positions are blocks, velocities blocks/second, angles degrees.
pub mod bridge;
mod data;
mod observation;
mod state;
pub use data::*;
pub use observation::*;
pub use state::*;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

const SNAPSHOT_AGE_MS: u64 = 1000;
const COMMAND_AGE_MS: u64 = 500;
const MAX_COMMANDS: usize = 128;
const MAX_MESSAGES: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Detached,
    NoWorld,
    Paused,
    StaleState,
    QueueFull,
    InvalidValue,
    Expired,
    WorldChanged,
    Abandoned,
    Unavailable(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(reason) => f.write_str(reason),
            other => write!(f, "{other:?}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Completion {
    Pending,
    Done,
    Failed(Error),
}

/// Queuing a command does not mean it ran. Check the receipt after a game tick.
#[derive(Clone, Debug)]
pub struct Receipt(Arc<Mutex<Completion>>);
impl Receipt {
    pub fn status(&self) -> Completion {
        lock(&self.0).clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatDirection {
    Sent,
    Received,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: u64,
    pub direction: ChatDirection,
    pub sender: Option<String>,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ChatBatch {
    pub messages: Vec<ChatMessage>,
    /// Pass this value to the next `chat_since` call.
    pub cursor: u64,
    /// Older messages were discarded before this reader caught up.
    pub missed: bool,
}

/// Cloneable handle usable from module, render, or worker threads.
#[derive(Clone)]
pub struct Game(Arc<Mutex<Shared>>);
impl Game {
    pub fn player(&self) -> Result<Player> {
        let shared = lock(&self.0);
        if !shared.attached {
            return Err(Error::Detached);
        }
        let (player, at) = shared.player.ok_or(Error::NoWorld)?;
        if (shared.clock)().saturating_sub(at) > SNAPSHOT_AGE_MS {
            return Err(Error::StaleState);
        }
        Ok(player)
    }
    /// Native left attack/destroy click, executed once by the game input tick.
    pub fn click(&self) -> Result<Receipt> {
        self.enqueue(bridge::Command::Click)
    }
    /// Low-level velocity override, in blocks/second. Uses the game's state
    /// vector; server-authoritative worlds may correct this movement.
    pub fn set_velocity(&self, velocity: Vec3) -> Result<Receipt> {
        if !velocity.is_finite() {
            return Err(Error::InvalidValue);
        }
        self.enqueue(bridge::Command::SetVelocity(velocity))
    }
    fn enqueue(&self, command: bridge::Command) -> Result<Receipt> {
        let mut shared = lock(&self.0);
        shared.ready()?;
        if let Some(error) = &shared.command_error {
            return Err(Error::Unavailable(error.clone()));
        }
        if shared.commands.len() >= MAX_COMMANDS {
            return Err(Error::QueueFull);
        }
        let receipt = Receipt(Arc::new(Mutex::new(Completion::Pending)));
        let world = shared.world.ok_or(Error::NoWorld)?;
        let queued = (shared.clock)();
        shared.commands.push_back(bridge::Pending {
            command,
            receipt: receipt.clone(),
            world,
            queued,
        });
        Ok(receipt)
    }
    /// Captured sent/received chat bodies, starting at attachment time.
    /// Each reader owns a cursor, so readers do not consume each other's messages.
    pub fn chat_since(&self, cursor: u64) -> Result<ChatBatch> {
        let shared = lock(&self.0);
        if !shared.attached {
            return Err(Error::Detached);
        }
        if let Some(error) = &shared.chat_error {
            return Err(Error::Unavailable(error.clone()));
        }
        let oldest = shared
            .messages
            .front()
            .map_or(shared.message_id + 1, |m| m.id);
        if cursor >= shared.message_id {
            return Ok(ChatBatch {
                messages: Vec::new(),
                cursor: shared.message_id,
                missed: false,
            });
        }
        Ok(ChatBatch {
            messages: shared
                .messages
                .iter()
                .filter(|m| m.id > cursor)
                .cloned()
                .collect(),
            cursor: shared.message_id,
            missed: cursor < oldest.saturating_sub(1),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
struct Shared {
    attached: bool,
    active: bool,
    world: Option<bridge::World>,
    player: Option<(Player, u64)>,
    commands: VecDeque<bridge::Pending>,
    messages: VecDeque<ChatMessage>,
    message_id: u64,
    chat_error: Option<String>,
    command_error: Option<String>,
    observations: Option<Arc<dyn bridge::ObservationBackend>>,
    world_epoch: u64,
    clock: fn() -> u64,
}
impl Shared {
    fn readable(&self) -> Result<()> {
        if !self.attached {
            return Err(Error::Detached);
        }
        if self.world.is_none() {
            return Err(Error::NoWorld);
        }
        if self
            .player
            .is_none_or(|(_, at)| (self.clock)().saturating_sub(at) > SNAPSHOT_AGE_MS)
        {
            return Err(Error::StaleState);
        }
        Ok(())
    }
    fn ready(&self) -> Result<()> {
        self.readable()?;
        if !self.active {
            return Err(Error::Paused);
        }
        Ok(())
    }
    fn reject_pending(&mut self, error: Error) {
        for pending in self.commands.drain(..) {
            pending.fail(error.clone());
        }
    }
}
