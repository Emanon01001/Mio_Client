//! Bounded read-only copies of the analyzed MinecraftJson ABI. Used only
//! on an explicit compatible-skin export, never for periodic list updates.
use super::{read, ReadBatch};
use serde_json::{Map, Value};
use std::collections::HashSet;

pub(crate) struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
pub(crate) struct Model {
    pub json: Value,
    pub face: Option<Texture>,
    pub identifier: String,
}

unsafe fn c_string(m: &mut ReadBatch, address: usize) -> Option<String> {
    let mut bytes = Vec::new();
    for i in 0..1024 {
        let at = address.checked_add(i)?;
        if !m.readable(at, 1) {
            return None;
        }
        let byte = read::<u8>(at);
        if byte == 0 {
            return String::from_utf8(bytes).ok();
        }
        bytes.push(byte);
    }
    None
}
struct Reader {
    remaining: usize,
    active: HashSet<usize>,
}
impl Reader {
    unsafe fn value(&mut self, m: &mut ReadBatch, at: usize, depth: usize) -> Option<Value> {
        if depth > 32 || self.remaining == 0 || !m.readable(at, 16) || !self.active.insert(at) {
            return None;
        }
        self.remaining -= 1;
        let result = self.inner(m, at, depth);
        self.active.remove(&at);
        result
    }
    unsafe fn inner(&mut self, m: &mut ReadBatch, at: usize, depth: usize) -> Option<Value> {
        let data: usize = read(at);
        match read::<u8>(at + 8) {
            0 => Some(Value::Null),
            1 => Some(read::<i64>(at).into()),
            2 => Some(read::<u64>(at).into()),
            3 => serde_json::Number::from_f64(read(at)).map(Value::Number),
            4 => {
                if !m.readable(data, 8) {
                    return None;
                }
                c_string(m, read(data)).map(Value::String)
            }
            5 => match read::<u8>(at) {
                0 => Some(Value::Bool(false)),
                1 => Some(Value::Bool(true)),
                _ => None,
            },
            6 => {
                if !m.readable(data, 16) {
                    return None;
                }
                let begin: usize = read(data);
                let end: usize = read(data + 8);
                let bytes = end.checked_sub(begin)?;
                if bytes % 8 != 0
                    || bytes / 8 > self.remaining
                    || bytes > 65536
                    || (bytes != 0 && !m.readable(begin, bytes))
                {
                    return None;
                }
                let mut out = Vec::with_capacity(bytes / 8);
                for entry in (begin..end).step_by(8) {
                    out.push(self.value(m, read(entry), depth + 1)?);
                }
                Some(Value::Array(out))
            }
            7 => {
                if !m.readable(data, 16) {
                    return None;
                }
                let head: usize = read(data);
                let size: usize = read(data + 8);
                if size > self.remaining || !m.readable(head, 32) {
                    return None;
                }
                let mut stack = vec![read::<usize>(head + 8)];
                let mut seen = HashSet::new();
                let mut out = Map::new();
                while let Some(node) = stack.pop() {
                    if node == head {
                        continue;
                    }
                    if seen.len() >= size
                        || !seen.insert(node)
                        || !m.readable(node, 56)
                        || read::<u8>(node + 25) != 0
                    {
                        return None;
                    }
                    stack.push(read(node));
                    stack.push(read(node + 16));
                    let key = c_string(m, read(node + 32))?;
                    if out
                        .insert(key, self.value(m, node + 40, depth + 1)?)
                        .is_some()
                    {
                        return None;
                    }
                }
                (out.len() == size).then_some(Value::Object(out))
            }
            _ => None,
        }
    }
}

pub(super) unsafe fn capture(
    m: &mut ReadBatch,
    skin: usize,
    identifier: Option<&str>,
) -> Result<Model, String> {
    let identifier = identifier
        .filter(|id| !id.is_empty())
        .ok_or("モデルIDが読み取れません")?;
    let fields = super::layout::model()?;
    let raw = super::layout::raw()?;
    if !m.readable(skin, fields.geometry + 16) {
        return Err("スキンのモデル情報がありません".into());
    }
    let json = Reader {
        remaining: 8192,
        active: HashSet::new(),
    }
    .value(m, skin + fields.geometry, 0)
    .ok_or("モデル情報の読み取りに失敗しました。元テクスチャは別ボタンで保存できます")?;
    let mut face = None;
    if let Some((begin, end)) = m.span(skin + fields.animations, 0x40) {
        if end - begin > 16 * 0x40 {
            return Err("スキンのアニメーション数が上限を超えています".into());
        }
        for entry in (begin..end).step_by(0x40) {
            if read::<u32>(entry) != 1 {
                continue;
            }
            let image = super::read_texture(m, entry + 8, raw).ok_or("顔の画像を読み取れません")?;
            face = Some(Texture {
                width: image.width,
                height: image.height,
                rgba: super::copy_pixels(&image)?,
            });
            break;
        }
    }
    Ok(Model {
        json,
        face,
        identifier: identifier.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_json_copy_limits_cycles_types_and_strings() {
        let mut m = ReadBatch::new();
        let scalar = [42u64, 1];
        let mut reader = Reader {
            remaining: 8,
            active: HashSet::new(),
        };
        unsafe {
            assert_eq!(
                reader.value(&mut m, scalar.as_ptr() as usize, 0),
                Some(Value::from(42))
            );
            let text = b"hello\0";
            let ptr = [text.as_ptr() as usize];
            let string = [ptr.as_ptr() as u64, 4];
            assert_eq!(
                reader.value(&mut m, string.as_ptr() as usize, 0),
                Some(Value::from("hello"))
            );
            let mut array = [0usize, 6];
            let mut children = [array.as_ptr() as usize];
            let span = [
                children.as_mut_ptr() as usize,
                children.as_ptr() as usize + 8,
            ];
            array[0] = span.as_ptr() as usize;
            assert!(reader.value(&mut m, array.as_ptr() as usize, 0).is_none());
            assert!(reader.value(&mut m, scalar.as_ptr() as usize, 33).is_none());
            let invalid = [0u64, 99];
            assert!(reader.value(&mut m, invalid.as_ptr() as usize, 0).is_none());
            let nan = [f64::NAN.to_bits(), 3];
            assert!(reader.value(&mut m, nan.as_ptr() as usize, 0).is_none());
            reader.remaining = 0;
            assert!(reader.value(&mut m, scalar.as_ptr() as usize, 0).is_none());
        }
    }
    #[test]
    fn native_object_tree_validates_counts_links_and_keys() {
        let mut head = [0usize; 4];
        let mut node = [0usize; 7];
        let key = b"n\0";
        let sentinel = head.as_ptr() as usize;
        head[1] = node.as_ptr() as usize;
        node[0] = sentinel;
        node[1] = sentinel;
        node[2] = sentinel;
        node[4] = key.as_ptr() as usize;
        node[5] = 42;
        node[6] = 1;
        let mut map = [sentinel, 1usize];
        let value = [map.as_ptr() as usize, 7];
        let parse = || unsafe {
            Reader {
                remaining: 32,
                active: HashSet::new(),
            }
            .value(&mut ReadBatch::new(), value.as_ptr() as usize, 0)
        };
        std::hint::black_box((&head, &node, &map));
        assert_eq!(parse().unwrap(), serde_json::json!({"n":42}));
        map[1] = 2;
        assert!(parse().is_none());
        map[1] = 1;
        node[0] = node.as_ptr() as usize;
        std::hint::black_box((&node, &map));
        assert!(parse().is_none());
    }
}
