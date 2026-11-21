//! Field cross-checks used by both the runtime and offline checker.
use crate::compatibility::Code;
use crate::signatures::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Raw {
    pub actor_ref: usize,
    pub id: usize,
    pub image: usize,
    pub data: usize,
    pub length: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Model {
    pub geometry: usize,
    pub animations: usize,
}
fn unique<'a>(code: &'a impl Code, pattern: &str, size: usize) -> Result<&'a [u8], String> {
    let matches = code.find(pattern);
    let [rva] = matches.as_slice() else {
        return Err(format!("{}: 候補{}件", name(pattern), matches.len()));
    };
    code.bytes(*rva, size)
        .ok_or_else(|| format!("{}: 命令が範囲外です", name(pattern)))
}
fn name(pattern: &str) -> &str {
    CATALOG
        .iter()
        .find(|e| e.pattern == pattern)
        .map_or("signature", |e| e.name)
}
pub(crate) fn word(bytes: &[u8], at: usize) -> Result<usize, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or("命令が短すぎます")?
            .try_into()
            .unwrap(),
    ) as usize)
}
fn field(value: usize) -> Result<usize, String> {
    if value <= 0x4000 && value.is_multiple_of(8) {
        Ok(value)
    } else {
        Err(format!("フィールド位置が不正です: {value:#x}"))
    }
}
pub(crate) fn decode_raw(
    actor: &[u8],
    id: &[u8],
    image: &[u8],
    copy: &[u8],
) -> Result<Raw, String> {
    let actor_ref = field(word(actor, 0x33)?)?;
    if actor_ref < 0x20 {
        return Err("Actor参照位置が小さすぎます".into());
    }
    let id_base = field(
        usize::from(*id.get(3).ok_or("ID命令が短すぎます")?)
            .checked_sub(16)
            .ok_or("ID長さの位置が不正です")?,
    )?;
    if id.get(7).copied().map(usize::from) != Some(id_base + 24)
        || id.get(14).copied().map(usize::from) != Some(id_base)
        || id.get(27).copied().map(usize::from) != Some(id_base)
    {
        return Err("ID文字列の参照位置が一致しません".into());
    }
    let image_base = field(
        usize::try_from(-i16::from(
            *image.get(6).ok_or("画像命令が短すぎます")? as i8
        ))
        .map_err(|_| "画像位置が負数です")?,
    )?;
    if image_base < id_base + 32 {
        return Err("画像とIDの領域が重複します".into());
    }
    if word(copy, 0x4c)? != image_base || word(copy, 0x41)? != image_base + 16 {
        return Err("画像ヘッダーの参照位置が一致しません".into());
    }
    let data = word(copy, 0x53)?;
    let length = word(copy, 0x7b)?;
    let deleter = word(copy, 0x66)?;
    if data != word(copy, 0x5a)?
        || deleter != word(copy, 0x74)?
        || data != image_base + 32
        || length != data + 8
        || deleter + 8 != data
    {
        return Err("画像BlobのABIが一致しません".into());
    }
    Ok(Raw {
        actor_ref,
        id: id_base,
        image: image_base,
        data: data - image_base,
        length: length - image_base,
    })
}
pub(crate) fn derive_raw(code: &impl Code) -> Result<Raw, String> {
    let actor = unique(code, ACTOR_SKIN_REF_SIGNATURE, 0x5a)?;
    let id = unique(code, SKIN_ID_FIELD_SIGNATURE, 33)?;
    let image = unique(code, SKIN_IMAGE_FIELD_SIGNATURE, 37)?;
    // Skin and cape share code shape. Match the independently captured image
    // field, never pick the first occurrence or a fixed function address.
    let copies = code.find(SKIN_IMAGE_COPY_SIGNATURE);
    if copies.len() >= 256 {
        return Err("画像コピーの候補が上限を超えています".into());
    }
    let matching: Vec<_> = copies
        .into_iter()
        .filter_map(|at| code.bytes(at, 0x7f))
        .filter_map(|copy| decode_raw(actor, id, image, copy).ok())
        .collect();
    match matching.as_slice() {
        [only] => Ok(*only),
        _ => Err(format!("画像レイアウト: 検証済み候補{}件", matching.len())),
    }
}
pub(crate) fn derive_model(code: &impl Code) -> Result<Model, String> {
    let geometry = field(word(unique(code, SKIN_GEOMETRY_FIELD_SIGNATURE, 42)?, 2)?)?;
    let animations = field(word(unique(code, SKIN_ANIMATION_FIELD_SIGNATURE, 42)?, 1)?)?;
    if geometry != animations + 24 {
        return Err("スキンのモデル配置が一致しません".into());
    }
    Ok(Model {
        geometry,
        animations,
    })
}
