//! Runtime view of the embedded block-texture pack.
//!
//! The manifest and PNG paths are namespace-neutral. `strata:` is only the id
//! used by this game's blocks.

use std::path::Path;

use crate::{
    materials,
    texture_pack::{
        self, FACE_ALL, FACE_BOTTOM, FACE_EAST, FACE_NORTH, FACE_SIDE, FACE_SOUTH, FACE_TOP,
        FACE_WEST,
    },
    world::Block,
};

include!(concat!(env!("OUT_DIR"), "/block_textures_gen.rs"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuadMaterial {
    pub layer: u32,
    pub variant_count: u8,
    pub random_top: bool,
}

pub fn block_texture_id(block: Block) -> &'static str {
    match block {
        Block::Air | Block::Stone => "strata:stone",
        Block::Grass => "strata:grass",
        Block::Dirt => "strata:dirt",
        Block::Sand => "strata:sand",
        Block::Water => "strata:water",
        Block::Wood => "strata:wood",
        Block::Leaves => "strata:leaves",
        Block::Snow => "strata:snow",
        Block::Clay => "strata:clay",
        Block::Crystal => "strata:crystal",
        Block::Basalt => "strata:basalt",
        Block::CoalOre => "strata:coal_ore",
        Block::IronOre => "strata:iron_ore",
        Block::GoldOre => "strata:gold_ore",
        Block::DiamondOre => "strata:diamond_ore",
    }
}

pub fn quad_material(block: Block, normal: [i32; 3]) -> QuadMaterial {
    let id = block_texture_id(block);
    for face in preferred_faces(normal) {
        if let Some(binding) = FACES
            .iter()
            .find(|binding| binding.id == id && binding.face == face)
        {
            return QuadMaterial {
                layer: binding.base_layer,
                variant_count: binding.variant_count,
                random_top: binding.random_top,
            };
        }
    }
    debug_assert!(
        FACES.iter().any(|binding| binding.id == id),
        "no texture binding for {id} {normal:?}"
    );
    QuadMaterial {
        layer: 0,
        variant_count: 1,
        random_top: false,
    }
}

fn preferred_faces(normal: [i32; 3]) -> [u8; 3] {
    if normal[1] > 0 {
        [FACE_TOP, FACE_ALL, FACE_ALL]
    } else if normal[1] < 0 {
        [FACE_BOTTOM, FACE_ALL, FACE_ALL]
    } else if normal[0] > 0 {
        [FACE_EAST, FACE_SIDE, FACE_ALL]
    } else if normal[0] < 0 {
        [FACE_WEST, FACE_SIDE, FACE_ALL]
    } else if normal[2] > 0 {
        [FACE_SOUTH, FACE_SIDE, FACE_ALL]
    } else {
        [FACE_NORTH, FACE_SIDE, FACE_ALL]
    }
}

pub fn packed_rgba() -> Result<Vec<u8>, String> {
    let stride = (TILE_SIZE * TILE_SIZE * 4) as usize;
    let mut raw = Vec::with_capacity(stride * LAYERS.len());
    for layer in LAYERS {
        let (_size, rgba) = texture_pack::decode_png_bytes(layer.png, layer.file)?;
        if rgba.len() != stride {
            return Err(format!(
                "{} decoded to {} bytes; the pack tile is {TILE_SIZE}",
                layer.file,
                rgba.len()
            ));
        }
        raw.extend(rgba);
    }
    apply_debug_override(&mut raw)?;
    Ok(raw)
}

pub fn gpu_mips() -> Result<(u32, Vec<Vec<u8>>), String> {
    let base = packed_rgba()?;
    Ok((
        TILE_SIZE,
        materials::mip_chain_from(&base, TILE_SIZE, LAYER_COUNT),
    ))
}

#[cfg(debug_assertions)]
fn apply_debug_override(raw: &mut [u8]) -> Result<(), String> {
    let Some(root) = std::env::var_os("STRATA_BLOCK_TEXTURES") else {
        return Ok(());
    };
    let root = Path::new(&root);
    let stride = (TILE_SIZE * TILE_SIZE * 4) as usize;
    for (index, layer) in LAYERS.iter().enumerate() {
        let path = texture_pack::texture_path(root, layer.namespace, layer.file);
        if !path.exists() {
            continue;
        }
        let (size, rgba) = texture_pack::decode_png_file(&path)?;
        if size != TILE_SIZE || rgba.len() != stride {
            return Err(format!(
                "debug texture {} is {size}x{size}; this pack is {TILE_SIZE}x{TILE_SIZE}. Save files were not opened",
                path.display()
            ));
        }
        let start = index * stride;
        raw[start..start + stride].copy_from_slice(&rgba);
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
fn apply_debug_override(_raw: &mut [u8]) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::materials::{self, layer_for};
    use crate::texture_pack::{
        self, HARDWARE_ARRAY_LAYERS, MAX_LAYERS, TexturePack, top_spin, variant_hash, variant_layer,
    };
    use std::collections::HashSet;
    use std::fs;

    #[test]
    fn shipped_pngs_match_the_procedural_tiles_and_mip_count_follows_size() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let pack = texture_pack::load_pack(&assets.join("block-textures.manifest"), &assets)
            .expect("shipped pack");
        let procedural = materials::rgba_layers();
        let stride = (materials::TILE_SIZE * materials::TILE_SIZE * 4) as usize;
        assert_eq!(pack.tile_size, 32);
        assert_eq!(pack.layers.len(), materials::LAYER_COUNT as usize);
        for (index, layer) in pack.layers.iter().enumerate() {
            assert_eq!(
                layer.rgba,
                &procedural[index * stride..(index + 1) * stride],
                "{} does not match the procedural layer",
                layer.file
            );
        }
        let loaded = packed_rgba().unwrap();
        assert_eq!(loaded, procedural);
        let mips = materials::mip_chain_from(&loaded, pack.tile_size, pack.layers.len() as u32);
        assert_eq!(mips.len(), 6);
        assert_eq!(mips, materials::mip_chain());
        const {
            assert!(LAYER_COUNT <= MAX_LAYERS);
            assert!(MAX_LAYERS <= HARDWARE_ARRAY_LAYERS);
        }
    }

    #[test]
    fn every_block_face_uses_the_same_layer_as_the_procedural_map() {
        let mut id = 0u8;
        while let Some(block) = Block::from_id(id) {
            if block != Block::Air {
                for normal in [
                    [1, 0, 0],
                    [-1, 0, 0],
                    [0, 1, 0],
                    [0, -1, 0],
                    [0, 0, 1],
                    [0, 0, -1],
                ] {
                    let material = quad_material(block, normal);
                    assert_eq!(
                        material.layer,
                        layer_for(block, normal),
                        "{block:?} {normal:?}"
                    );
                    assert_eq!(material.variant_count, 1);
                    assert!(!material.random_top);
                }
            }
            id += 1;
        }
    }

    #[test]
    fn variant_hash_matches_the_wgsl_source_and_selects_more_than_one_slot() {
        let shader = include_str!("shaders/world.wgsl");
        for needle in [
            "bitcast<u32>(x) * 0x9E3779B1u",
            "bitcast<u32>(y) * 0x85EBCA77u",
            "bitcast<u32>(z) * 0xC2B2AE3Du",
            "face * 0x27D4EB2Fu",
            "h ^= h >> 16u",
            "h *= 0x7FEB352Du",
            "h ^= h >> 15u",
            "h *= 0x846CA68Bu",
            "input.world - normal * 0.5",
            "(hash >> 16u) & 3u",
            "(hash >> 18u) & 1u",
            "choice % count",
        ] {
            assert!(shader.contains(needle), "WGSL hash missing {needle}");
        }
        assert_eq!(variant_hash(0, 0, 0, 2), variant_hash(0, 0, 0, 2));
        assert_ne!(variant_hash(-3, 4, -8, 2), variant_hash(-3, 4, -8, 0));
        let mut seen = HashSet::new();
        for x in -5..6 {
            for z in -5..6 {
                let hash = variant_hash(x, 2, z, 2);
                let layer = variant_layer(8, 4, hash);
                assert!((8..12).contains(&layer));
                seen.insert(layer);
                assert_eq!(top_spin(hash).0, (hash >> 16) & 3);
            }
        }
        assert!(
            seen.len() > 1,
            "a four-variant run must not collapse to one slot"
        );
    }

    #[test]
    fn manifest_rejects_bad_size_format_duplicates_and_unmapped_files() {
        let root = std::env::temp_dir().join(format!(
            "strata-textures-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let write = |namespace: &str, file: &str, png: &[u8]| {
            let dir = root.join(namespace).join("textures").join("block");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), png).unwrap();
        };
        let rgba = vec![10u8; 16 * 16 * 4];
        let png16 = texture_pack::encode_png(&rgba, 16).unwrap();
        write("example", "brick.png", &png16);
        write("example", "brick-b.png", &png16);
        let manifest = root.join("pack.manifest");
        fs::write(
            &manifest,
            "layer example brick.png brick-b.png\nbind example:brick all brick.png\n",
        )
        .unwrap();
        let pack = texture_pack::load_pack(&manifest, &root).unwrap();
        assert_eq!(pack.tile_size, 16);
        assert_eq!(pack.layers.len(), 2);
        assert_eq!(pack.bindings[0].variant_count, 2);
        assert_eq!(pack.bindings[0].base_layer, 0);
        assert_eq!(pack.bindings[0].id, "example:brick");

        write("example", "extra.png", &png16);
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("unmapped"), "{error}");
        fs::remove_file(root.join("example/textures/block/extra.png")).unwrap();
        fs::remove_file(root.join("example/textures/block/brick-b.png")).unwrap();

        fs::write(
            &manifest,
            "layer example brick.png\nlayer example brick.png\nbind example:brick all brick.png\n",
        )
        .unwrap();
        // The second layer reuses the same file. That is a second array slice,
        // not a duplicate bind. A duplicate bind is the failure below.
        assert!(texture_pack::load_pack(&manifest, &root).is_ok());
        fs::write(
            &manifest,
            "layer example brick.png\nbind example:brick all brick.png\nbind example:brick all brick.png\n",
        )
        .unwrap();
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("duplicate"), "{error}");

        fs::write(
            &manifest,
            "layer example missing.png\nbind example:brick all missing.png\n",
        )
        .unwrap();
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("missing texture file"), "{error}");

        let rgb = encode_rgb(&vec![1u8; 16 * 16 * 3], 16);
        write("example", "brick.png", &rgb);
        fs::write(
            &manifest,
            "layer example brick.png\nbind example:brick all brick.png\n",
        )
        .unwrap();
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("RGBA8"), "{error}");

        let odd = texture_pack::encode_png(&vec![2u8; 24 * 24 * 4], 24).unwrap();
        write("example", "brick.png", &odd);
        fs::write(
            &manifest,
            "layer example brick.png\nbind example:brick all brick.png\n",
        )
        .unwrap();
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("24x24"), "{error}");
        let big = texture_pack::encode_png(&vec![0u8; 1024 * 1024 * 4], 1024).unwrap();
        // encode itself allows any size; the decoder enforces the pack range.
        write("example", "brick.png", &big);
        let error = texture_pack::load_pack(&manifest, &root).unwrap_err();
        assert!(error.contains("1024x1024"), "{error}");

        let _typed: TexturePack = pack;
        let _ = fs::remove_dir_all(&root);
    }

    fn encode_rgb(rgb: &[u8], size: u32) -> Vec<u8> {
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut encoded), size, size);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(rgb).unwrap();
            writer.finish().unwrap();
        }
        encoded
    }
}
