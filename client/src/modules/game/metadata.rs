//! Typed copies of actor metadata. Native virtual getters are checked, never called.
use super::{actors::type_hash, entities::PoolView, known_build, memory::ReadBatch, patch, read};
use crate::signatures::METADATA_ACCESSORS_SIGNATURE;
use std::sync::{Arc, OnceLock};

pub(super) const HASH: u32 = type_hash("SynchedActorDataComponent");
pub(super) const SIZE: usize = 0x48;
const MAX_ITEMS: usize = 512;
#[derive(Clone, Copy)]
struct Accessors {
    id: usize,
    kind: usize,
}
static ACCESSORS: OnceLock<Result<Accessors, String>> = OnceLock::new();
fn accessors() -> Result<Accessors, String> {
    ACCESSORS
        .get_or_init(|| {
            if !known_build() {
                return Err("actor metadata requires the analyzed game build".into());
            }
            // Another class has the same two getters at offsets 0x20/0x22.
            // Resolve all anchors, then require one with the verified field offsets.
            let candidates = patch::find_all(METADATA_ACCESSORS_SIGNATURE, 16)?;
            let matching: Vec<_> = candidates
                .into_iter()
                .filter(|&at| unsafe { read::<u8>(at + 3) == 0x0a && read::<u8>(at + 19) == 8 })
                .collect();
            if matching.len() != 1 {
                return Err(format!(
                    "DataItem accessors: {} matching layouts",
                    matching.len()
                ));
            }
            let at = matching[0];
            Ok(Accessors {
                id: at,
                kind: at + 16,
            })
        })
        .clone()
}
pub(super) fn available() -> Result<(), String> {
    accessors().map(|_| ())
}

unsafe fn item(
    m: &mut ReadBatch,
    component: usize,
    id: u16,
    kind: u8,
    methods: Accessors,
) -> Option<usize> {
    let (begin, end) = m.span(component, 8)?;
    let count = (end - begin) / 8;
    if count > MAX_ITEMS || id as usize >= count {
        return None;
    }
    let at: usize = read(begin + id as usize * 8);
    if !m.readable(at, 16) {
        return None;
    }
    let vt: usize = read(at);
    if !m.readable(vt, 24)
        || read::<usize>(vt + 8) != methods.id
        || read::<usize>(vt + 16) != methods.kind
        || read::<u16>(at + 10) != id
        || read::<u8>(at + 8) != kind
    {
        return None;
    }
    Some(at)
}

pub(super) struct View {
    pool: PoolView,
    methods: Accessors,
}
impl View {
    pub unsafe fn name(&self, m: &mut ReadBatch, entity: u32) -> Option<Arc<str>> {
        self.name_reusing(m, entity, None)
    }
    pub unsafe fn owner(&self, m: &mut ReadBatch, entity: u32) -> Option<u64> {
        let component = self.pool.component(m, entity)?;
        let at = item(m, component, 5, 7, self.methods)?;
        m.readable(at, 24).then(|| read::<u64>(at + 16))
    }
    pub unsafe fn new(m: &mut ReadBatch, pool: usize) -> Option<Self> {
        Some(Self {
            pool: PoolView::new(m, pool, SIZE)?,
            methods: accessors().ok()?,
        })
    }
    pub unsafe fn name_reusing(
        &self,
        m: &mut ReadBatch,
        entity: u32,
        old: Option<&Arc<str>>,
    ) -> Option<Arc<str>> {
        let component = self.pool.component(m, entity)?;
        let at = item(m, component, 4, 4, self.methods)?;
        m.with_string(at + 16, 256, |s| shared_name(s, old))
            .flatten()
    }
    pub unsafe fn air(&self, m: &mut ReadBatch, entity: u32) -> Option<(i16, i16)> {
        let component = self.pool.component(m, entity)?;
        let current = read::<i16>(item(m, component, 7, 1, self.methods)? + 12);
        let max = read::<i16>(item(m, component, 42, 1, self.methods)? + 12);
        (max > 0 && current >= -20 && current <= max).then_some((current, max))
    }
}

