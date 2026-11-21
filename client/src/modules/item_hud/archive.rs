//! The supplied game's uncompressed binary resource archive: 16-byte header,
//! 256-byte directory records, followed by the concatenated payloads.
const MAGIC: &[u8; 8] = b"\x7d\x27\x25\xb1\xa0\x52\x70\x26";
pub(super) struct Entry<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

pub(super) fn entries(data: &[u8]) -> Result<Vec<Entry<'_>>, &'static str> {
    let u32_at = |at| {
        data.get(at..at + 4)
            .and_then(|s| s.try_into().ok())
            .map(u32::from_le_bytes)
    };
    if data.get(..8) != Some(MAGIC.as_slice()) || u32_at(12) != Some(1) {
        return Err("未対応のリソースアーカイブです");
    }
    let count = u32_at(8).ok_or("ヘッダーが不足しています")? as usize;
    if count > 16384 {
        return Err("アーカイブの項目数が多すぎます");
    }
    let start = 16 + count * 256;
    if start > data.len() {
        return Err("アーカイブの目次が不足しています");
    }
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let record = 16 + i * 256;
        let length = data[record] as usize;
        if length == 0 || length > 247 {
            return Err("不正なファイル名の長さです");
        }
        let name = std::str::from_utf8(&data[record + 1..record + 1 + length])
            .map_err(|_| "ファイル名がUTF-8ではありません")?;
        // Names are logical keys only; no archive entry is written to disk.
        if name.contains(['/', '\\', '\0']) || name == "." || name == ".." {
            return Err("不正なリソース名です");
        }
        let offset = u32_at(record + 248).unwrap() as usize;
        let length = u32_at(record + 252).unwrap() as usize;
        let begin = start
            .checked_add(offset)
            .ok_or("アーカイブの範囲が不正です")?;
        let end = begin
            .checked_add(length)
            .ok_or("アーカイブの範囲が不正です")?;
        let payload = data
            .get(begin..end)
            .ok_or("アーカイブのデータが不足しています")?;
        result.push(Entry {
            name,
            data: payload,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_exact_payloads_and_rejects_bad_tables_ranges_and_names() {
        let mut data = vec![0; 16 + 256];
        data[..8].copy_from_slice(MAGIC);
        data[8..12].copy_from_slice(&1u32.to_le_bytes());
        data[12..16].copy_from_slice(&1u32.to_le_bytes());
        data[16] = 5;
        data[17..22].copy_from_slice(b"a.png");
        data[268..272].copy_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(b"abc");
        let found = entries(&data).unwrap();
        assert_eq!((found[0].name, found[0].data), ("a.png", b"abc".as_slice()));
        assert!(entries(&data[..270]).is_err());
        data[264..268].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(entries(&data).is_err());
        data[264..268].fill(0);
        data[17] = b'/';
        assert!(entries(&data).is_err());
        data[16] = 255;
        assert!(entries(&data).is_err());
        assert!(entries(&[]).is_err());
    }
}
