//! Block-texture manifest and PNG checks.
//!
//! `build.rs` and the game both use this module. It does not know which
//! namespace is the default: a namespaced id is `namespace:name`, and the file
//! is `assets/<namespace>/textures/block/<file>.png` relative to the assets root.

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

pub const MIN_TILE: u32 = 16;
pub const MAX_TILE: u32 = 512;
pub const MAX_LAYERS: u32 = 1024;
pub const MAX_VARIANTS: usize = 16;
/// Typical GTX 1660 Ti 2D texture-array limit. The quad field stops at 1024.
pub const HARDWARE_ARRAY_LAYERS: u32 = 2048;

pub const FACE_ALL: u8 = 0;
pub const FACE_TOP: u8 = 1;
pub const FACE_BOTTOM: u8 = 2;
pub const FACE_SIDE: u8 = 3;
pub const FACE_EAST: u8 = 4;
pub const FACE_WEST: u8 = 5;
pub const FACE_SOUTH: u8 = 6;
pub const FACE_NORTH: u8 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerImage {
    pub namespace: String,
    pub file: String,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceBinding {
    pub id: String,
    pub face: u8,
    pub base_layer: u32,
    pub variant_count: u8,
    pub random_top: bool,
}

#[derive(Clone, Debug)]
pub struct TexturePack {
    pub tile_size: u32,
    pub layers: Vec<LayerImage>,
    pub bindings: Vec<FaceBinding>,
}

pub fn load_pack(manifest: &Path, assets_root: &Path) -> Result<TexturePack, String> {
    let text = fs::read_to_string(manifest)
        .map_err(|error| format!("missing texture manifest {}: {error}", manifest.display()))?;
    parse_manifest(&text, assets_root, manifest)
}

pub fn parse_manifest(
    text: &str,
    assets_root: &Path,
    manifest: &Path,
) -> Result<TexturePack, String> {
    #[derive(Clone)]
    struct Group {
        namespace: String,
        files: Vec<String>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut binds: Vec<(String, u8, String, bool, usize)> = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let line_number = number + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens[0] {
            "layer" => {
                if tokens.len() < 3 {
                    return Err(format!(
                        "{}:{line_number}: layer needs a namespace and at least one file",
                        manifest.display()
                    ));
                }
                let namespace = parse_name(tokens[1], manifest, line_number, "namespace")?;
                let mut files = Vec::new();
                for token in &tokens[2..] {
                    files.push(parse_file(token, manifest, line_number)?);
                }
                if files.len() > MAX_VARIANTS {
                    return Err(format!(
                        "{}:{line_number}: {} variants exceeds the 16 stored on a quad",
                        manifest.display(),
                        files.len()
                    ));
                }
                groups.push(Group { namespace, files });
            }
            "bind" => {
                if tokens.len() < 4 {
                    return Err(format!(
                        "{}:{line_number}: bind needs an id, a face, and a file",
                        manifest.display()
                    ));
                }
                let id = parse_id(tokens[1], manifest, line_number)?;
                let face = parse_face(tokens[2], manifest, line_number)?;
                let mut file = None;
                let mut random_top = false;
                for token in &tokens[3..] {
                    if *token == "rotate" {
                        random_top = true;
                        continue;
                    }
                    if file.is_some() {
                        return Err(format!(
                            "{}:{line_number}: bind lists one file; variants belong on the layer line",
                            manifest.display()
                        ));
                    }
                    file = Some(parse_file(token, manifest, line_number)?);
                }
                let Some(file) = file else {
                    return Err(format!(
                        "{}:{line_number}: bind needs a file",
                        manifest.display()
                    ));
                };
                binds.push((id, face, file, random_top, line_number));
            }
            other => {
                return Err(format!(
                    "{}:{line_number}: unknown manifest keyword {other}",
                    manifest.display()
                ));
            }
        }
    }
    if groups.is_empty() {
        return Err(format!(
            "{}: manifest has no layer lines",
            manifest.display()
        ));
    }

    let mut layers = Vec::new();
    let mut tile_size = None;
    for group in &groups {
        for file in &group.files {
            let path = texture_path(assets_root, &group.namespace, file);
            let (size, rgba) = decode_png_file(&path)?;
            match tile_size {
                None => tile_size = Some(size),
                Some(expected) if expected != size => {
                    return Err(format!(
                        "{} is {size}x{size}; every tile in the pack must be {expected}x{expected}",
                        path.display()
                    ));
                }
                Some(_) => {}
            }
            if layers.len() as u32 >= MAX_LAYERS {
                return Err(format!(
                    "{}: {MAX_LAYERS} is the 10-bit layer field. A GTX 1660 Ti class array is typically {HARDWARE_ARRAY_LAYERS}, which is larger than the quad can store",
                    path.display()
                ));
            }
            layers.push(LayerImage {
                namespace: group.namespace.clone(),
                file: file.clone(),
                rgba,
            });
        }
    }
    let tile_size = tile_size.unwrap_or(0);

    let mut bindings = Vec::new();
    let mut seen = Vec::new();
    for (id, face, file, random_top, line_number) in binds {
        let key = (id.clone(), face);
        if seen.contains(&key) {
            return Err(format!(
                "{}:{line_number}: duplicate manifest entry {id} face {}",
                manifest.display(),
                face_name(face)
            ));
        }
        seen.push(key);
        let namespace = id.split_once(':').unwrap().0;
        let mut base = None;
        let mut count = 0u32;
        let mut cursor = 0u32;
        for group in &groups {
            if group.namespace == namespace && group.files.first() == Some(&file) {
                base = Some(cursor);
                count = group.files.len() as u32;
                break;
            }
            cursor += group.files.len() as u32;
        }
        let Some(base_layer) = base else {
            return Err(format!(
                "{}:{line_number}: {file} is not the first file of a {namespace} layer group",
                manifest.display()
            ));
        };
        bindings.push(FaceBinding {
            id,
            face,
            base_layer,
            variant_count: count as u8,
            random_top,
        });
    }

    let mut unmapped = Vec::new();
    if assets_root.is_dir() {
        for namespace_dir in fs::read_dir(assets_root).map_err(|error| error.to_string())? {
            let namespace_dir = namespace_dir.map_err(|error| error.to_string())?;
            if !namespace_dir.path().is_dir() {
                continue;
            }
            let block_dir = namespace_dir.path().join("textures").join("block");
            if !block_dir.is_dir() {
                continue;
            }
            let namespace = namespace_dir.file_name().to_string_lossy().to_string();
            for entry in fs::read_dir(&block_dir).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
                    continue;
                }
                let file = path.file_name().unwrap().to_string_lossy().to_string();
                let referenced = layers
                    .iter()
                    .any(|layer| layer.namespace == namespace && layer.file == file);
                if !referenced {
                    unmapped.push(path);
                }
            }
        }
    }
    if let Some(path) = unmapped.first() {
        return Err(format!(
            "unmapped texture file {}; add a layer line to {}",
            path.display(),
            manifest.display()
        ));
    }

    Ok(TexturePack {
        tile_size,
        layers,
        bindings,
    })
}

