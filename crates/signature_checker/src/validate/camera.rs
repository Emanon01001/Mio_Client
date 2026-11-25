//! Static checks for the two optional Freecam camera anchors. Only supported
//! instruction fields are checked; live camera pools/optics still need runtime
//! validation by the client before any camera writes are allowed.
use super::{one, result, values};
use crate::{compatibility::Code, pe::u32_at, signatures::*, Check};
use std::collections::BTreeMap;

const RENDER_PROLOGUE: [u8; 12] = [
    0x41, 0x57, 0x41, 0x56, 0x41, 0x54, 0x56, 0x57, 0x53, 0x48, 0x81, 0xEC,
];

fn position(code: &impl Code) -> Result<BTreeMap<String, u64>, String> {
    let b = one(code, CAMERA_POSITION_COPY_SIGNATURE, 50)?;
    // The native copy reads/writes XYZ and two quaternion lanes separately.
    for (lane, expected) in [0x40, 0x44, 0x48, 0x3c, 0x30].into_iter().enumerate() {
        if b[lane * 10 + 4] != expected || b[lane * 10 + 9] != expected {
            return Err("Camera position/quaternion copy fields changed".into());
        }
    }
    Ok(values(&[("position", 0x40), ("quaternion", 0x30)]))
}

fn render(code: &impl Code) -> Result<BTreeMap<String, u64>, String> {
    let b = one(code, RENDER_CAMERA_ORIGIN_SIGNATURE, 52)?;
    let origin = u32_at(b, 14).ok_or("Truncated origin displacement")? as usize;
    let copy = u32_at(b, 48).ok_or("Truncated origin copy displacement")? as usize;
    // Layout::camera_origin defaults to 0x64C in the client. A static pass
    // must not imply that a changed wildcard operand is supported there.
    if b[4] != 0x48
        || b[9] != 0x40
        || origin != 0x64c
        || !copy.is_multiple_of(4)
        || !(origin + 12..origin + 0x40).contains(&copy)
        || u32_at(b, 22) != Some((origin + 8) as u32)
        || u32_at(b, 28) != Some((origin + 8) as u32)
        || u32_at(b, 34) != Some((copy + 8) as u32)
        || u32_at(b, 41) != Some(origin as u32)
    {
        return Err(
            "Render camera XYZ/origin/copy fields do not agree with supported layout".into(),
        );
    }
    let site = code.find(RENDER_CAMERA_ORIGIN_SIGNATURE)[0];
    let (text, size) = code.section(".text").ok_or("Missing code section")?;
    let end = text.checked_add(size).ok_or("Code section overflow")?;
    if site < text || site.checked_add(b.len()).is_none_or(|at| at > end) {
        return Err("Render camera anchor is outside .text".into());
    }
    // Match the runtime's bounded backwards search, including the preceding
    // padding byte. Never cross a section or inspect virtual-only file bytes.
    let start = site.saturating_sub(0x1000).max(text.saturating_add(1));
    let function = (start..site)
        .rev()
        .find(|&at| {
            code.bytes(at - 1, 1) == Some(&[0xcc])
                && code.bytes(at, RENDER_PROLOGUE.len()) == Some(&RENDER_PROLOGUE)
        })
        .ok_or("Render camera setup prologue was not found within 0x1000 bytes")?;
    Ok(values(&[
        ("function_rva", function),
        ("anchor_rva", site),
        ("camera_position", 0x40),
        ("origin", origin),
        ("origin_copy", copy),
    ]))
}

pub(super) fn checks(code: &impl Code) -> [Check; 2] {
    [
        result("Freecam camera position layout", position(code)),
        result("Freecam render camera layout", render(code)),
    ]
}