/// Strip game formatting and line breaks; bound label width in Unicode characters.
fn cleaned<'a>(text: &str, buffer: &'a mut [u8; 256]) -> Option<&'a str> {
    let mut chars = text.chars();
    let mut used = 0;
    let mut count = 0;
    while let Some(c) = chars.next() {
        if c == '§' {
            chars.next();
            continue;
        }
        if c.is_control() {
            continue;
        }
        if count == 64 {
            break;
        }
        used += c.encode_utf8(&mut buffer[used..]).len();
        count += 1;
    }
    // The buffer contains only char::encode_utf8 output.
    let trimmed = std::str::from_utf8(&buffer[..used]).ok()?.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}
fn shared_name(text: &str, old: Option<&Arc<str>>) -> Option<Arc<str>> {
    let mut buffer = [0; 256];
    let name = cleaned(text, &mut buffer)?;
    Some(match old {
        Some(old) if old.as_ref() == name => Arc::clone(old),
        _ => Arc::from(name),
    })
}
#[cfg(test)]
fn clean_name(text: &str) -> Option<String> {
    cleaned(text, &mut [0; 256]).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unchanged_display_names_reuse_owned_text_but_renames_and_empty_names_do_not() {
        let first = shared_name("§a[MVP+] 日本 Player§r", None).unwrap();
        let same = shared_name("§b[MVP+] 日本 Player§r", Some(&first)).unwrap();
        assert!(Arc::ptr_eq(&first, &same));
        let renamed = shared_name("§bNew Player§r", Some(&first)).unwrap();
        assert_eq!(renamed.as_ref(), "New Player");
        assert!(!Arc::ptr_eq(&first, &renamed));
        assert!(shared_name("§r \n", Some(&first)).is_none());
        let long = "😀".repeat(100);
        assert_eq!(shared_name(&long, None).unwrap().chars().count(), 64);
    }
    #[test]
    fn metadata_checks_id_type_and_virtual_getters_without_calling_them() {
        let methods = Accessors {
            id: 0x123,
            kind: 0x456,
        };
        let vt = [0usize, methods.id, methods.kind];
        let mut row = [0u64; 4];
        row[0] = vt.as_ptr() as u64;
        let mut slots = [0usize; 43];
        slots[7] = row.as_ptr() as usize;
        let header = [
            slots.as_ptr() as usize,
            slots.as_ptr() as usize + slots.len() * 8,
        ];
        let component = header.as_ptr() as usize;
        let mut m = ReadBatch::new();
        unsafe {
            (row.as_mut_ptr().cast::<u8>().add(8)).write(1);
            (row.as_mut_ptr().cast::<u8>().add(10).cast::<u16>()).write_unaligned(7);
            assert_eq!(
                item(&mut m, component, 7, 1, methods),
                Some(row.as_ptr() as usize)
            );
            assert_eq!(item(&mut m, component, 7, 4, methods), None);
            assert_eq!(item(&mut m, component, 42, 1, methods), None);
            assert_eq!(item(&mut m, component, 512, 1, methods), None);
            assert_eq!(
                item(
                    &mut m,
                    component,
                    7,
                    1,
                    Accessors {
                        id: 0x999,
                        ..methods
                    }
                ),
                None
            );
            (row.as_mut_ptr().cast::<u8>().add(10).cast::<u16>()).write_unaligned(8);
            assert_eq!(item(&mut m, component, 7, 1, methods), None);
        }
    }
    #[test]
    fn display_names_preserve_unicode_and_remove_formatting_controls() {
        assert_eq!(clean_name("§aSteve§r\n\0"), Some("Steve".into()));
        assert_eq!(clean_name("  牛の名前  "), Some("牛の名前".into()));
        assert_eq!(clean_name("§r \t"), None);
        assert_eq!(clean_name(&"牛".repeat(100)).unwrap().chars().count(), 64);
    }
}