pub fn texture_path(assets_root: &Path, namespace: &str, file: &str) -> PathBuf {
    assets_root
        .join(namespace)
        .join("textures")
        .join("block")
        .join(file)
}

pub fn decode_png_file(path: &Path) -> Result<(u32, Vec<u8>), String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("missing texture file {}: {error}", path.display()))?;
    decode_png_bytes(&bytes, &path.display().to_string())
}

pub fn decode_png_bytes(bytes: &[u8], label: &str) -> Result<(u32, Vec<u8>), String> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("{label}: {error}"))?;
    let (width, height, color, depth) = {
        let info = reader.info();
        (info.width, info.height, info.color_type, info.bit_depth)
    };
    if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
        return Err(format!("{label} is {color:?}/{depth:?}; expected RGBA8"));
    }
    if width != height || !width.is_power_of_two() || !(MIN_TILE..=MAX_TILE).contains(&width) {
        return Err(format!(
            "{label} is {width}x{height}; expected a square power-of-two RGBA8 tile from {MIN_TILE} to {MAX_TILE}"
        ));
    }
    let mut rgba = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let frame = reader
        .next_frame(&mut rgba)
        .map_err(|error| format!("{label}: {error}"))?;
    rgba.truncate(frame.buffer_size());
    if rgba.len() != (width * height * 4) as usize {
        return Err(format!(
            "{label} decoded to {} bytes, expected {} for a square RGBA8 tile",
            rgba.len(),
            width * height * 4
        ));
    }
    Ok((width, rgba))
}

