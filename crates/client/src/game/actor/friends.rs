//! Local name rules shared by combat selection and native automatic attacks.
use crate::game::{
    actors::type_hash,
    entities::{component_pools, PoolView},
    memory::ReadBatch,
    metadata, read,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, LazyLock, Mutex,
};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static NAMES: LazyLock<Mutex<Arc<Vec<String>>>> =
    LazyLock::new(|| Mutex::new(Arc::new(Vec::new())));
pub fn available() -> Result<(), String> {
    crate::game::weapon::attack_available()?;
    metadata::available()
}
pub fn list() -> Arc<Vec<String>> {
    Arc::clone(&NAMES.lock().unwrap_or_else(|e| e.into_inner()))
}
pub fn set(names: Vec<String>) {
    let count = names.len();
    *NAMES.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(names);
    COUNT.store(count, Ordering::Release);
}
pub fn enable(on: bool) {
    ACTIVE.store(on, Ordering::Release);
}
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire) && COUNT.load(Ordering::Acquire) > 0
}
pub fn snapshot() -> Option<Arc<Vec<String>>> {
    if active() {
        Some(list())
    } else {
        None
    }
}
pub fn excludes(names: &[String], name: Option<&str>) -> bool {
    !names.is_empty() && name.is_none_or(|name| names.iter().any(|f| f.eq_ignore_ascii_case(name)))
}
/// Native pointers remain on the attack thread. Unknown player names are
/// protected while rules are active; non-player targets remain unaffected.
pub(in crate::game) unsafe fn excludes_actor(player: usize, target: usize) -> bool {
    let Some(names) = snapshot() else {
        return false;
    };
    let mut m = ReadBatch::new();
    if !m.readable(player, 0x1c) || !m.readable(target, 0x1c) {
        return true;
    }
    let registry = read::<usize>(player + 0x10);
    let entity = read::<u32>(target + 0x18);
    if read::<usize>(target + 0x10) != registry {
        return true;
    }
    let [types, meta] = component_pools(
        &mut m,
        registry,
        [type_hash("ActorTypeComponent"), metadata::HASH],
    );
    let Some(types) = types.and_then(|p| PoolView::new(&mut m, p, 4)) else {
        return true;
    };
    let Some(at) = types.component(&mut m, entity) else {
        return true;
    };
    if read::<u32>(at) & 0xff != 63 {
        return false;
    }
    let name = meta
        .and_then(|p| metadata::View::new(&mut m, p))
        .and_then(|view| view.name(&mut m, entity));
    excludes(&names, name.as_deref())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn friends_match_exact_names_and_unknown_names_are_protected() {
        let names = vec!["Steve".into(), "日本 太郎".into()];
        assert!(excludes(&names, Some("sTEVE")));
        assert!(excludes(&names, Some("日本 太郎")));
        assert!(!excludes(&names, Some("Steve2")));
        assert!(!excludes(&names, Some("Alex")));
        assert!(excludes(&names, None));
        assert!(!excludes(&[], None));
    }
}
