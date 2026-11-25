//! A single cancellable request, executed only inside a live ScreenModel callback.
//! No native ScreenModel/controller/player pointer is saved across callbacks.
use super::{sent_hook, SENT};
use crate::{game, gui, input};
use mio_sdk::bridge::World;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
const REQUEST_MS: u64 = 500;
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static DEQUEUED: AtomicU64 = AtomicU64::new(0);
static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static ACCEPTED: AtomicU64 = AtomicU64::new(0);
static LAST_AGE: AtomicU64 = AtomicU64::new(0);
static WORKER_FLAGS: AtomicU64 = AtomicU64::new(0);
static WORKER_AT: AtomicU64 = AtomicU64::new(0);
static CHAT_AT: AtomicU64 = AtomicU64::new(0);
static CHAT_TICKS: AtomicU64 = AtomicU64::new(0);
static CHAT_SENDS: AtomicU64 = AtomicU64::new(0);
static LAST_RESULT: Mutex<Option<(u64, Outcome)>> = Mutex::new(None);
static ON: AtomicBool = AtomicBool::new(false);
static WORLD: Mutex<Option<(World, u64)>> = Mutex::new(None);
static QUEUE: Mutex<Option<Request>> = Mutex::new(None);
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Pending,
    Sent,
    Paused,
    Cancelled,
    Failed(String),
}
#[derive(Clone)]
pub(crate) struct Ticket(Arc<Mutex<Outcome>>);
impl Ticket {
    pub fn status(&self) -> Outcome {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn finish(&self, value: Outcome) {
        if value == Outcome::Sent {
            ACCEPTED.fetch_add(1, Ordering::Relaxed);
        }
        let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
        *LAST_RESULT.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, value.clone()));
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }
}
struct Lease {
    live: AtomicBool,
    until: AtomicU64,
    generation: AtomicU64,
}
pub(crate) struct Batch {
    world: World,
    lease: Arc<Lease>,
}
impl Batch {
    pub fn begin(now: u64) -> Result<Self, String> {
        let world = current_world(now).ok_or(
            "ワールドの取得を待っています。メニューを閉じて一度ワールドへ戻ってください。",
        )?;
        Ok(Self {
            world,
            lease: Arc::new(Lease {
                live: AtomicBool::new(true),
                until: AtomicU64::new(0),
                generation: AtomicU64::new(0),
            }),
        })
    }
    pub fn pause(&self) {
        if self.lease.until.swap(0, Ordering::AcqRel) != 0 {
            // Even after resume, a request from before the pause is obsolete.
            self.lease.generation.fetch_add(1, Ordering::AcqRel);
        }
        // Chat/inventory can stop HUD callbacks entirely. A queued request must
        // therefore be acknowledged here, without waiting for that callback.
        let withdrawn = QUEUE
            .try_lock()
            .ok()
            .and_then(|mut q| take_for_batch(&mut q, &self.lease));
        if let Some(r) = withdrawn {
            r.ticket.finish(if self.lease.live.load(Ordering::Acquire) {
                Outcome::Paused
            } else {
                Outcome::Cancelled
            });
        }
    }
    pub fn resume(&self, now: u64) {
        self.lease
            .until
            .store(now.saturating_add(100), Ordering::Release);
    }
    pub fn same_world(&self, now: u64) -> Option<bool> {
        current_world(now).map(|w| w == self.world)
    }
    pub fn submit(&self, text: &str, now: u64) -> Result<Ticket, String> {
        validate(text)?;
        if !ON.load(Ordering::Acquire) || !self.lease.live.load(Ordering::Acquire) {
            return Err("送信は停止しています".into());
        }
        let ticket = Ticket(Arc::new(Mutex::new(Outcome::Pending)));
        let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        if q.as_ref()
            .is_some_and(|r| r.lease.live.load(Ordering::Acquire))
        {
            return Err("前の送信処理を待っています".into());
        }
        if let Some(old) = q.take() {
            old.ticket.finish(Outcome::Cancelled);
        }
        *q = Some(Request {
            text: text.to_owned(),
            world: self.world,
            at: now,
            generation: self.lease.generation.load(Ordering::Acquire),
            lease: Arc::clone(&self.lease),
            ticket: ticket.clone(),
        });
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        Ok(ticket)
    }
}
impl Drop for Batch {
    fn drop(&mut self) {
        self.lease.live.store(false, Ordering::Release);
        self.pause();
    }
}
struct Request {
    text: String,
    world: World,
    at: u64,
    generation: u64,
    lease: Arc<Lease>,
    ticket: Ticket,
}
fn take_for_batch(queue: &mut Option<Request>, lease: &Arc<Lease>) -> Option<Request> {
    if queue.as_ref().is_some_and(|r| Arc::ptr_eq(&r.lease, lease)) {
        queue.take()
    } else {
        None
    }
}
pub fn validate(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("本文を入力してください".into());
    }
    if text.chars().count() > 256
        || text
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err("本文は改行なしの256文字以内にしてください".into());
    }
    if text.trim_start().starts_with('/') {
        return Err("コマンドではなく通常チャットの本文を指定してください".into());
    }
    Ok(())
}
pub fn available() -> Result<(), String> {
    if game::known_build() && crate::api::input_ready() && SENT.load(Ordering::Acquire) != 0 {
        game::inventory::available()
    } else {
        Err("通常チャットの送信処理を利用できません".into())
    }
}
pub fn set_active(on: bool) {
    ON.store(on, Ordering::Release);
    if !on {
        CHAT_AT.store(0, Ordering::Release);
        *WORLD.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if let Some(r) = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).take() {
            r.ticket.finish(Outcome::Cancelled);
        }
    }
}
fn current_world(now: u64) -> Option<World> {
    let g = WORLD.lock().ok()?;
    observed_world(*g, now)
}
/// The native observer can publish after the worker captured its tick time.
/// A slightly newer observation is fresh, rather than a reason to reset a timer.
pub(crate) fn observed_world(observation: Option<(World, u64)>, now: u64) -> Option<World> {
    let (w, at) = observation?;
    (now.saturating_sub(at) <= 1000).then_some(w)
}
fn decision(r: &Request, world: World, now: u64, active: bool) -> Option<Outcome> {
    if !r.lease.live.load(Ordering::Acquire) {
        Some(Outcome::Cancelled)
    } else if r.world != world {
        Some(Outcome::Failed("ワールドが変わったため停止しました".into()))
    } else if !active
        || r.generation != r.lease.generation.load(Ordering::Acquire)
        || now >= r.lease.until.load(Ordering::Acquire)
    {
        Some(Outcome::Paused)
    } else if now < r.at || now - r.at > REQUEST_MS {
        Some(Outcome::Failed("送信要求が期限切れになりました".into()))
    } else {
        None
    }
}
/// The native bool reports a permission denial, NOT successful submission.
/// Both builds return the permission flag; only zero takes the normal-send path.
/// Only AL is defined by that ABI, so ignore the upper bits of the hook result.
fn send_outcome(result: usize) -> Outcome {
    if result as u8 == 0 {
        Outcome::Sent
    } else {
        Outcome::Failed("ゲームが送信を受け付けませんでした".into())
    }
}
#[repr(C)]
struct NativeString {
    data: [u8; 16],
    len: usize,
    capacity: usize,
}
/// Borrowed const MSVC string. Long text is owned by Rust until the native
/// call returns; the native const-reference function never destroys this value.
fn with_string<R>(text: &str, call: impl FnOnce(usize) -> R) -> R {
    let mut value = NativeString {
        data: [0; 16],
        len: text.len(),
        capacity: 15,
    };
    let mut heap = Vec::new();
    if text.len() <= 15 {
        value.data[..text.len()].copy_from_slice(text.as_bytes());
    } else {
        heap.extend_from_slice(text.as_bytes());
        heap.push(0);
        value.data[..8].copy_from_slice(&(heap.as_ptr() as usize).to_le_bytes());
        value.capacity = text.len();
    }
    let result = call(&value as *const _ as usize);
    drop(heap);
    result
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    World,
    Chat,
}
pub(crate) fn chat_active(now: u64) -> bool {
    let at = CHAT_AT.load(Ordering::Acquire);
    at != 0 && now.saturating_sub(at) <= 150 && game::inventory::chat_open(now)
}
unsafe fn local_player(instance: usize) -> Option<usize> {
    if !game::readable(instance, 8) {
        return None;
    }
    let table = game::read::<usize>(instance);
    if !game::readable(table.saturating_add(0xf0), 8) {
        return None;
    }
    let getter = game::read::<usize>(table + 0xf0);
    if getter == 0 {
        return None;
    }
    let get_player: unsafe extern "system" fn(usize) -> usize = std::mem::transmute(getter);
    let player = get_player(instance);
    game::readable(player, 0x1c).then_some(player)
}
pub(crate) unsafe fn sample_chat(controller: usize) {
    if !ON.load(Ordering::Acquire) || !game::readable(controller, 0xd58) {
        return;
    }
    let model = game::read::<usize>(controller + 0xd48);
    if !game::readable(model, 0x60) {
        return;
    }
    let flag = game::read::<usize>(model + 0x48);
    if !game::readable(flag, 1) || game::read::<u8>(flag) != 1 {
        return;
    }
    let instance = game::read::<usize>(model + 0x58);
    let now = windows_sys::Win32::System::SystemInformation::GetTickCount64();
    if !game::inventory::chat_open_for(instance, now) {
        return;
    }
    let Some(player) = local_player(instance) else {
        return;
    };
    CHAT_AT.store(now, Ordering::Release);
    CHAT_TICKS.fetch_add(1, Ordering::Relaxed);
    sample_in(model, instance, player, Source::Chat);
}
fn input_allowed(source: Source, instance: usize, now: u64) -> bool {
    match source {
        Source::World => game::actions::world_input_active(now) && !game::inventory::screen_open(),
        Source::Chat => game::inventory::chat_open_for(instance, now),
    }
}
fn native_active(source: Source, instance: usize, now: u64) -> bool {
    ON.load(Ordering::Acquire)
        && input_allowed(source, instance, now)
        && !gui::captures_input()
        && !gui::DORMANT.load(Ordering::Acquire)
        && input::is_foreground()
        && !crate::api::blocked()
}
pub(crate) unsafe fn sample(model: usize, instance: usize, player: usize) {
    sample_in(model, instance, player, Source::World);
}
unsafe fn sample_in(model: usize, instance: usize, player: usize, source: Source) {
    if !ON.load(Ordering::Acquire) {
        return;
    }
    if !game::readable(player, 0x1c) {
        return;
    }
    let world = World(game::read(player + 0x10), game::read(player + 0x18));
    let observed_at = windows_sys::Win32::System::SystemInformation::GetTickCount64();
    if let Ok(mut g) = WORLD.try_lock() {
        *g = Some((world, observed_at));
    }
    // A hidden HUD callback must not consume the chat callback's request.
    // Inactive queues are withdrawn by Batch::pause, even without callbacks.
    if !native_active(source, instance, observed_at) {
        return;
    }
    let request = QUEUE.try_lock().ok().and_then(|mut q| q.take());
    let Some(r) = request else {
        return;
    };
    // A request can be queued while the rest of this HUD frame is sampling.
    // Read the clock AFTER taking it; the frame-start time may predate r.at.
    let now = windows_sys::Win32::System::SystemInformation::GetTickCount64();
    DEQUEUED.fetch_add(1, Ordering::Relaxed);
    LAST_AGE.store(now.saturating_sub(r.at), Ordering::Relaxed);
    let active = native_active(source, instance, now);
    if let Some(outcome) = decision(&r, world, now, active) {
        r.ticket.finish(outcome);
        return;
    }
    // The current HUD/chat callback owns this model during the call.
    // Independently verify its instance, lifetime flag and game references.
    if !game::readable(model, 0x60) || game::read::<usize>(model + 0x58) != instance {
        r.ticket
            .finish(Outcome::Failed("画面モデルが一致しません".into()));
        return;
    }
    let flag = game::read::<usize>(model + 0x48);
    let owner = game::read::<usize>(model + 0x20);
    if !game::readable(flag, 1) || game::read::<u8>(flag) != 1 || !game::readable(owner, 8) {
        r.ticket
            .finish(Outcome::Failed("画面モデルは無効です".into()));
        return;
    }
    if SENT.load(Ordering::Acquire) == 0 {
        r.ticket
            .finish(Outcome::Failed("送信処理が解除されています".into()));
        return;
    }
    // A request may have crossed a scene transition since the HUD sample began.
    // Ask the instance again rather than letting the native sender dereference
    // a missing/different local player internally.
    let Some(current) = local_player(instance) else {
        r.ticket
            .finish(Outcome::Failed("プレイヤーを取得できません".into()));
        return;
    };
    if current != player
        || !game::readable(current, 0x1c)
        || World(game::read(current + 0x10), game::read(current + 0x18)) != world
    {
        r.ticket.finish(Outcome::Failed(
            "プレイヤーが変わったため停止しました".into(),
        ));
        return;
    }
    // Validation above may itself span a scheduling delay or a stop request.
    let now = windows_sys::Win32::System::SystemInformation::GetTickCount64();
    if let Some(outcome) = decision(&r, world, now, native_active(source, instance, now)) {
        r.ticket.finish(outcome);
        return;
    }
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);
    if source == Source::Chat {
        CHAT_SENDS.fetch_add(1, Ordering::Relaxed);
    }
    let result = with_string(&r.text, |message| sent_hook(model, message));
    r.ticket.finish(send_outcome(result));
}
pub(crate) fn worker_flags(now: u64, flags: u64) {
    WORKER_FLAGS.store(flags, Ordering::Relaxed);
    WORKER_AT.store(now, Ordering::Relaxed);
}
#[cfg(any(feature = "entrypoint", test))]
pub(crate) fn diagnostic() -> String {
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let observation = *WORLD.lock().unwrap_or_else(|e| e.into_inner());
    let result = LAST_RESULT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let queued = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).is_some();
    format!("ChatRepeat native on={} observation_age={:?}ms pending={} submitted={} dequeued={} attempts={} accepted={} last_request_age={}ms; last_foreground_flags=0x{:X} age={}ms (bits: 0=active incl chat, 1=focused, 2=world, 3=input incl chat, 4=allowed screen, 5=overlay closed); chat_ticks={} chat_attempts={} chat_age={}ms; last_result={:?}",ON.load(Ordering::Acquire),observation.map(|(_,at)|now.saturating_sub(at)),queued,SUBMITTED.load(Ordering::Relaxed),DEQUEUED.load(Ordering::Relaxed),ATTEMPTS.load(Ordering::Relaxed),ACCEPTED.load(Ordering::Relaxed),LAST_AGE.load(Ordering::Relaxed),WORKER_FLAGS.load(Ordering::Relaxed),now.saturating_sub(WORKER_AT.load(Ordering::Relaxed)),CHAT_TICKS.load(Ordering::Relaxed),CHAT_SENDS.load(Ordering::Relaxed),now.saturating_sub(CHAT_AT.load(Ordering::Relaxed)),result.map(|(at,value)|(now.saturating_sub(at),value)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_pause_removal_is_scoped_to_the_batch() {
        let r = request();
        let lease = Arc::clone(&r.lease);
        let mut q = Some(r);
        let other = request();
        assert!(take_for_batch(&mut q, &other.lease).is_none());
        assert!(q.is_some());
        let withdrawn = take_for_batch(&mut q, &lease).unwrap();
        assert!(q.is_none());
        withdrawn.ticket.finish(Outcome::Paused);
        assert_eq!(withdrawn.ticket.status(), Outcome::Paused);
        assert!(take_for_batch(&mut q, &lease).is_none());
    }
    #[test]
    fn a_request_dequeued_before_chat_is_invalid_after_resume_even_if_very_recent() {
        let mut r = request();
        let batch = Batch {
            world: r.world,
            lease: Arc::clone(&r.lease),
        };
        batch.pause();
        assert_eq!(decision(&r, r.world, 5000, false), Some(Outcome::Paused));
        batch.resume(5000);
        assert_eq!(decision(&r, r.world, 5001, true), Some(Outcome::Paused));
        // A short pause must also invalidate the old request, not just its TTL.
        r.at = 5000;
        assert_eq!(decision(&r, r.world, 5001, true), Some(Outcome::Paused));
        r.generation = r.lease.generation.load(Ordering::Acquire);
        assert_eq!(decision(&r, r.world, 5001, true), None);
        let generation = r.generation;
        batch.pause();
        batch.pause();
        assert_eq!(r.lease.generation.load(Ordering::Acquire), generation + 1);
        assert_eq!(decision(&r, r.world, 5001, true), Some(Outcome::Paused));
    }
    #[test]
    fn native_zero_means_sent_and_nonzero_means_denied_using_only_al() {
        assert_eq!(send_outcome(0), Outcome::Sent);
        assert_eq!(send_outcome(0x1234_5600), Outcome::Sent);
        assert!(matches!(send_outcome(1), Outcome::Failed(_)));
        assert!(matches!(send_outcome(0x1234_5601), Outcome::Failed(_)));
    }
    #[test]
    fn a_request_created_during_sampling_uses_the_dequeue_time() {
        let mut r = request();
        r.at = 1100;
        r.lease.until.store(1200, Ordering::Release);
        assert!(matches!(
            decision(&r, r.world, 1000, true),
            Some(Outcome::Failed(_))
        ));
        assert_eq!(decision(&r, r.world, 1120, true), None);
    }
    fn request() -> Request {
        Request {
            text: "hello".into(),
            world: World(10, 20),
            at: 100,
            generation: 0,
            lease: Arc::new(Lease {
                live: AtomicBool::new(true),
                until: AtomicU64::new(200),
                generation: AtomicU64::new(0),
            }),
            ticket: Ticket(Arc::new(Mutex::new(Outcome::Pending))),
        }
    }
    #[test]
    fn stop_pause_world_change_and_expiry_never_invoke_native_send() {
        let r = request();
        assert_eq!(decision(&r, r.world, 150, true), None);
        assert_eq!(decision(&r, r.world, 150, false), Some(Outcome::Paused));
        assert!(matches!(
            decision(&r, World(11, 20), 150, true),
            Some(Outcome::Failed(_))
        ));
        r.lease.until.store(1000, Ordering::Release);
        assert!(matches!(
            decision(&r, r.world, 700, true),
            Some(Outcome::Failed(_))
        ));
        r.lease.live.store(false, Ordering::Release);
        assert_eq!(decision(&r, r.world, 150, true), Some(Outcome::Cancelled));
    }
    #[test]
    fn text_validation_and_native_borrow_preserve_unicode_and_sso_boundaries() {
        for text in ["", " ", "line\nbreak", "bad\0text", "/kill", "  /say test"] {
            assert!(validate(text).is_err());
        }
        assert!(validate(&"界".repeat(257)).is_err());
        for text in [
            "hi",
            "123456789012345",
            "1234567890123456",
            "日本語のチャット本文",
            &"界".repeat(256),
        ] {
            assert!(validate(text).is_ok());
            with_string(text, |pointer| {
                assert_eq!(
                    unsafe { game::string(pointer, 2048) }.as_deref(),
                    Some(text)
                );
            });
        }
    }
}
