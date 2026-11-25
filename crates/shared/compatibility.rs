//! Pure compatibility checks shared by the native client and offline audit.
use crate::signatures::*;

/// An analyzed executable: how to recognize it (PE timestamp, image size and
/// checksum) and the addresses in it that cannot be found by signature.
/// Every other address is found by signature and checked at run time.
pub(crate) struct Build {
    pub name: &'static str,
    pub timestamp: u32,
    pub image_size: u32,
    pub checksum: u32,
    /// BlockSource vtable (its slot +0x10 is getBlock).
    pub source_vtable: usize,
    /// Pointer chain to the air block: `[[[image + this]] + 0x240]`.
    pub air_registry: usize,
    /// FishingHook vtable (its slot +0x220 is the event handler).
    pub hook_vtable: usize,
    /// Inventory request manager vtable (slot +0x28 begins a request).
    pub inventory_manager_vtable: usize,
    pub item_stack_vtable: usize,
    pub request_scope_vtables: [usize; 2],
}

/// The analyzed builds. 1.26.65.1 is a hotfix of 1.26.6301.0: the same code
/// moved by a few bytes, `.rdata` and `.data` by 0x1000. Its addresses were
/// derived from the same anchors (and matched by the code each vtable slot
/// points at), and the air chain was confirmed in the running game; see
/// analysis/notes/build_1.26.65.1.md.
pub(crate) const BUILDS: [Build; 2] = [
    Build {
        name: "1.26.6301.0",
        timestamp: 0x6AED_1906,
        image_size: 0x1383_D000,
        checksum: 0x134D_8B7E,
        source_vtable: 0xF1899F0,
        air_registry: 0x129685B8,
        hook_vtable: 0xF16F850,
        inventory_manager_vtable: 0xF160B20,
        item_stack_vtable: 0xEFFAE80,
        request_scope_vtables: [0xF160FD0, 0xF161000],
    },
    Build {
        name: "1.26.65.1",
        timestamp: 0x6AFA_31EC,
        image_size: 0x1383_E000,
        checksum: 0x134D_E2E9,
        source_vtable: 0xF18A9F0,
        air_registry: 0x129695B8,
        hook_vtable: 0xF170850,
        inventory_manager_vtable: 0xF161B20,
        item_stack_vtable: 0xEFFBE80,
        request_scope_vtables: [0xF161FD0, 0xF162000],
    },
];

/// Build-specific addresses (RVAs) that no signature finds directly. None
/// where a build that was not analyzed did not yield one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Addresses {
    /// BlockSource vtable (its slot +0x10 is getBlock).
    pub source_vtable: Option<usize>,
    /// Pointer chain to the air block: `[[[image + this]] + 0x240]`.
    pub air_registry: Option<usize>,
    /// FishingHook vtable (its slot +0x220 is the event handler).
    pub hook_vtable: Option<usize>,
    /// Inventory request manager vtable (slot +0x28 begins a request).
    pub inventory_manager_vtable: Option<usize>,
    pub item_stack_vtable: Option<usize>,
    pub request_scope_vtables: Option<[usize; 2]>,
}

impl Addresses {
    pub(crate) fn complete(&self) -> bool {
        self.source_vtable.is_some()
            && self.air_registry.is_some()
            && self.hook_vtable.is_some()
            && self.inventory_manager_vtable.is_some()
            && self.item_stack_vtable.is_some()
            && self.request_scope_vtables.is_some()
    }
}

impl From<&Build> for Addresses {
    fn from(b: &Build) -> Self {
        Self {
            source_vtable: Some(b.source_vtable),
            air_registry: Some(b.air_registry),
            hook_vtable: Some(b.hook_vtable),
            inventory_manager_vtable: Some(b.inventory_manager_vtable),
            item_stack_vtable: Some(b.item_stack_vtable),
            request_scope_vtables: Some(b.request_scope_vtables),
        }
    }
}

/// An executable as the derivation sees it, in RVAs: the running game, or
/// (in tests) an executable file.
pub(crate) trait Code {
    /// Every match of `signature` in the executable code (at least two when
    /// there are several).
    fn find(&self, signature: &str) -> Vec<usize>;
    /// `len` bytes at `rva`, if mapped (in a file: present in the file).
    fn bytes(&self, rva: usize, len: usize) -> Option<&[u8]>;
    /// (RVA, size) of the section `name`.
    fn section(&self, name: &str) -> Option<(usize, usize)>;
    /// The image base that absolute pointers in the image are relative to.
    fn base(&self) -> usize;
}

