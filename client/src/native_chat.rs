//! Capture owned copies at native displayed-chat and submitted-chat boundaries.
pub(crate) mod repeat;
use crate::signatures::{CHAT_RECEIVE_SIGNATURE, CHAT_SENT_SIGNATURE, CHAT_TICK_SIGNATURE};
use crate::{
    api,
    modules::{game, patch},
};
use hudhook::mh::{MH_CreateHook, MH_EnableHook};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

type ReceiveFn = unsafe extern "system" fn(usize, usize, u32) -> usize;
type SentFn = unsafe extern "system" fn(usize, usize) -> usize;
static RECEIVE: AtomicUsize = AtomicUsize::new(0);
static SENT: AtomicUsize = AtomicUsize::new(0);
type ChatTickFn = unsafe extern "system" fn(usize) -> u32;
static CHAT_TICK: AtomicUsize = AtomicUsize::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static TARGETS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static START: OnceLock<std::result::Result<(), String>> = OnceLock::new();

unsafe fn string(address: usize) -> Option<String> {
    game::string(address, 32768)
}
unsafe extern "system" fn receive_hook(context: usize, message: usize, kind: u32) -> usize {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    // GuiData receives a ChatMessage record by value and destroys it in the
    // original function. Copy the body and author before handing it over.
    let text = string(message.saturating_add(8));
    let sender = string(message.saturating_add(0x70)).filter(|s| !s.is_empty());
    let original: ReceiveFn = std::mem::transmute(RECEIVE.load(Ordering::Acquire));
    let result = original(context, message, kind);
    if let Some(text) = text.filter(|s| !s.is_empty()) {
        api::chat(api::ChatDirection::Received, sender, text);
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}
unsafe extern "system" fn sent_hook(context: usize, message: usize) -> usize {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let text = string(message);
    let original: SentFn = std::mem::transmute(SENT.load(Ordering::Acquire));
    let result = original(context, message);
    if let Some(text) = text.filter(|s| !s.is_empty()) {
        api::chat(api::ChatDirection::Sent, None, text);
    }
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}
unsafe extern "system" fn chat_tick_hook(controller: usize) -> u32 {
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    // The receiver owns its ScreenModel for this callback. No controller or
    // draft pointer is retained, and the normal tick's result is preserved.
    repeat::sample_chat(controller);
    let original: ChatTickFn = std::mem::transmute(CHAT_TICK.load(Ordering::Acquire));
    let result = original(controller);
    IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    result
}
pub fn start() -> std::result::Result<(), String> {
    START
        .get_or_init(|| {
            if !game::known_build() {
                return Err(format!(
                    "native chat requires {}",
                    crate::modules::game::SUPPORTED
                ));
            }
            let receive = patch::find(CHAT_RECEIVE_SIGNATURE)?;
            let sent = patch::find(CHAT_SENT_SIGNATURE)?;
            let chat_tick = patch::find(CHAT_TICK_SIGNATURE)?;
            let mut targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
            unsafe {
                for (target, detour, original) in [
                    (receive, receive_hook as *mut c_void, &RECEIVE),
                    (sent, sent_hook as *mut c_void, &SENT),
                    (chat_tick, chat_tick_hook as *mut c_void, &CHAT_TICK),
                ] {
                    let mut trampoline = std::ptr::null_mut();
                    MH_CreateHook(target as *mut c_void, detour, &mut trampoline)
                        .ok()
                        .map_err(|e| format!("chat MH_CreateHook: {e:?}"))?;
                    original.store(trampoline as usize, Ordering::Release);
                    targets.push(target);
                }
                for &target in targets.iter() {
                    MH_EnableHook(target as *mut c_void)
                        .ok()
                        .map_err(|e| format!("chat MH_EnableHook: {e:?}"))?;
                }
            }
            Ok(())
        })
        .clone()
}
pub fn shutdown() -> bool {
    repeat::set_active(false);
    let targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    targets.iter().all(|&target| crate::hooks::disable(target))
        && crate::hooks::drain(&IN_FLIGHT, 2)
}

#[cfg(test)]
mod tests {
    use super::string;
    #[repr(C)]
    struct NativeString {
        bytes: [u8; 16],
        length: usize,
        capacity: usize,
    }

    #[test]
    fn copies_native_sso_and_heap_utf8() {
        let mut short = NativeString {
            bytes: [0; 16],
            length: 6,
            capacity: 15,
        };
        short.bytes[..6].copy_from_slice("短文".as_bytes());
        assert_eq!(
            unsafe { string(&short as *const _ as usize) }.as_deref(),
            Some("短文")
        );
        let data = "こんにちは from native chat".as_bytes().to_vec();
        let mut heap = NativeString {
            bytes: [0; 16],
            length: data.len(),
            capacity: data.len().max(16),
        };
        heap.bytes[..8].copy_from_slice(&(data.as_ptr() as usize).to_le_bytes());
        let owned = unsafe { string(&heap as *const _ as usize) }.unwrap();
        drop(data);
        assert_eq!(owned, "こんにちは from native chat");
    }
    #[test]
    fn rejects_invalid_or_unbounded_native_strings() {
        for (length, capacity) in [(16, 15), (1, 2), (32769, 65535)] {
            let invalid = NativeString {
                bytes: [0; 16],
                length,
                capacity,
            };
            assert!(unsafe { string(&invalid as *const _ as usize) }.is_none());
        }
        let invalid_pointer = NativeString {
            bytes: [0; 16],
            length: 20,
            capacity: 31,
        };
        assert!(unsafe { string(&invalid_pointer as *const _ as usize) }.is_none());
    }
}
