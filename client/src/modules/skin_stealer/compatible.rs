//! Remap verified box meshes into the classic wide-arm UV layout.
//! Persona atlases and animated faces have independent UVs: resizing/cropping
//! the atlas would produce a valid PNG with the wrong skin.
use super::skins::geometry::{Model, Texture};
use serde_json::Value;

struct Part {
    uv: [usize; 2],
    size: [usize; 3],
    base: Option<usize>,
}
fn part(name: &str) -> Option<Part> {
    let (uv, size, base) = match name {
        "head" => ([0, 0], [8, 8, 8], Some(0)),
        "hat" => ([32, 0], [8, 8, 8], None),
        "body" => ([16, 16], [8, 12, 4], Some(1)),
        "jacket" => ([16, 32], [8, 12, 4], None),
        "rightarm" => ([40, 16], [4, 12, 4], Some(2)),
        "rightsleeve" => ([40, 32], [4, 12, 4], None),
        "leftarm" => ([32, 48], [4, 12, 4], Some(3)),
        "leftsleeve" => ([48, 48], [4, 12, 4], None),
        "rightleg" => ([0, 16], [4, 12, 4], Some(4)),
        "rightpants" => ([0, 32], [4, 12, 4], None),
        "leftleg" => ([16, 48], [4, 12, 4], Some(5)),
        "leftpants" => ([0, 48], [4, 12, 4], None),
        _ => return None,
    };
    Some(Part { uv, size, base })
}
fn vector<const N: usize>(v: &Value) -> Result<[f64; N], String> {
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or("モデルの座標数が不正です")?;
    let mut out = [0.; N];
    for (i, v) in a.iter().enumerate() {
        out[i] = v
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or("モデルの座標が不正です")?;
    }
    Ok(out)
}
fn index(v: &Value) -> Result<usize, String> {
    usize::try_from(v.as_u64().ok_or("モデルの添字が不正です")?)
        .map_err(|_| "モデルの添字が大きすぎます".into())
}
// up/down/-X/+X/-Z/+Z, in the same box-UV convention as Bedrock.
fn face(n: [f64; 3]) -> Result<usize, String> {
    [
        [0., 1., 0.],
        [0., -1., 0.],
        [-1., 0., 0.],
        [1., 0., 0.],
        [0., 0., -1.],
        [0., 0., 1.],
    ]
    .iter()
    .position(|v| (0..3).all(|i| (n[i] - v[i]).abs() < 0.001))
    .ok_or("斜めのモデル面は通常スキンへ変換できません".into())
}
fn project(f: usize, p: [f64; 3]) -> [f64; 2] {
    match f {
        0 => [p[0], 1. - p[2]],
        1 => [1. - p[0], 1. - p[2]],
        2 => [1. - p[2], 1. - p[1]],
        3 => [p[2], 1. - p[1]],
        4 => [p[0], 1. - p[1]],
        _ => [1. - p[0], 1. - p[1]],
    }
}
fn rect(part: &Part, face: usize) -> [usize; 4] {
    let [u, v] = part.uv;
    let [w, h, d] = part.size;
    match face {
        0 => [u + d, v, w, d],
        1 => [u + d + w, v, w, d],
        2 => [u, v + d, d, h],
        3 => [u + d + w, v + d, d, h],
        4 => [u + d, v + d, w, h],
        _ => [u + 2 * d + w, v + d, w, h],
    }
}
type FaceUvs = [[f64; 2]; 4];