fn unique<C: Code + ?Sized>(code: &C, signature: &str) -> Option<usize> {
    match code.find(signature)[..] {
        [at] => Some(at),
        _ => None,
    }
}

fn in_section<C: Code + ?Sized>(code: &C, name: &str, rva: usize) -> bool {
    code.section(name)
        .is_some_and(|(start, size)| (start..start + size).contains(&rva))
}

/// The RVA a pointer at `rva` points to.
fn pointer<C: Code + ?Sized>(code: &C, rva: usize) -> Option<usize> {
    let bytes = code.bytes(rva, 8)?.try_into().ok()?;
    (u64::from_le_bytes(bytes) as usize).checked_sub(code.base())
}

/// What the RIP-relative operand (a 7-byte `mov`/`lea` with its displacement
/// at +3) `operand` bytes into the unique match of `signature` refers to.
fn operand<C: Code + ?Sized>(code: &C, signature: &str, operand: usize) -> Option<usize> {
    let at = unique(code, signature)? + operand;
    let displacement = i32::from_le_bytes(code.bytes(at + 3, 4)?.try_into().ok()?);
    (at + 7).checked_add_signed(displacement as isize)
}

/// Tables in `.rdata` that hold a pointer to `function` at `slot`.
fn tables_holding<C: Code + ?Sized>(code: &C, function: usize, slot: usize) -> Vec<usize> {
    let Some((start, size)) = code.section(".rdata") else {
        return Vec::new();
    };
    let Some(bytes) = code.bytes(start, size) else {
        return Vec::new();
    };
    let wanted = (code.base() + function) as u64;
    // Sections start on page boundaries: chunks are 8-byte aligned.
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, q)| u64::from_le_bytes(**q) == wanted)
        .filter_map(|(i, _)| (start + 8 * i).checked_sub(slot))
        .collect()
}

/// Derives the build-specific addresses from code, each checked against
/// what it must hold: a vtable's known slot points at the function its
/// signature finds, data lies in the section it must.
pub(crate) fn derive<C: Code + ?Sized>(code: &C) -> Addresses {
    let rdata = |rva: &usize| in_section(code, ".rdata", *rva);
    let slot_holds = |table: usize, slot: usize, function: Option<usize>| {
        function.is_some() && pointer(code, table + slot) == function
    };
    let getter = unique(code, GET_BLOCK_SIGNATURE);
    let begin = unique(code, INVENTORY_REQUEST_BEGIN_SIGNATURE);
    let scopes = (
        operand(code, REQUEST_SCOPE_VTABLE_SIGNATURE, 0).filter(rdata),
        operand(code, REQUEST_SCOPE_VTABLE_2_SIGNATURE, 0).filter(rdata),
    );
    Addresses {
        source_vtable: operand(code, BLOCK_SOURCE_VTABLE_SIGNATURE, 0)
            .filter(|&t| rdata(&t) && slot_holds(t, 0x10, getter)),
        air_registry: operand(code, AIR_REGISTRY_SIGNATURE, 0)
            .filter(|&r| in_section(code, ".data", r)),
        hook_vtable: unique(code, FISH_EVENT_SIGNATURE).and_then(|event| {
            match tables_holding(code, event, 0x220)[..] {
                [table] => Some(table),
                _ => None,
            }
        }),
        inventory_manager_vtable: operand(code, REQUEST_MANAGER_VTABLE_SIGNATURE, 8)
            .filter(|&t| rdata(&t) && slot_holds(t, 0x28, begin)),
        item_stack_vtable: operand(code, ITEM_STACK_VTABLE_SIGNATURE, 0).filter(rdata),
        request_scope_vtables: match scopes {
            (Some(a), Some(b)) if a != b => Some([a, b]),
            _ => None,
        },
    }
}

/// Finds the option id in the registration code following the name anchor:
/// `mov edx, <id>` followed closely by `mov r9d, <imm> / call`.
pub(crate) fn option_id_after(code: &[u8]) -> Option<u32> {
    (0..code.len().saturating_sub(5)).find_map(|i| {
        // mov edx, imm32 with a small id
        if code[i] != 0xBA || code[i + 3] != 0 || code[i + 4] != 0 {
            return None;
        }
        // ...then, within the next few instructions, mov r9d, imm32 / call rel32
        let tail = &code[i + 5..(i + 5 + 16).min(code.len())];
        let call = tail
            .windows(7)
            .any(|w| w[0] == 0x41 && w[1] == 0xB9 && w[4] == 0 && w[5] == 0 && w[6] == 0xE8);
        call.then(|| u32::from_le_bytes([code[i + 1], code[i + 2], 0, 0]))
    })
}
