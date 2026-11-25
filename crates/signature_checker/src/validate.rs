use crate::{
    compatibility::{self, Build, Code},
    pe::u32_at,
    signatures::*,
    skin_layout_checks, Check, CheckState, Scanned,
};
use std::collections::{BTreeMap, BTreeSet};
mod camera;

fn one<'a>(code: &'a impl Code, pattern: &str, len: usize) -> Result<&'a [u8], String> {
    let hits = code.find(pattern);
    let [at] = hits.as_slice() else {
        return Err(format!("Expected one anchor, found {}", hits.len()));
    };
    code.bytes(*at, len)
        .ok_or_else(|| "Instruction bytes are not file-backed".into())
}
fn result(name: &str, value: Result<BTreeMap<String, u64>, String>) -> Check {
    match value {
        Ok(values) => Check {
            name: name.into(),
            state: CheckState::Passed,
            detail: "Static instruction/field cross-check passed".into(),
            rvas_or_values: values,
        },
        Err(detail) => Check {
            name: name.into(),
            state: CheckState::Failed,
            detail,
            rvas_or_values: BTreeMap::new(),
        },
    }
}
fn values(entries: &[(&str, usize)]) -> BTreeMap<String, u64> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), *v as u64))
        .collect()
}
fn option(code: &impl Code, sensitivity: bool) -> Result<BTreeMap<String, u64>, String> {
    let (anchor, id_at) = if sensitivity {
        (SENSITIVITY_NAME_ANCHOR, SENSITIVITY_ID_AT)
    } else {
        (GAMMA_NAME_ANCHOR, GAMMA_ID_AT)
    };
    let mut options = Vec::new();
    let anchors = code.find(anchor);
    if !sensitivity && anchors.len() != 1 {
        return Err(format!("Gamma name anchors: {}", anchors.len()));
    }
    for at in anchors {
        if sensitivity {
            let bytes = code.bytes(at, 7).ok_or("Truncated sensitivity reference")?;
            let relative = i32::from_le_bytes(bytes[3..7].try_into().unwrap());
            let Some(name) = (at + 7).checked_add_signed(relative as isize) else {
                continue;
            };
            if code.bytes(name, 16) != Some(&SENSITIVITY_NAME[..16]) {
                continue;
            }
        }
        // A registration can be near the end of a section; use its available
        // suffix, exactly as the runtime uses the available image suffix.
        let bytes = (1..=0x200)
            .rev()
            .find_map(|len| code.bytes(at, len))
            .ok_or("Missing registration bytes")?;
        if let Some(id) = compatibility::option_id_after(bytes) {
            options.push(id);
        }
    }
    let [id] = options.as_slice() else {
        return Err(format!("Option registrations: {}", options.len()));
    };
    let exact = if sensitivity {
        sensitivity_signature(*id)
    } else {
        gamma_signature(*id)
    };
    let found = code.find(&exact);
    let [getter] = found.as_slice() else {
        return Err(format!("ID-filtered getter candidates: {}", found.len()));
    };
    if code.bytes(getter + id_at, 4) != Some(&id.to_le_bytes()) {
        return Err("Getter ID does not match registration".into());
    }
    Ok(values(&[
        ("option_id", *id as usize),
        ("getter_rva", *getter),
    ]))
}
pub(crate) fn checks(code: &Scanned<'_>, known: Option<&Build>) -> (Vec<Check>, BTreeSet<String>) {
    let mut checks = Vec::new();
    let mut resolved = BTreeSet::new();
    let addresses = compatibility::derive(code);
    for (name, derived, expected) in [
        (
            "BlockSource vtable",
            addresses.source_vtable,
            known.map(|b| b.source_vtable),
        ),
        (
            "Air registry",
            addresses.air_registry,
            known.map(|b| b.air_registry),
        ),
        (
            "FishingHook vtable",
            addresses.hook_vtable,
            known.map(|b| b.hook_vtable),
        ),
        (
            "Inventory request manager vtable",
            addresses.inventory_manager_vtable,
            known.map(|b| b.inventory_manager_vtable),
        ),
        (
            "ItemStack vtable",
            addresses.item_stack_vtable,
            known.map(|b| b.item_stack_vtable),
        ),
        (
            "Request scope vtable 1",
            addresses.request_scope_vtables.map(|a| a[0]),
            known.map(|b| b.request_scope_vtables[0]),
        ),
        (
            "Request scope vtable 2",
            addresses.request_scope_vtables.map(|a| a[1]),
            known.map(|b| b.request_scope_vtables[1]),
        ),
    ] {
        checks.push(result(
            name,
            match derived {
                Some(rva) if expected.is_none_or(|old| old == rva) => Ok(values(&[("rva", rva)])),
                Some(rva) => Err(format!(
                    "Derived RVA 0x{rva:X} differs from recorded 0x{:X}",
                    expected.unwrap()
                )),
                None => Err("Cannot derive and cross-check this address".into()),
            },
        ));
    }
    checks.push(result(
        "Player/position offsets",
        (|| {
            let accessor = one(code, ACCESSOR_SIGNATURE, 43)?;
            let actor = one(code, ACTOR_SIGNATURE, 14)?;
            let client = accessor[32] as usize;
            let player = u32_at(accessor, 39).ok_or("Missing vtable displacement")? as usize;
            let state = u32_at(actor, 3).ok_or("Missing state displacement")? as usize;
            let bounds = u32_at(actor, 10).ok_or("Missing bounds displacement")? as usize;
            if [client, player, state, bounds]
                .iter()
                .any(|&v| !(8..=0x4000).contains(&v) || v % 8 != 0)
                || state == bounds
            {
                return Err("Unaligned, overlapping or out-of-range captured fields".into());
            }
            Ok(values(&[
                ("client_instance", client),
                ("player_vtable_slot", player),
                ("state", state),
                ("bounds", bounds),
            ]))
        })(),
    ));
    for (sensitivity, label, names) in [
        (
            false,
            "Gamma option/getter ID",
            ["GAMMA_NAME_ANCHOR", "GAMMA_GETTER_SHAPE"],
        ),
        (
            true,
            "Sensitivity option/getter ID",
            ["SENSITIVITY_NAME_ANCHOR", "SENSITIVITY_GETTER_SHAPE"],
        ),
    ] {
        let check = result(label, option(code, sensitivity));
        if check.state == CheckState::Passed {
            resolved.extend(names.map(str::to_string));
        }
        checks.push(check);
    }
    let metadata = result(
        "Metadata accessor fields",
        (|| {
            let found: Vec<_> = code
                .find(METADATA_ACCESSORS_SIGNATURE)
                .into_iter()
                .filter(|&at| {
                    code.bytes(at, 20)
                        .is_some_and(|b| b[3] == 0x0a && b[19] == 8)
                })
                .collect();
            let [at] = found.as_slice() else {
                return Err(format!(
                    "Verified metadata accessor candidates: {}",
                    found.len()
                ));
            };
            Ok(values(&[
                ("id_getter_rva", *at),
                ("kind_getter_rva", *at + 16),
            ]))
        })(),
    );
    if metadata.state == CheckState::Passed {
        resolved.insert("METADATA_ACCESSORS_SIGNATURE".into());
    }
    checks.push(metadata);
    checks.push(result(
        "Effect layout",
        (|| {
            let b = one(code, EFFECT_LAYOUT_SIGNATURE, 37)?;
            if u32_at(b, 3) != Some(0xa8) || b[17] != 38 || b[36] != 0x20 {
                return Err("Effect stride/count/amplifier layout changed".into());
            }
            Ok(values(&[
                ("stride", 0xa8),
                ("effect_limit", 38),
                ("amplifier", 0x20),
            ]))
        })(),
    ));
    checks.push(result(
        "Mining progress field",
        (|| {
            let stop = one(code, MINING_STOP_SIGNATURE, 36)?;
            let progress = one(code, MINING_PROGRESS_SIGNATURE, 14)?;
            let field = stop[19];
            if field != stop[35]
                || field != progress[8]
                || field != progress[13]
                || field < 8
                || field % 4 != 0
            {
                return Err("Mining stop/reset/read/write fields do not agree".into());
            }
            Ok(values(&[("progress", field as usize)]))
        })(),
    ));
    let raw = result(
        "Skin raw image layout",
        skin_layout_checks::derive_raw(code).map(|v| {
            values(&[
                ("actor_ref", v.actor_ref),
                ("id", v.id),
                ("image", v.image),
                ("blob_data", v.data),
                ("blob_length", v.length),
            ])
        }),
    );
    if raw.state == CheckState::Passed {
        resolved.insert("SKIN_IMAGE_COPY_SIGNATURE".into());
    }
    checks.push(raw);
    let mut model = result(
        "Skin model field layout",
        skin_layout_checks::derive_model(code)
            .map(|v| values(&[("geometry", v.geometry), ("animations", v.animations)])),
    );
    if known.is_none() && model.state == CheckState::Passed {
        model.state = CheckState::Review;
        model.detail =
            "Field cross-check passed; model JSON ABI is unverified on this build".into();
    }
    checks.push(model);
    checks.extend(camera::checks(code));
    (checks, resolved)
}