pub fn encode_png(rgba: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut encoded), size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(encoded)
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn parse_name(token: &str, manifest: &Path, line: usize, kind: &str) -> Result<String, String> {
    if token.is_empty()
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!(
            "{}:{line}: {kind} {token} must be lowercase letters, digits, or underscore",
            manifest.display()
        ));
    }
    Ok(token.to_string())
}

fn parse_id(token: &str, manifest: &Path, line: usize) -> Result<String, String> {
    let Some((namespace, name)) = token.split_once(':') else {
        return Err(format!(
            "{}:{line}: {token} is not a namespaced id like strata:stone",
            manifest.display()
        ));
    };
    if namespace.contains(':') || name.contains(':') || token.matches(':').count() != 1 {
        return Err(format!(
            "{}:{line}: {token} must contain one colon",
            manifest.display()
        ));
    }
    parse_name(namespace, manifest, line, "namespace")?;
    parse_name(name, manifest, line, "name")?;
    Ok(token.to_string())
}

fn parse_file(token: &str, manifest: &Path, line: usize) -> Result<String, String> {
    if token.contains('/') || token.contains('\\') || !token.ends_with(".png") {
        return Err(format!(
            "{}:{line}: {token} must be a png file name with no path",
            manifest.display()
        ));
    }
    let stem = token.trim_end_matches(".png");
    if stem.is_empty()
        || !stem.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
    {
        return Err(format!(
            "{}:{line}: {token} has unsupported characters",
            manifest.display()
        ));
    }
    Ok(token.to_string())
}

fn parse_face(token: &str, manifest: &Path, line: usize) -> Result<u8, String> {
    Ok(match token {
        "all" => FACE_ALL,
        "top" => FACE_TOP,
        "bottom" => FACE_BOTTOM,
        "side" => FACE_SIDE,
        "east" => FACE_EAST,
        "west" => FACE_WEST,
        "south" => FACE_SOUTH,
        "north" => FACE_NORTH,
        other => {
            return Err(format!(
                "{}:{line}: unknown face {other}",
                manifest.display()
            ));
        }
    })
}

pub fn face_name(face: u8) -> &'static str {
    match face {
        FACE_ALL => "all",
        FACE_TOP => "top",
        FACE_BOTTOM => "bottom",
        FACE_SIDE => "side",
        FACE_EAST => "east",
        FACE_WEST => "west",
        FACE_SOUTH => "south",
        FACE_NORTH => "north",
        _ => "unknown",
    }
}

/// Stable integer hash shared with `variant_hash` in `world.wgsl`.
/// Negative coordinates use the two's-complement bit pattern.
pub fn variant_hash(x: i32, y: i32, z: i32, face: u32) -> u32 {
    let mut hash = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ (z as u32).wrapping_mul(0xC2B2_AE3D)
        ^ face.wrapping_mul(0x27D4_EB2F);
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x7FEB_352D);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x846C_A68B);
    hash ^= hash >> 16;
    hash
}

pub fn top_spin(hash: u32) -> (u32, bool) {
    ((hash >> 16) & 3, (hash >> 18) & 1 == 1)
}

pub fn variant_layer(base: u32, count: u32, hash: u32) -> u32 {
    base + hash % count.max(1)
}
