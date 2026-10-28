//! Original 32-pixel material albedo tiles authored as deterministic pixel rules.
//! Layers are RGBA8 sRGB, opaque, and independent of world seed. No asset files,
//! image downloads, random generator, clock, or external dependency are involved.

use crate::world::Block;

pub const TILE_SIZE: u32 = 32;
pub const LAYER_COUNT: u32 = 17;

const GRASS_TOP: u32 = 0;
const GRASS_SIDE: u32 = 1;
const SOIL: u32 = 2;
const LIMESTONE: u32 = 3;
const SAND: u32 = 4;
const WATER: u32 = 5;
const BARK: u32 = 6;
const ENDGRAIN: u32 = 7;
const LEAVES: u32 = 8;
const SNOW: u32 = 9;
const CLAY: u32 = 10;
const CRYSTAL: u32 = 11;
const BASALT: u32 = 12;
const COAL_ORE: u32 = 13;
const IRON_ORE: u32 = 14;
const GOLD_ORE: u32 = 15;
const DIAMOND_ORE: u32 = 16;

/// Axis-aligned block face to texture-array layer. Grass side v=0 is its top
/// edge; wood grain runs along increasing v. Air is never meshed and falls back
/// to limestone instead of an invalid layer.
pub fn layer_for(block: Block, normal: [i32; 3]) -> u32 {
    match block {
        Block::Grass if normal[1] > 0 => GRASS_TOP,
        Block::Grass if normal[1] < 0 => SOIL,
        Block::Grass => GRASS_SIDE,
        Block::Dirt => SOIL,
        Block::Stone | Block::Air => LIMESTONE,
        Block::Sand => SAND,
        Block::Water => WATER,
        Block::Wood if normal[1] != 0 => ENDGRAIN,
        Block::Wood => BARK,
        Block::Leaves => LEAVES,
        Block::Snow => SNOW,
        Block::Clay => CLAY,
        Block::Crystal => CRYSTAL,
        Block::Basalt => BASALT,
        Block::CoalOre => COAL_ORE,
        Block::IronOre => IRON_ORE,
        Block::GoldOre => GOLD_ORE,
        Block::DiamondOre => DIAMOND_ORE,
    }
}

/// Packed array order is layer, row, column, RGBA. Each layer has 4096 bytes.
/// These are full albedo colors, not multipliers for Block::color().
pub fn rgba_layers() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((TILE_SIZE * TILE_SIZE * LAYER_COUNT * 4) as usize);
    for layer in 0..LAYER_COUNT {
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                bytes.extend_from_slice(&pixel(layer, x as i32, y as i32));
            }
        }
    }
    bytes
}

/// Complete 32,16,8,4,2,1 mip chain, each level containing every packed layer.
/// RGB is decoded from sRGB, averaged in linear light, then encoded to sRGB.
/// Alpha stays opaque. No layer ever contributes to another layer's mip.
pub fn mip_chain() -> Vec<Vec<u8>> {
    let mut levels = vec![rgba_layers()];
    let mut size = TILE_SIZE;
    while size > 1 {
        let next = downsample(levels.last().expect("base level exists"), size);
        levels.push(next);
        size /= 2;
    }
    levels
}

fn downsample(source: &[u8], size: u32) -> Vec<u8> {
    let next = size / 2;
    let mut destination = vec![255; (next * next * LAYER_COUNT * 4) as usize];
    for layer in 0..LAYER_COUNT {
        for y in 0..next {
            for x in 0..next {
                let target = ((layer * next * next + y * next + x) * 4) as usize;
                for channel in 0..3 {
                    let mut sum = 0.0;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let source_index =
                                ((layer * size * size + (y * 2 + dy) * size + x * 2 + dx) * 4)
                                    as usize;
                            sum += srgb_to_linear(source[source_index + channel]);
                        }
                    }
                    destination[target + channel] = linear_to_srgb(sum * 0.25);
                }
            }
        }
    }
    destination
}