fn mesh_faces(mesh: &Value, texture: &Texture) -> Result<[FaceUvs; 6], String> {
    let normalized_uvs = mesh["normalized_uvs"]
        .as_bool()
        .ok_or("UV座標の単位が不明です")?;
    let width = f64::from(texture.width);
    let height = f64::from(texture.height);
    let positions = mesh["positions"]
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or("頂点が不正です")?;
    let normals = mesh["normals"]
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or("法線が不正です")?;
    let uvs = mesh["uvs"]
        .as_array()
        .filter(|v| v.len() <= 256)
        .ok_or("UVが不正です")?;
    let polys = mesh["polys"]
        .as_array()
        .filter(|v| v.len() == 6)
        .ok_or("箱型以外のモデルは通常スキンへ変換できません")?;
    let vertices = positions
        .iter()
        .map(vector::<3>)
        .collect::<Result<Vec<_>, _>>()?;
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    if (0..3).any(|i| hi[i] - lo[i] < 0.1 || hi[i] - lo[i] > 40.) {
        return Err("モデルの大きさが未対応です".into());
    }
    let mut used = [false; 6];
    let mut faces = [[[0.; 2]; 4]; 6];
    for poly in polys {
        let poly = poly
            .as_array()
            .filter(|v| v.len() == 4)
            .ok_or("四角形以外の面は未対応です")?;
        let mut corners = [[0.; 2]; 4];
        let mut corner_set = [false; 4];
        let mut direction = None;
        for vertex in poly {
            let v = vertex
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or("面の添字が不正です")?;
            let p = vertices.get(index(&v[0])?).ok_or("頂点の範囲外です")?;
            let n = vector::<3>(normals.get(index(&v[1])?).ok_or("法線の範囲外です")?)?;
            let f = face(n)?;
            if direction.is_some_and(|d| d != f) {
                return Err("法線が一致しません".into());
            }
            direction = Some(f);
            let normalized = std::array::from_fn(|i| (p[i] - lo[i]) / (hi[i] - lo[i]));
            // Every vertex must be an actual box corner, not a curved face.
            if normalized
                .iter()
                .any(|v| v.abs().min((v - 1.).abs()) > 0.001)
            {
                return Err("箱型以外の頂点は未対応です".into());
            }
            let axis = match f {
                0 | 1 => 1,
                2 | 3 => 0,
                _ => 2,
            };
            let side = if matches!(f, 0 | 3 | 5) { 1. } else { 0. };
            if (normalized[axis] - side).abs() > 0.001 {
                return Err("面の位置が不正です".into());
            }
            let q = project(f, normalized);
            let corner = (q[0].round() as usize) + 2 * (q[1].round() as usize);
            let [u, v] = vector::<2>(uvs.get(index(&v[2])?).ok_or("UVの範囲外です")?)?;
            let (u, v) = if normalized_uvs {
                (u * width, v * height)
            } else {
                (u, v)
            };
            if !(0.0..=width).contains(&u)
                || !(0.0..=height).contains(&v)
                || corner >= 4
                || corner_set[corner]
            {
                return Err("UVの配置が未対応です".into());
            }
            corners[corner] = [u, height - v];
            corner_set[corner] = true;
        }
        let f = direction.ok_or("面がありません")?;
        if used[f] || !corner_set.iter().all(|v| *v) {
            return Err("モデル面が重複しています".into());
        }
        used[f] = true;
        faces[f] = corners;
    }
    Ok(faces)
}

fn texel_span(uv: &FaceUvs) -> [f64; 2] {
    let length = |a: usize, b: usize| (uv[a][0] - uv[b][0]).hypot(uv[a][1] - uv[b][1]);
    [
        length(0, 1).max(length(2, 3)),
        length(0, 2).max(length(1, 3)),
    ]
}

fn texel(texture: &Texture, uv: &FaceUvs, u: f64, v: f64) -> [u8; 4] {
    let p: [f64; 2] = std::array::from_fn(|k| {
        let top = uv[0][k] * (1. - u) + uv[1][k] * u;
        let bottom = uv[2][k] * (1. - u) + uv[3][k] * u;
        top * (1. - v) + bottom * v
    });
    let x = (p[0] as usize).min(texture.width as usize - 1);
    let y = (p[1] as usize).min(texture.height as usize - 1);
    let src = (y * texture.width as usize + x) * 4;
    texture.rgba[src..src + 4].try_into().unwrap()
}

fn sample(
    texture: &Texture,
    uv: &FaceUvs,
    xy: [usize; 2],
    size: [usize; 2],
    samples: [usize; 2],
) -> [u8; 4] {
    let at = |i: usize, j: usize| {
        let u = (xy[0] as f64 + (i as f64 + 0.5) / samples[0] as f64) / size[0] as f64;
        let v = (xy[1] as f64 + (j as f64 + 0.5) / samples[1] as f64) / size[1] as f64;
        texel(texture, uv, u, v)
    };
    if samples == [1, 1] {
        return at(0, 0);
    }
    let mut rgb = [0u32; 3];
    let mut alpha = 0u32;
    for j in 0..samples[1] {
        for i in 0..samples[0] {
            let color = at(i, j).map(u32::from);
            alpha += color[3];
            for k in 0..3 {
                rgb[k] += color[k] * color[3];
            }
        }
    }
    if alpha == 0 {
        return [0; 4];
    }
    let count = (samples[0] * samples[1]) as u32;
    let [r, g, b] = rgb.map(|n| ((n + alpha / 2) / alpha) as u8);
    [r, g, b, ((alpha + count / 2) / count) as u8]
}

