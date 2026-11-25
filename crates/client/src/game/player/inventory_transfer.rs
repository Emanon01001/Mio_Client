//! Balanced native inventory swaps on the input thread. ItemStack ownership,
//! container notifications and legacy transaction grouping remain in the game.
use crate::game::{layout::image, memory::ReadBatch, patch, read};
use crate::signatures::{
    INVENTORY_ITEM_SIGNATURE, INVENTORY_REQUEST_BEGIN_SIGNATURE, INVENTORY_SET_SIGNATURE,
    INVENTORY_STACK_COPY_SIGNATURE,
};
use std::sync::OnceLock;
struct Native {
    begin: usize,
    copy: usize,
    set: usize,
    get: usize,
    manager_vt: usize,
    stack_vt: usize,
    scope_vts: [usize; 2],
}
static NATIVE: OnceLock<Result<Native, String>> = OnceLock::new();
pub(in crate::game) fn available() -> Result<(), String> {
    NATIVE
        .get_or_init(|| {
            let a = crate::game::layout::addresses();
            let (Some(manager_vt), Some(stack_vt), Some(scope_vts)) = (
                a.inventory_manager_vtable,
                a.item_stack_vtable,
                a.request_scope_vtables,
            ) else {
                return Err(format!(
                    "inventory swap requires {}",
                    crate::game::layout::SUPPORTED
                ));
            };
            if !crate::game::actions_build() {
                return Err(format!(
                    "inventory swap requires {}",
                    crate::game::layout::SUPPORTED
                ));
            }
            let base = image().0;
            let n = Native {
                begin: patch::find(INVENTORY_REQUEST_BEGIN_SIGNATURE)?,
                copy: patch::find(INVENTORY_STACK_COPY_SIGNATURE)?,
                set: patch::find(INVENTORY_SET_SIGNATURE)?,
                get: patch::find(INVENTORY_ITEM_SIGNATURE)?,
                manager_vt: base + manager_vt,
                stack_vt: base + stack_vt,
                scope_vts: scope_vts.map(|vt| base + vt),
            };
            if unsafe { read::<usize>(n.manager_vt + 0x28) } != n.begin {
                return Err("inventory request vtable mismatch".into());
            }
            Ok(n)
        })
        .as_ref()
        .map(|_| ())
        .map_err(Clone::clone)
}
#[repr(C, align(16))]
struct StackBuffer([u8; 0xa0]);
struct Stack {
    buffer: Box<StackBuffer>,
    dtor: usize,
}
impl Stack {
    unsafe fn copy(n: &Native, source: usize) -> Self {
        // C++ holds internal self-relative SBO pointers: storage must never move.
        let mut s = Self {
            buffer: Box::new(StackBuffer([0; 0xa0])),
            dtor: read::<usize>(n.stack_vt),
        };
        let copy: unsafe extern "system" fn(usize, usize) -> usize = std::mem::transmute(n.copy);
        copy(s.ptr(), source);
        s
    }
    fn ptr(&mut self) -> usize {
        self.buffer.0.as_mut_ptr() as usize
    }
}
impl Drop for Stack {
    fn drop(&mut self) {
        unsafe {
            let f: unsafe extern "system" fn(usize, u32) = std::mem::transmute(self.dtor);
            f(self.ptr(), 0);
        }
    }
}
#[repr(C, align(16))]
struct ScopeBuffer([u8; 0x50]);
struct Scope {
    buffer: Box<ScopeBuffer>,
    invoke: usize,
    destroy: usize,
}
impl Scope {
    unsafe fn begin(n: &Native, manager: usize) -> Result<Self, &'static str> {
        let mut buffer = Box::new(ScopeBuffer([0; 0x50]));
        let f: unsafe extern "system" fn(usize, usize) -> usize = std::mem::transmute(n.begin);
        f(manager, buffer.0.as_mut_ptr() as usize);
        let vt = read::<usize>(buffer.0.as_ptr() as usize);
        // Both concrete std::function closures were verified against this build.
        if !n.scope_vts.contains(&vt) {
            return Err("unexpected inventory request scope");
        }
        let scope = Self {
            buffer,
            invoke: read(vt + 0x10),
            destroy: read(vt + 0x20),
        };
        if vt == n.scope_vts[0] {
            return Err("inventory request context unavailable");
        }
        Ok(scope)
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        unsafe {
            let at = self.buffer.0.as_mut_ptr() as usize;
            let object = read::<usize>(at + 0x38);
            if read::<u8>(at + 0x40) != 0 {
                let invoke: unsafe extern "system" fn(usize) = std::mem::transmute(self.invoke);
                invoke(object);
            }
            let destroy: unsafe extern "system" fn(usize, u8) = std::mem::transmute(self.destroy);
            destroy(object, 0); // inline std::function storage; never free the Rust buffer
        }
    }
}
/// The caller has revalidated item identities and owns the game's input tick.
pub(in crate::game) unsafe fn swap(
    player: usize,
    source: usize,
    destination: usize,
) -> Result<(), &'static str> {
    let Some(Ok(n)) = NATIVE.get() else {
        return Err("inventory swap unavailable");
    };
    swap_in(n, player, source, destination)
}
unsafe fn swap_in(
    n: &Native,
    player: usize,
    source: usize,
    destination: usize,
) -> Result<(), &'static str> {
    if !(9..36).contains(&source) || destination >= 9 {
        return Err("invalid inventory swap slots");
    }
    let (container, begin, manager) = {
        let mut m = ReadBatch::new();
        if !m.readable(player, 0xae8) {
            return Err("player unavailable");
        }
        let proxy = read::<usize>(player + 0x5e0);
        if !m.readable(proxy, 0xc0) {
            return Err("inventory proxy unavailable");
        }
        let container = read::<usize>(proxy + 0xb8);
        if !m.readable(container, 0x1b8) {
            return Err("inventory container unavailable");
        }
        let vt = read::<usize>(container);
        if !m.readable(vt, 0x70)
            || read::<usize>(vt + 0x38) != n.get
            || read::<usize>(vt + 0x68) != n.set
            || read::<usize>(container + 0x1b0) != player
        {
            return Err("inventory owner / notification route mismatch");
        }
        let (begin, end) = m
            .span(container + 0x198, 0x98)
            .ok_or("inventory span unavailable")?;
        if end - begin != 36 * 0x98 {
            return Err("inventory span mismatch");
        }
        let manager = read::<usize>(player + 0xae0);
        if !m.readable(manager, 0x70) || read::<usize>(manager) != n.manager_vt {
            return Err("inventory request manager mismatch");
        }
        if read::<i32>(manager + 0x50) != 0 || read::<usize>(manager + 0x68) != 0 {
            return Err("inventory request already active");
        }
        (container, begin, manager)
    };
    // No ReadBatch spans a native call. Copy via C++ before either slot changes.
    let mut from = Stack::copy(n, begin + source * 0x98);
    let mut to = Stack::copy(n, begin + destination * 0x98);
    let _scope = Scope::begin(n, manager)?;
    let set: unsafe extern "system" fn(usize, u32, usize, u32) = std::mem::transmute(n.set);
    set(container, destination as u32, from.ptr(), 0);
    set(container, source as u32, to.ptr(), 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    static VALID: AtomicBool = AtomicBool::new(true);
    static COPIES: AtomicUsize = AtomicUsize::new(0);
    static DTORS: AtomicUsize = AtomicUsize::new(0);
    static SETS: AtomicUsize = AtomicUsize::new(0);
    static FINISHED: AtomicUsize = AtomicUsize::new(0);
    unsafe fn put<T>(at: usize, value: T) {
        (at as *mut T).write_unaligned(value);
    }
    fn check(ok: bool) {
        if !ok {
            VALID.store(false, Ordering::Relaxed);
        }
    }
    unsafe extern "system" fn copy(to: usize, from: usize) -> usize {
        std::ptr::copy_nonoverlapping(from as *const u8, to as *mut u8, 0x98);
        put(to + 0x80, to + 0x88);
        COPIES.fetch_add(1, Ordering::Relaxed);
        to
    }
    unsafe extern "system" fn dtor(at: usize, flags: u32) {
        check(flags == 0 && read::<usize>(at + 0x80) == at + 0x88);
        DTORS.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "system" fn begin(manager: usize, out: usize) -> usize {
        put(manager + 0x50, -4i32);
        put(out, read::<usize>(manager + 8));
        put(out + 8, manager);
        put(out + 0x38, out);
        put(out + 0x40, 1u8);
        out
    }
    unsafe extern "system" fn finish(at: usize) {
        let manager = read::<usize>(at + 8);
        check(read::<usize>(at + 0x38) == at && read::<i32>(manager + 0x50) == -4);
        put(manager + 0x50, 0i32);
        FINISHED.fetch_add(1, Ordering::Relaxed);
    }
    unsafe extern "system" fn destroy(_: usize, free: u8) {
        check(free == 0);
    }
    unsafe extern "system" fn set(container: usize, slot: u32, stack: usize, flag: u32) {
        let player = read::<usize>(container + 0x1b0);
        let manager = read::<usize>(player + 0xae0);
        check(
            flag == 0
                && read::<i32>(manager + 0x50) == -4
                && read::<usize>(stack + 0x80) == stack + 0x88,
        );
        let target = read::<usize>(container + 0x198) + slot as usize * 0x98;
        std::ptr::copy_nonoverlapping(stack as *const u8, target as *mut u8, 0x98);
        put(target + 0x80, target + 0x88);
        SETS.fetch_add(1, Ordering::Relaxed);
    }
    #[test]
    fn native_swap_groups_both_changes_and_destroys_stable_copies_on_all_paths() {
        let mut svt = [0usize; 6];
        svt[0] = dtor as *const () as usize;
        let mut scope_vt = [0usize; 6];
        scope_vt[2] = finish as *const () as usize;
        scope_vt[4] = destroy as *const () as usize;
        let mut no_op = [0usize; 6];
        no_op[2] = finish as *const () as usize;
        no_op[4] = destroy as *const () as usize;
        let mut mvt = [0usize; 6];
        mvt[5] = begin as *const () as usize;
        let mut cvt = [0usize; 14];
        cvt[7] = 0x1234;
        cvt[13] = set as *const () as usize;
        let n = Native {
            begin: begin as *const () as usize,
            copy: copy as *const () as usize,
            set: cvt[13],
            get: cvt[7],
            manager_vt: mvt.as_ptr() as usize,
            stack_vt: svt.as_ptr() as usize,
            scope_vts: [no_op.as_ptr() as usize, scope_vt.as_ptr() as usize],
        };
        let mut slots = vec![[0u64; 19]; 36];
        let mut manager = [0usize; 16];
        manager[0] = n.manager_vt;
        manager[1] = n.scope_vts[1];
        let mut container = [0usize; 55];
        container[0] = cvt.as_ptr() as usize;
        container[0x198 / 8] = slots.as_mut_ptr() as usize;
        container[0x1a0 / 8] = container[0x198 / 8] + 36 * 0x98;
        let mut proxy = [0usize; 24];
        proxy[0xb8 / 8] = container.as_mut_ptr() as usize;
        let mut player = [0usize; 350];
        player[0x5e0 / 8] = proxy.as_mut_ptr() as usize;
        player[0xae0 / 8] = manager.as_mut_ptr() as usize;
        let player = player.as_mut_ptr() as usize;
        container[0x1b0 / 8] = player;
        let a = slots.as_mut_ptr() as usize;
        let b = a + 10 * 0x98;
        unsafe {
            put(a, n.stack_vt);
            put(b, n.stack_vt);
            put(a + 0x70, 11u64);
            put(b + 0x70, 99u64);
            assert_eq!(swap_in(&n, player, 10, 0), Ok(()));
            assert_eq!((read::<u64>(a + 0x70), read::<u64>(b + 0x70)), (99, 11));
            assert_eq!(read::<i32>(manager.as_ptr() as usize + 0x50), 0);
            assert_eq!(
                (
                    COPIES.load(Ordering::Relaxed),
                    DTORS.load(Ordering::Relaxed),
                    SETS.load(Ordering::Relaxed),
                    FINISHED.load(Ordering::Relaxed)
                ),
                (2, 2, 2, 1)
            );
            assert!(VALID.load(Ordering::Relaxed));
            // Bad bounds, ownership and an already-active request never mutate slots.
            assert!(swap_in(&n, player, 8, 0).is_err());
            assert!(swap_in(&n, player, 10, 9).is_err());
            put(manager.as_mut_ptr() as usize + 0x50, -6i32);
            assert!(swap_in(&n, player, 10, 0).is_err());
            put(manager.as_mut_ptr() as usize + 0x50, 0i32);
            put(container.as_mut_ptr() as usize + 0x1b0, 0usize);
            assert!(swap_in(&n, player, 10, 0).is_err());
            assert_eq!(SETS.load(Ordering::Relaxed), 2);
            assert_eq!((read::<u64>(a + 0x70), read::<u64>(b + 0x70)), (99, 11));
            put(container.as_mut_ptr() as usize + 0x1b0, player);
            put(manager.as_mut_ptr() as usize + 8, n.scope_vts[0]);
            assert_eq!(
                swap_in(&n, player, 10, 0),
                Err("inventory request context unavailable")
            );
            assert_eq!(
                (
                    COPIES.load(Ordering::Relaxed),
                    DTORS.load(Ordering::Relaxed),
                    SETS.load(Ordering::Relaxed)
                ),
                (4, 4, 2)
            );
            assert_eq!(read::<i32>(manager.as_ptr() as usize + 0x50), 0);
            assert!(VALID.load(Ordering::Relaxed));
        }
    }
}