fn srgb_to_linear(byte: u8) -> f64 {
    let value = f64::from(byte) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f64) -> u8 {
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn pixel(layer: u32, x: i32, y: i32) -> [u8; 4] {
    let x = x.rem_euclid(TILE_SIZE as i32);
    let y = y.rem_euclid(TILE_SIZE as i32);
    let grain = (hash(x, y, layer + 17) % 7) as i16 - 3;
    let color = match layer {
        GRASS_TOP => meadow(x, y),
        GRASS_SIDE => grass_side(x, y),
        SOIL => soil(x, y),
        LIMESTONE => limestone(x, y),
        SAND => sand(x, y),
        WATER => water(x, y),
        BARK => bark(x, y),
        ENDGRAIN => endgrain(x, y),
        LEAVES => leaves(x, y),
        SNOW => snow(x, y),
        CLAY => clay(x, y),
        CRYSTAL => crystal(x, y),
        BASALT => basalt(x, y),
        COAL_ORE => ore(x, y, [32, 32, 34], 13),
        IRON_ORE => ore(x, y, [176, 96, 64], 29),
        GOLD_ORE => ore(x, y, [228, 184, 62], 47),
        DIAMOND_ORE => ore(x, y, [86, 228, 214], 61),
        _ => unreachable!("all generated layer IDs are defined"),
    };
    let color = shade(color, grain);
    [color[0], color[1], color[2], 255]
}

fn meadow(x: i32, y: i32) -> [u8; 3] {
    let mut color = [99, 137, 60];
    // Sixteen tufts, each with three intentionally shaped grass blades. Toroidal
    // distances carry silhouettes through tile edges instead of clipping them.
    for row in 0..4 {
        for col in 0..4 {
            let anchor_x = col * 8 + (hash(col, row, 31) % 5) as i32;
            let anchor_y = row * 8 + (hash(col, row, 53) % 5) as i32;
            let dx = wrap_distance(x, anchor_x);
            let dy = wrap_distance(y, anchor_y);
            if dx * dx + dy * dy < 14 {
                color = [87, 126, 55];
            }
            if (-4..=0).contains(&dy) && (dx == 0 || (dx == -1 && dy < -1) || (dx == 2 && dy > -3))
            {
                color = if dx == 0 {
                    [141, 169, 78]
                } else {
                    [117, 151, 65]
                };
            }
            if dy == 1 && dx.abs() <= 1 {
                color = [65, 105, 48];
            }
        }
    }
    color
}

fn grass_side(x: i32, y: i32) -> [u8; 3] {
    let edge = 5 + (hash(x.div_euclid(3), 0, 7) % 4) as i32;
    if y < edge {
        if y == 0 {
            [145, 170, 79]
        } else if y == edge - 1 {
            [61, 100, 43]
        } else if x.rem_euclid(5) == 1 && y < edge - 2 {
            [114, 149, 65]
        } else {
            [92, 131, 54]
        }
    } else if y < edge + 7 && (x + y / 3).rem_euclid(11) == 2 {
        [138, 114, 68]
    } else {
        soil(x, y)
    }
}

fn soil(x: i32, y: i32) -> [u8; 3] {
    let row = y.div_euclid(8);
    let shifted_x = (x + (row & 1) * 4).rem_euclid(32);
    let clod = hash(shifted_x.div_euclid(8), row, 71);
    let local_x = shifted_x.rem_euclid(8);
    let local_y = y.rem_euclid(8);
    if local_y == 7 || (local_x == 7 && local_y > 2) {
        return [79, 56, 41];
    }
    if local_x == 2 && (2..=3).contains(&local_y) && clod.is_multiple_of(3) {
        return [167, 153, 120];
    }
    if local_y == 1 && (2..=5).contains(&local_x) {
        return [127, 92, 61];
    }
    shade([107, 76, 51], (clod % 13) as i16 - 6)
}

fn limestone(x: i32, y: i32) -> [u8; 3] {
    let stratum = (y + triangular(x + 7) / 8).rem_euclid(16);
    let joint = (x + triangular(y) / 7).rem_euclid(16);
    if stratum == 0 {
        [111, 122, 121]
    } else if stratum == 1 {
        [178, 180, 163]
    } else if joint == 0 && (3..=12).contains(&stratum) {
        [123, 130, 127]
    } else {
        let fossil_x = wrap_distance(x, 11);
        let fossil_y = wrap_distance(y, 22);
        let oval = fossil_x * fossil_x + fossil_y * fossil_y * 3;
        if (15..=27).contains(&oval) && fossil_x >= -2 {
            [191, 186, 159]
        } else {
            shade([149, 154, 146], i16::from((stratum / 4) as u8) * 2)
        }
    }
}

fn sand(x: i32, y: i32) -> [u8; 3] {
    let ripple = (y + triangular(x) / 4).rem_euclid(8);
    if ripple == 0 {
        [188, 169, 122]
    } else if ripple == 1 {
        [227, 210, 157]
    } else if hash(x, y, 97).is_multiple_of(89) {
        [245, 228, 178]
    } else {
        [211, 193, 143]
    }
}

fn water(x: i32, y: i32) -> [u8; 3] {
    let first = (y * 2 + triangular(x + 5)).rem_euclid(16);
    let second = (x + y * 2).rem_euclid(16);
    if first == 0 || (first == 1 && x.rem_euclid(7) < 3) {
        [80, 158, 172]
    } else if second == 0 {
        [41, 116, 138]
    } else {
        [51, 132, 152]
    }
}

fn bark(x: i32, y: i32) -> [u8; 3] {
    let groove = (x + triangular(y + 3) / 10).rem_euclid(8);
    let knot_x = wrap_distance(x, 19);
    let knot_y = wrap_distance(y, 17);
    let knot = knot_x * knot_x * 3 + knot_y * knot_y;
    if (13..=24).contains(&knot) {
        [66, 46, 31]
    } else if knot < 13 {
        [124, 86, 49]
    } else if groove == 0 || (groove == 1 && y.rem_euclid(11) > 3) {
        [70, 49, 32]
    } else if groove == 2 {
        [146, 105, 62]
    } else {
        [108, 76, 45]
    }
}

fn endgrain(x: i32, y: i32) -> [u8; 3] {
    let dx = wrap_distance(x, 15);
    let dy = wrap_distance(y, 16);
    let radius = ((dx * dx + dy * dy) as u32).isqrt() as i32;
    let ring = (radius + i32::from(dx > 2 && dy < -1)).rem_euclid(5);
    if dy.abs() <= 1 && dx > 6 {
        [104, 74, 43]
    } else if ring == 0 {
        [116, 82, 44]
    } else if ring == 1 {
        [190, 148, 89]
    } else {
        [161, 120, 71]
    }
}

fn leaves(x: i32, y: i32) -> [u8; 3] {
    let row = y.div_euclid(8);
    let local_x = (x + (row & 1) * 4).rem_euclid(8) - 3;
    let local_y = y.rem_euclid(8) - 3;
    if local_x.abs() + local_y.abs() > 4 {
        [34, 76, 43]
    } else if local_x == 0 && local_y <= 3 {
        [118, 153, 73]
    } else if local_y == -2 || local_x == -2 {
        [83, 132, 61]
    } else if local_x > 0 {
        [50, 99, 49]
    } else {
        [68, 117, 55]
    }
}

fn snow(x: i32, y: i32) -> [u8; 3] {
    let facet = (x + y * 2 + triangular(y) / 4).rem_euclid(16);
    if facet == 0 && x.rem_euclid(8) < 5 {
        [197, 216, 226]
    } else if facet <= 2 {
        [238, 244, 241]
    } else if x.rem_euclid(13) == 4 && y.rem_euclid(11) == 3 {
        [251, 253, 247]
    } else {
        [225, 234, 234]
    }
}

fn clay(x: i32, y: i32) -> [u8; 3] {
    let band = (y + triangular(x + 11) / 7).rem_euclid(16);
    let crack = (x - triangular(y) / 4).rem_euclid(16);
    if crack == 0 && band > 3 {
        [120, 69, 50]
    } else if band == 0 || band == 1 {
        [198, 128, 83]
    } else if band == 2 {
        [145, 77, 57]
    } else if band > 11 {
        [183, 106, 72]
    } else {
        [169, 94, 65]
    }
}

fn crystal(x: i32, y: i32) -> [u8; 3] {
    let seam = (x * 2 + y + triangular(y + 9) / 4).rem_euclid(16);
    let crossing = (x - y * 2).rem_euclid(16);
    if seam == 0 || (crossing == 0 && seam < 9) {
        [191, 244, 225]
    } else if seam <= 2 || (crossing <= 2 && seam < 12) {
        [91, 205, 200]
    } else if seam <= 5 {
        [42, 138, 149]
    } else if (x + y).rem_euclid(16) < 3 {
        [66, 102, 110]
    } else {
        [45, 72, 83]
    }
}

fn ore(x: i32, y: i32, speck: [u8; 3], salt: u32) -> [u8; 3] {
    let host = limestone(x, y);
    let col = x.div_euclid(8);
    let row = y.div_euclid(8);
    let anchor_x = (col * 8 + 2 + (hash(col, row, salt) % 4) as i32).rem_euclid(TILE_SIZE as i32);
    let anchor_y =
        (row * 8 + 2 + (hash(col, row, salt + 5) % 4) as i32).rem_euclid(TILE_SIZE as i32);
    let dx = wrap_distance(x, anchor_x);
    let dy = wrap_distance(y, anchor_y);
    let distance = dx * dx + dy * dy;
    if distance <= 2 {
        speck
    } else if distance <= 5 || (dx == 2 && dy == 0) || (dx == -1 && dy == 1) {
        shade(speck, -28)
    } else {
        host
    }
}

fn basalt(x: i32, y: i32) -> [u8; 3] {
    let row = y.div_euclid(16);
    let local_x = (x + row * 8).rem_euclid(16);
    let local_y = y.rem_euclid(16);
    let joint = local_x == 0
        || (local_y == 0 && local_x < 9)
        || (local_x + local_y == 23)
        || (local_x - local_y == 8);
    if joint {
        [37, 47, 55]
    } else if local_x == 1 || local_x + local_y == 22 {
        [90, 102, 107]
    } else if hash(x, y, 139).is_multiple_of(71) {
        [119, 125, 122]
    } else if local_x > 9 && local_y < 8 {
        [70, 80, 88]
    } else {
        [61, 73, 81]
    }
}

fn shade(color: [u8; 3], amount: i16) -> [u8; 3] {
    color.map(|channel| (i16::from(channel) + amount).clamp(0, 255) as u8)
}

fn wrap_distance(value: i32, center: i32) -> i32 {
    (value - center + 16).rem_euclid(32) - 16
}

fn triangular(value: i32) -> i32 {
    (value.rem_euclid(32) - 16).abs()
}

fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut value = (x as u32).wrapping_mul(0x9E37_79B9)
        ^ (y as u32).wrapping_mul(0x85EB_CA6B)
        ^ salt.wrapping_mul(0xC2B2_AE35);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^ (value >> 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn dimensions_alpha_and_bytes_are_deterministic() {
        let first = rgba_layers();
        assert_eq!(first.len(), (32 * 32 * LAYER_COUNT * 4) as usize);
        assert_eq!(first, rgba_layers());
        assert!(first.chunks_exact(4).all(|rgba| rgba[3] == 255));
    }

    #[test]
    fn every_block_face_maps_inside_array_and_directional_faces_are_distinct() {
        let normals = [
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ];
        let mut id = 0_u8;
        while let Some(block) = Block::from_id(id) {
            for normal in normals {
                assert!(layer_for(block, normal) < LAYER_COUNT);
            }
            id += 1;
        }
        assert!(id > 15);
        let ore_layers = [
            layer_for(Block::CoalOre, [0, 1, 0]),
            layer_for(Block::IronOre, [0, 1, 0]),
            layer_for(Block::GoldOre, [0, 1, 0]),
            layer_for(Block::DiamondOre, [0, 1, 0]),
        ];
        assert!(
            ore_layers
                .iter()
                .all(|layer| *layer != layer_for(Block::Stone, [0, 1, 0]))
        );
        for (index, layer) in ore_layers.iter().enumerate() {
            assert!(!ore_layers[..index].contains(layer));
        }
        assert_ne!(
            layer_for(Block::Grass, [0, 1, 0]),
            layer_for(Block::Grass, [1, 0, 0])
        );
        assert_eq!(
            layer_for(Block::Grass, [0, -1, 0]),
            layer_for(Block::Dirt, [0, 1, 0])
        );
        assert_ne!(
            layer_for(Block::Wood, [0, 1, 0]),
            layer_for(Block::Wood, [1, 0, 0])
        );
        assert_eq!(
            layer_for(Block::Wood, [0, 1, 0]),
            layer_for(Block::Wood, [0, -1, 0])
        );
    }

    #[test]
    fn every_material_has_authored_contrast_and_a_distinct_tile() {
        let pixels = rgba_layers();
        let layers: Vec<_> = pixels
            .chunks_exact((TILE_SIZE * TILE_SIZE * 4) as usize)
            .collect();
        for (index, layer) in layers.iter().enumerate() {
            let colors: HashSet<_> = layer.chunks_exact(4).collect();
            assert!(
                colors.len() > 12,
                "layer {index} needs visible structural variation"
            );
            let min = layer.chunks_exact(4).map(|p| p[0]).min().unwrap();
            let max = layer.chunks_exact(4).map(|p| p[0]).max().unwrap();
            assert!(max - min > 20, "layer {index} must exceed fine-grain noise");
            for other in &layers[..index] {
                assert_ne!(layer, other);
            }
        }
        assert!(pixel(GRASS_SIDE, 10, 0)[1] > pixel(GRASS_SIDE, 10, 31)[1]);
        assert!(pixel(CRYSTAL, 0, 0)[2] > pixel(CRYSTAL, 0, 0)[0]);
    }

    #[test]
    fn periodic_sampling_preserves_features_across_repeated_tiles() {
        for layer in 0..LAYER_COUNT {
            for y in 0..32 {
                for x in 0..32 {
                    assert_eq!(pixel(layer, x, y), pixel(layer, x + 32, y - 32));
                }
            }
        }
    }

    #[test]
    fn mip_dimensions_and_alpha_cover_all_layers_to_one_pixel() {
        let levels = mip_chain();
        assert_eq!(levels.len(), 6);
        assert_eq!(levels[0], rgba_layers());
        for (level, pixels) in levels.iter().enumerate() {
            let size = TILE_SIZE >> level;
            assert_eq!(pixels.len(), (size * size * LAYER_COUNT * 4) as usize);
            assert!(pixels.chunks_exact(4).all(|rgba| rgba[3] == 255));
        }
        assert_eq!(levels, mip_chain());
    }

    #[test]
    fn mip_filter_preserves_constant_layer_colors_and_averages_in_linear_light() {
        let mut pixels = vec![0; (LAYER_COUNT * 2 * 2 * 4) as usize];
        for layer in 0..LAYER_COUNT as usize {
            for pixel in 0..4 {
                let offset = (layer * 4 + pixel) * 4;
                pixels[offset..offset + 4].copy_from_slice(&[37 + layer as u8, 113, 209, 255]);
            }
        }
        let filtered = downsample(&pixels, 2);
        for (layer, pixel) in filtered.chunks_exact(4).enumerate() {
            assert_eq!(pixel, [37 + layer as u8, 113, 209, 255]);
        }
        for (pixel, bytes) in pixels[..16].chunks_exact_mut(4).enumerate() {
            let value = if pixel < 2 { 0 } else { 255 };
            bytes.copy_from_slice(&[value, value, value, 255]);
        }
        assert_eq!(&downsample(&pixels, 2)[..4], &[188, 188, 188, 255]);
    }
}
