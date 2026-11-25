//! Checked, file-backed PE32+ access. The target is never loaded or executed.
use crate::Identity;
use sigscan::Section;

pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}
pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}
pub(crate) struct Pe<'a> {
    pub data: &'a [u8],
    pub identity: Identity,
    pub sections: Vec<Section>,
    headers: usize,
}
impl<'a> Pe<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, String> {
        let invalid = || "Invalid or truncated PE32+ executable".to_string();
        let pe = u32_at(data, 0x3c).ok_or_else(invalid)? as usize;
        if data.get(..2) != Some(b"MZ") || data.get(pe..pe.saturating_add(4)) != Some(b"PE\0\0") {
            return Err(invalid());
        }
        let opt = pe.checked_add(24).ok_or_else(invalid)?;
        let count = u16_at(data, pe + 6).ok_or_else(invalid)? as usize;
        let opt_len = u16_at(data, pe + 20).ok_or_else(invalid)? as usize;
        if !(1..=96).contains(&count)
            || opt_len < 112
            || data.get(opt..opt.saturating_add(opt_len)).is_none()
        {
            return Err(invalid());
        }
        let machine = u16_at(data, pe + 4).ok_or_else(invalid)?;
        if machine != 0x8664 || u16_at(data, opt) != Some(0x20b) {
            return Err("Expected an AMD64 / PE32+ executable".into());
        }
        let identity = Identity {
            machine,
            timestamp: u32_at(data, pe + 8).ok_or_else(invalid)?,
            image_size: u32_at(data, opt + 56).ok_or_else(invalid)?,
            checksum: u32_at(data, opt + 64).ok_or_else(invalid)?,
            image_base: u64_at(data, opt + 24).ok_or_else(invalid)?,
            file_size: data.len() as u64,
        };
        let headers = u32_at(data, opt + 60).ok_or_else(invalid)? as usize;
        let table_end = opt + opt_len + count * 40;
        if headers < table_end
            || headers > data.len()
            || headers > identity.image_size as usize
            || identity
                .image_base
                .checked_add(u64::from(identity.image_size))
                .is_none()
        {
            return Err(invalid());
        }
        let sections = sigscan::sections(data).ok_or_else(invalid)?;
        let overlaps = |a: std::ops::Range<u64>, b: std::ops::Range<u64>| {
            a.start < a.end && b.start < b.end && a.start < b.end && b.start < a.end
        };
        for (i, s) in sections.iter().enumerate() {
            let mapped = u64::from(s.virtual_size.max(s.raw_size));
            let rva = u64::from(s.rva);
            let raw = u64::from(s.raw_offset);
            if rva + mapped > u64::from(identity.image_size)
                || (s.raw_size != 0
                    && (raw < headers as u64 || raw + u64::from(s.raw_size) > data.len() as u64))
            {
                return Err(format!("Section {} is outside the file/image", s.name));
            }
            for old in &sections[..i] {
                if s.name == old.name
                    || overlaps(
                        rva..rva + mapped,
                        u64::from(old.rva)
                            ..u64::from(old.rva) + u64::from(old.virtual_size.max(old.raw_size)),
                    )
                    || (s.raw_size != 0
                        && old.raw_size != 0
                        && overlaps(
                            raw..raw + u64::from(s.raw_size),
                            u64::from(old.raw_offset)
                                ..u64::from(old.raw_offset) + u64::from(old.raw_size),
                        ))
                {
                    return Err("Overlapping or ambiguously named PE sections".into());
                }
            }
        }
        if !sections.iter().any(|s| s.executable && s.raw_size != 0) {
            return Err("No file-backed executable section".into());
        }
        Ok(Self {
            data,
            identity,
            sections,
            headers,
        })
    }
    pub fn bytes(&self, rva: usize, len: usize) -> Option<&'a [u8]> {
        let end = rva.checked_add(len)?;
        if rva < self.headers {
            return (end <= self.headers).then(|| &self.data[rva..end]);
        }
        let s = self
            .sections
            .iter()
            .find(|s| rva >= s.rva as usize && rva < s.rva as usize + s.raw_size as usize)?;
        if end > s.rva as usize + s.raw_size as usize {
            return None;
        }
        let raw = s.raw_offset as usize + rva - s.rva as usize;
        self.data.get(raw..raw.checked_add(len)?)
    }
    pub fn section(&self, name: &str) -> Option<(usize, usize)> {
        self.sections
            .iter()
            .find(|s| s.name == name)
            .map(|s| (s.rva as usize, s.virtual_size as usize))
    }
    pub fn executable_sections(&self) -> impl Iterator<Item = (&Section, &'a [u8])> {
        self.sections
            .iter()
            .filter(|s| s.executable)
            .filter_map(|s| {
                let size = if s.virtual_size == 0 {
                    s.raw_size
                } else {
                    s.raw_size.min(s.virtual_size)
                } as usize;
                (size != 0).then(|| {
                    (
                        s,
                        &self.data[s.raw_offset as usize..s.raw_offset as usize + size],
                    )
                })
            })
    }
}