fn composite(dst: &mut [u8], src: [u8; 4]) {
    let source = u32::from(src[3]) * 255;
    let background = u32::from(dst[3]) * (255 - u32::from(src[3]));
    let total = source + background;
    if total == 0 || background == 0 {
        dst.copy_from_slice(&src);
        return;
    }
    for i in 0..3 {
        let color = u32::from(src[i]) * source + u32::from(dst[i]) * background;
        dst[i] = ((color + total / 2) / total) as u8;
    }
    dst[3] = ((total + 127) / 255) as u8;
}

struct MappedPart<'a> {
    part: Part,
    texture: &'a Texture,
    faces: [FaceUvs; 6],
}

impl MappedPart<'_> {
    fn needs_high_resolution(&self) -> bool {
        self.faces.iter().enumerate().any(|(face, uv)| {
            let [_, _, w, h] = rect(&self.part, face);
            let [u, v] = texel_span(uv);
            u > w as f64 + 0.01 || v > h as f64 + 0.01
        })
    }

    fn draw(&self, out: &mut [u8], scale: usize) {
        for (face, uv) in self.faces.iter().enumerate() {
            let [x, y, w, h] = rect(&self.part, face).map(|n| n * scale);
            let size = [w, h];
            let span = texel_span(uv);
            let samples = std::array::from_fn(|i| {
                ((span[i] / size[i] as f64 - 0.001).ceil() as usize).clamp(1, 8)
            });
            for j in 0..h {
                for i in 0..w {
                    let src = sample(self.texture, uv, [i, j], size, samples);
                    let dst = ((y + j) * 64 * scale + x + i) * 4;
                    composite(&mut out[dst..dst + 4], src);
                }
            }
        }
    }
}
/// Resolve the body and its corresponding animated face in draw order.
fn geometries(model: &Model) -> Result<[Option<&Value>; 2], String> {
    let entries = model.json["minecraft:geometry"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if entries.len() > 16 {
        return Err("モデルの数が上限を超えています".into());
    }
    let face_id = model
        .identifier
        .strip_prefix("geometry.persona_")
        .map(|id| format!("geometry.animated_face_persona-{id}"));
    let mut selected = [None, None];
    for g in entries {
        let Some(id) = g["description"]["identifier"].as_str() else {
            continue;
        };
        let slot = if id == model.identifier {
            0
        } else if face_id.as_deref() == Some(id)
            || id == "geometry.animated_face"
            || (face_id.is_none() && id.starts_with("geometry.animated_face_"))
        {
            1
        } else {
            continue;
        };
        if selected[slot].replace(g).is_some() {
            return Err("同じIDのモデルが複数あります".into());
        }
    }
    Ok(selected)
}

pub(super) fn needs_conversion(model: &Model) -> Result<bool, String> {
    Ok(geometries(model)?
        .into_iter()
        .flatten()
        .flat_map(|g| g["bones"].as_array().into_iter().flatten())
        .any(|bone| bone.get("poly_mesh").is_some()))
}

pub(super) fn arm_label(id: Option<&str>) -> &'static str {
    match id {
        Some("geometry.humanoid.customSlim") => "・細い腕（Alex型）",
        Some("geometry.humanoid" | "geometry.humanoid.custom") => "・幅広の腕（Steve型）",
        _ => "",
    }
}

