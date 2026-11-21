//! Capture field displacements from matching instructions, and cross-check
//! independent uses. A match alone never authorizes an arbitrary offset.
#[cfg(test)]
use crate::modules::game::layout::Code;
use crate::modules::game::layout::Running;
use crate::skin_layout_checks::derive_model;
pub(crate) use crate::skin_layout_checks::{derive_raw, Model, Raw};
#[cfg(test)]
use crate::{
    signatures::*,
    skin_layout_checks::{decode_raw, word},
};
use std::sync::OnceLock;

static RAW: OnceLock<Result<Raw, String>> = OnceLock::new();
pub(crate) fn raw() -> Result<Raw, String> {
    RAW.get_or_init(|| {
        if !super::super::known_build() {
            return Err("基本レイアウトの互換性を確認できません（compatdebug）".into());
        }
        derive_raw(&Running::new().ok_or("実行ファイルを参照できません")?)
    })
    .clone()
}
static MODEL: OnceLock<Result<Model, String>> = OnceLock::new();
pub(crate) fn model() -> Result<Model, String> {
    MODEL.get_or_init(|| {
        raw()?;
        if super::super::build_name().is_none() {
            return Err("このビルドのモデルJSONのABIは未検証です。「元テクスチャを保存」を使用してください".into());
        }
        derive_model(&Running::new().ok_or("実行ファイルを参照できません")?)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake {
        actor: Vec<u8>,
        id: Vec<u8>,
        image: Vec<u8>,
        copies: Vec<Vec<u8>>,
    }
    impl Code for Fake {
        fn find(&self, p: &str) -> Vec<usize> {
            match p {
                ACTOR_SKIN_REF_SIGNATURE => vec![0],
                SKIN_ID_FIELD_SIGNATURE => vec![1],
                SKIN_IMAGE_FIELD_SIGNATURE => vec![2],
                SKIN_IMAGE_COPY_SIGNATURE => (100..100 + self.copies.len()).collect(),
                _ => vec![],
            }
        }
        fn bytes(&self, at: usize, n: usize) -> Option<&[u8]> {
            match at {
                0 => self.actor.get(..n),
                1 => self.id.get(..n),
                2 => self.image.get(..n),
                100.. => self.copies.get(at - 100)?.get(..n),
                _ => None,
            }
        }
        fn section(&self, _: &str) -> Option<(usize, usize)> {
            None
        }
        fn base(&self) -> usize {
            0
        }
    }
    fn fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut a = vec![0; 0x5a];
        a[0x33..0x37].copy_from_slice(&0xad8u32.to_le_bytes());
        let mut id = vec![0; 33];
        id[3] = 0x30;
        id[7] = 0x38;
        id[14] = 0x20;
        id[27] = 0x20;
        let mut image = vec![0; 37];
        image[6] = 0x80;
        let mut c = vec![0; 0x7f];
        for (at, value) in [
            (0x41, 0x90u32),
            (0x4c, 0x80),
            (0x53, 0xa0),
            (0x5a, 0xa0),
            (0x66, 0x98),
            (0x74, 0x98),
            (0x7b, 0xa8),
        ] {
            c[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        (a, id, image, c)
    }
    #[test]
    fn relocates_fields_and_refuses_inconsistent_abi() {
        let (mut a, mut id, mut image, mut c) = fixture();
        assert_eq!(
            decode_raw(&a, &id, &image, &c).unwrap(),
            Raw {
                actor_ref: 0xad8,
                id: 0x20,
                image: 0x80,
                data: 0x20,
                length: 0x28
            }
        );
        a[0x33..0x37].copy_from_slice(&0xb18u32.to_le_bytes());
        id[3] = 0x50;
        id[7] = 0x58;
        id[14] = 0x40;
        id[27] = 0x40;
        image[6] = 0xa0; // subtract -96: image moved to +0x60
        for at in [0x41, 0x4c, 0x53, 0x5a, 0x66, 0x74, 0x7b] {
            let n = word(&c, at).unwrap() - 0x20;
            c[at..at + 4].copy_from_slice(&(n as u32).to_le_bytes());
        }
        let shifted = decode_raw(&a, &id, &image, &c).unwrap();
        assert_eq!(
            (shifted.actor_ref, shifted.id, shifted.image),
            (0xb18, 0x40, 0x60)
        );
        c[0x5a] += 8;
        assert!(decode_raw(&a, &id, &image, &c).is_err());
        assert!(decode_raw(&a[..20], &id, &image, &c).is_err());
        let (a, mut id, image, c) = fixture();
        id[27] += 8;
        assert!(decode_raw(&a, &id, &image, &c).is_err());
    }
    #[test]
    fn skin_and_cape_candidates_are_cross_checked_instead_of_picking_first() {
        let (actor, id, image, skin) = fixture();
        let mut cape = skin.clone();
        for at in [0x41, 0x4c, 0x53, 0x5a, 0x66, 0x74, 0x7b] {
            let n = word(&cape, at).unwrap() + 0x30;
            cape[at..at + 4].copy_from_slice(&(n as u32).to_le_bytes());
        }
        let mut code = Fake {
            actor,
            id,
            image,
            copies: vec![cape, skin.clone()],
        };
        assert_eq!(derive_raw(&code).unwrap().image, 0x80);
        code.copies.push(skin);
        assert!(derive_raw(&code).is_err());
        code.copies.clear();
        assert!(derive_raw(&code).is_err());
    }
}