pub(super) fn convert(model: &Model, body: &Texture) -> Result<(u32, Vec<u8>), String> {
    if super::skins::rgba_len(body.width, body.height) != Some(body.rgba.len()) {
        return Err("元画像が不正です".into());
    }
    if let Some(t) = &model.face {
        if super::skins::rgba_len(t.width, t.height) != Some(t.rgba.len()) {
            return Err("顔画像が不正です".into());
        }
    }
    let [body_geometry, face_geometry] = geometries(model)?;
    let mut parts = Vec::new();
    let mut required = [false; 6];
    for (slot, g) in [body_geometry, face_geometry]
        .into_iter()
        .enumerate()
        .filter_map(|(slot, g)| g.map(|g| (slot, g)))
    {
        let animated = slot == 1;
        let texture = if animated {
            model
                .face
                .as_ref()
                .ok_or("このスキンの顔画像がありません")?
        } else {
            body
        };
        if g["description"]["texture_width"].as_f64() != Some(texture.width as f64)
            || g["description"]["texture_height"].as_f64() != Some(texture.height as f64)
        {
            return Err("モデルと画像の寸法が一致しません".into());
        }
        let bones = g["bones"]
            .as_array()
            .filter(|v| v.len() <= 128)
            .ok_or("モデルの部品数が未対応です")?;
        let mut names = std::collections::HashSet::new();
        for b in bones {
            if b.get("rotation")
                .is_some_and(|v| vector::<3>(v).map_or(true, |v| v.iter().any(|n| n.abs() > 0.001)))
            {
                return Err("回転したモデル部品は未対応です".into());
            }
            let Some(mesh) = b.get("poly_mesh") else {
                if b["cubes"].as_array().is_some_and(|c| !c.is_empty()) {
                    return Err("このモデルの立体部品は未対応です".into());
                }
                continue;
            };
            let name = b["name"].as_str().unwrap_or_default();
            let p = part(name).ok_or(
                "独自形状の部品は通常スキンへ変換できません。元テクスチャは別ボタンで保存できます",
            )?;
            if animated && !matches!(name, "head" | "hat") {
                return Err("顔画像に対応しない部品があります".into());
            }
            if !names.insert(name) {
                return Err("同じ名前のスキン部品が複数あります".into());
            }
            if let Some(i) = p.base {
                required[i] = true;
            }
            parts.push(MappedPart {
                part: p,
                texture,
                faces: mesh_faces(mesh, texture)?,
            });
        }
    }
    if !required.iter().all(|v| *v) {
        return Err("頭・胴体・手足のモデルが揃っていません".into());
    }
    let scale = if parts.iter().any(MappedPart::needs_high_resolution) {
        2
    } else {
        1
    };
    let side = 64 * scale;
    let mut out = vec![0; side * side * 4];
    for part in &parts {
        part.draw(&mut out, scale);
    }
    // Classic base layer must be opaque; overlays retain their original alpha.
    for name in ["head", "body", "rightarm", "leftarm", "rightleg", "leftleg"] {
        let p = part(name).unwrap();
        for f in 0..6 {
            let [x, y, w, h] = rect(&p, f).map(|n| n * scale);
            for j in y..y + h {
                for i in x..x + w {
                    out[(j * side + i) * 4 + 3] = 255;
                }
            }
        }
    }
    Ok((side as u32, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn box_mesh() -> Value {
        let positions = serde_json::json!([
            [-1, 1, -1],
            [1, 1, -1],
            [1, 1, 1],
            [-1, 1, 1],
            [-1, -1, 1],
            [1, -1, 1],
            [1, -1, -1],
            [-1, -1, -1],
            [-1, -1, 1],
            [-1, -1, -1],
            [-1, 1, -1],
            [-1, 1, 1],
            [1, -1, -1],
            [1, -1, 1],
            [1, 1, 1],
            [1, 1, -1],
            [-1, -1, -1],
            [1, -1, -1],
            [1, 1, -1],
            [-1, 1, -1],
            [1, -1, 1],
            [-1, -1, 1],
            [-1, 1, 1],
            [1, 1, 1]
        ]);
        let normals = serde_json::json!([
            [0, 1, 0],
            [0, -1, 0],
            [-1, 0, 0],
            [1, 0, 0],
            [0, 0, -1],
            [0, 0, 1]
        ]);
        let mut polys = vec![];
        let mut uvs = vec![];
        // Independent canonical box-UV layout for an 8x8x8 head.
        for (f, [u, v]) in [
            [8., 0.],
            [16., 0.],
            [0., 8.],
            [16., 8.],
            [8., 8.],
            [24., 8.],
        ]
        .into_iter()
        .enumerate()
        {
            let corners = if f == 1 {
                [[u + 8., v], [u, v], [u, v + 8.], [u + 8., v + 8.]]
            } else {
                [[u, v + 8.], [u + 8., v + 8.], [u + 8., v], [u, v]]
            };
            for [x, y] in corners {
                uvs.push(serde_json::json!([x / 64., 1. - y / 64.]));
            }
            polys.push(serde_json::json!((0..4)
                .map(|i| [f * 4 + i, f, f * 4 + i])
                .collect::<Vec<_>>()));
        }
        serde_json::json!({"normalized_uvs":true,"positions":positions,"normals":normals,"uvs":uvs,"polys":polys})
    }
    #[test]
    fn all_faces_use_the_classic_uv_positions_without_flipping() {
        let mut rgba = vec![0; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let i = (y * 64 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[x as u8, y as u8, 91, 127]);
            }
        }
        let t = Texture {
            width: 64,
            height: 64,
            rgba,
        };
        let mut out = vec![0; 64 * 64 * 4];
        let p = part("head").unwrap();
        let mesh = box_mesh();
        let faces = mesh_faces(&mesh, &t).unwrap();
        MappedPart {
            part: part("head").unwrap(),
            texture: &t,
            faces,
        }
        .draw(&mut out, 1);
        for f in 0..6 {
            let [x, y, w, h] = rect(&p, f);
            for j in y..y + h {
                for i in x..x + w {
                    let at = (j * 64 + i) * 4;
                    assert_eq!(&out[at..at + 4], &t.rgba[at..at + 4], "face {f} at {i},{j}");
                }
            }
        }
        let mut invalid = mesh.clone();
        invalid["polys"][1] = invalid["polys"][0].clone();
        assert!(mesh_faces(&invalid, &t).is_err());
        let mut invalid = mesh.clone();
        invalid["uvs"][0] = serde_json::json!([2, 0]);
        assert!(mesh_faces(&invalid, &t).is_err());
        let mut pixels = mesh.clone();
        pixels["normalized_uvs"] = Value::Bool(false);
        for uv in pixels["uvs"].as_array_mut().unwrap() {
            let [u, v] = vector::<2>(uv).unwrap();
            *uv = serde_json::json!([u * 64., v * 64.]);
        }
        assert_eq!(mesh_faces(&pixels, &t).unwrap(), faces);
    }
    #[test]
    fn invalid_geometry_cannot_be_reported_as_an_importable_skin() {
        let texture = Texture {
            width: 64,
            height: 64,
            rgba: vec![0; 64 * 64 * 4],
        };
        let model = Model {
            json: serde_json::json!({"minecraft:geometry":[]}),
            identifier: "missing".into(),
            face: None,
        };
        assert!(convert(&model, &texture).is_err());
        assert!(face([0.5, 0.5, 0.]).is_err());
        assert_eq!(face([0., 0., -1.]).unwrap(), 4);
        assert_eq!(project(4, [0., 1., 0.]), [0., 0.]);
        assert_eq!(project(5, [1., 1., 1.]), [0., 0.]);
    }
    #[test]
    fn only_mesh_geometries_need_conversion() {
        let mesh = box_mesh();
        // Persona: mesh parts in its own geometry.
        let persona = Model {
            identifier: "geometry.persona_1".into(),
            face: None,
            json: serde_json::json!({"minecraft:geometry":[{"description":{"identifier":"geometry.persona_1"},"bones":[{"name":"head","poly_mesh":mesh}]}]}),
        };
        assert!(needs_conversion(&persona).unwrap());
        // Classic: the standard humanoid (no definition at all, or box parts).
        let standard = Model {
            identifier: "geometry.humanoid.customSlim".into(),
            face: None,
            json: serde_json::json!({}),
        };
        assert!(!needs_conversion(&standard).unwrap());
        assert_eq!(arm_label(Some(&standard.identifier)), "・細い腕（Alex型）");
        let boxes = Model {
            identifier: "geometry.custom".into(),
            face: None,
            json: serde_json::json!({"minecraft:geometry":[{"description":{"identifier":"geometry.custom"},"bones":[{"name":"head","cubes":[{"origin":[0,0,0],"size":[8,8,8],"uv":[0,0]}]}]}]}),
        };
        assert!(!needs_conversion(&boxes).unwrap());
        assert_eq!(arm_label(Some(&boxes.identifier)), "");
        // Mesh parts in another skin's geometry do not count.
        let other = Model {
            identifier: "geometry.custom".into(),
            ..persona
        };
        assert!(!needs_conversion(&other).unwrap());
    }

    #[test]
    fn compatible_output_requires_all_limbs_and_keeps_overlay_alpha() {
        let mesh = box_mesh();
        let bones = [
            "head", "body", "rightarm", "leftarm", "rightleg", "leftleg", "hat",
        ]
        .map(|name| serde_json::json!({"name":name,"poly_mesh":mesh}));
        let mut model = Model {
            identifier: "test".into(),
            face: None,
            json: serde_json::json!({"minecraft:geometry":[{"description":{"identifier":"test","texture_width":64,"texture_height":64},"bones":bones}]}),
        };
        let t = Texture {
            width: 64,
            height: 64,
            rgba: vec![127; 64 * 64 * 4],
        };
        let (side, rgba) = convert(&model, &t).unwrap();
        // A head-sized source for the 4-texel limbs needs the 128px output.
        assert_eq!(side, 128);
        assert_eq!(rgba[(16 * 128 + 16) * 4 + 3], 255);
        assert_eq!(rgba[(16 * 128 + 80) * 4 + 3], 127);
        model.json["minecraft:geometry"][0]["bones"][0]["rotation"] = serde_json::json!([0, 1, 0]);
        assert!(convert(&model, &t).is_err());
        model.json["minecraft:geometry"][0]["bones"][0]
            .as_object_mut()
            .unwrap()
            .remove("rotation");
        model.json["minecraft:geometry"][0]["bones"]
            .as_array_mut()
            .unwrap()
            .remove(5);
        assert!(convert(&model, &t).is_err());
    }

    #[test]
    fn the_matching_face_composites_over_the_body_in_either_json_order() {
        let mesh = box_mesh();
        let bones = ["head", "body", "rightarm", "leftarm", "rightleg", "leftleg"]
            .map(|name| serde_json::json!({"name": name, "poly_mesh": mesh}));
        let entry = |id: &str, bones: &[Value]| serde_json::json!({"description":{"identifier":id,"texture_width":64,"texture_height":64},"bones":bones});
        let body_geometry = entry("geometry.persona_test", &bones);
        let face_geometry = entry("geometry.animated_face_persona-test", &bones[..1]);
        let unrelated = entry("geometry.animated_face_persona-other", &bones);
        let body = Texture {
            width: 64,
            height: 64,
            rgba: [200, 60, 10, 255].repeat(64 * 64),
        };
        let mut face = Texture {
            width: 64,
            height: 64,
            rgba: [255, 0, 255, 0].repeat(64 * 64),
        };
        face.rgba[(8 * 64 + 9) * 4..(8 * 64 + 10) * 4].copy_from_slice(&[0, 100, 210, 128]);
        let model = Model {
            json: serde_json::json!({"minecraft:geometry":[face_geometry, unrelated, body_geometry]}),
            face: Some(face),
            identifier: "geometry.persona_test".into(),
        };
        let (size, rgba) = convert(&model, &body).unwrap();
        assert_eq!(size, 128);
        let at = (16 * 128 + 16) * 4;
        assert_eq!(&rgba[at..at + 4], [200, 60, 10, 255]);
        let at = (16 * 128 + 18) * 4;
        assert_eq!(&rgba[at..at + 4], [100, 80, 110, 255]);
    }

    #[test]
    fn downsampling_preserves_color_at_transparent_edges() {
        let texture = Texture {
            width: 2,
            height: 1,
            rgba: vec![240, 20, 30, 255, 0, 0, 255, 0],
        };
        let uv = [[0., 0.], [2., 0.], [0., 1.], [2., 1.]];
        assert_eq!(
            sample(&texture, &uv, [0, 0], [1, 1], [2, 1]),
            [240, 20, 30, 128]
        );
        assert_eq!(
            sample(&texture, &uv, [0, 0], [2, 1], [1, 1]),
            [240, 20, 30, 255]
        );
        assert_eq!(
            sample(&texture, &uv, [1, 0], [2, 1], [1, 1]),
            [0, 0, 255, 0]
        );
    }
    #[test]
    #[ignore = "requires local Persona geometry and face capture"]
    fn persona_probe() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let cache = root.join(".analysis-cache/skin-export-20261118");
        let json: Value =
            serde_json::from_slice(&std::fs::read(cache.join("efe-geometry-f8.json")).unwrap())
                .unwrap();
        // Use the captured geometry and face with a synthetic body atlas of
        // the original dimensions to exercise its UV bounds.
        let description = &json["minecraft:geometry"][0]["description"];
        let width = description["texture_width"].as_f64().unwrap() as u32;
        let height = description["texture_height"].as_f64().unwrap() as u32;
        let body = Texture {
            width,
            height,
            rgba: [16, 120, 220, 255].repeat((width * height) as usize),
        };
        let model = Model {
            json,
            identifier: "geometry.persona_d5dea2ce83df5c6d-12".into(),
            face: Some(Texture {
                width: 32,
                height: 64,
                rgba: std::fs::read(cache.join("efe-face.rgba")).unwrap(),
            }),
        };
        let (side, converted) = convert(&model, &body).unwrap();
        assert_eq!(side, 64);
        assert_eq!(converted.len(), 64 * 64 * 4);
        let png = super::super::storage::encode(side, side, &converted).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (64, 64));
        assert_eq!(&decoded[..info.buffer_size()], &converted);
    }
}
