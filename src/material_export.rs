//! Exports the exact runtime material bytes for artist review and asset handoff.
use crate::materials;
use std::{fs, io::BufWriter, path::Path};

const NAMES: [&str; 17] = [
    "grass-top",
    "grass-side",
    "soil",
    "limestone",
    "sand",
    "water",
    "wood-bark",
    "wood-endgrain",
    "leaves",
    "snow",
    "clay",
    "crystal",
    "basalt",
    "coal-ore",
    "iron-ore",
    "gold-ore",
    "diamond-ore",
];

pub fn export_materials(directory: &Path) -> Result<(), String> {
    // A fresh directory makes repeated exports explicit and protects artist edits.
    fs::create_dir(directory).map_err(|e| {
        format!(
            "Create a new material export directory {}: {e}",
            directory.display()
        )
    })?;
    let pixels = materials::rgba_layers();
    let side = materials::TILE_SIZE as usize;
    let stride = side * side * 4;
    if NAMES.len() != materials::LAYER_COUNT as usize {
        return Err("Material export names do not match the runtime layer count".into());
    }
    for (layer, name) in NAMES.iter().enumerate() {
        write_png(
            &directory.join(format!("{layer:02}-{name}.png")),
            side as u32,
            side as u32,
            &pixels[layer * stride..(layer + 1) * stride],
        )?;
    }
    // A nearest-neighbour contact sheet uses the same bytes, with no retouching.
    let scale = 6;
    let gap = 16;
    let columns = 4;
    let rows = NAMES.len().div_ceil(columns);
    let cell = side * scale + gap;
    let width = columns * cell + gap;
    let height = rows * cell + gap;
    let mut sheet = vec![0_u8; width * height * 4];
    for pixel in sheet.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[18, 28, 31, 255]);
    }
    for layer in 0..NAMES.len() {
        for y in 0..side * scale {
            for x in 0..side * scale {
                let source = layer * stride + ((y / scale) * side + x / scale) * 4;
                let target_x = gap + (layer % columns) * cell + x;
                let target_y = gap + (layer / columns) * cell + y;
                let target = (target_y * width + target_x) * 4;
                sheet[target..target + 4].copy_from_slice(&pixels[source..source + 4]);
            }
        }
    }
    write_png(
        &directory.join("contact-sheet.png"),
        width as u32,
        height as u32,
        &sheet,
    )?;
    let manifest = serde_json::json!({
        "tile_size": materials::TILE_SIZE,
        "color_space": "sRGB RGBA8",
        "provenance": "Original deterministic STRATA material algorithms in src/materials.rs; no external source assets",
        "contact_sheet": "four columns, row-major layer order; 6x nearest-neighbour preview",
        "layers": NAMES,
        "usage": "Exports exact runtime bytes; editing these files does not yet hot-reload the engine."
    });
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("Material manifest: {e}"))?;
    Ok(())
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Create {}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut writer = encoder
        .write_header()
        .map_err(|e| format!("PNG header: {e}"))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| format!("PNG pixels: {e}"))?;
    writer.finish().map_err(|e| format!("Finish PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::BufReader,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn png_exports_decode_to_exact_runtime_tiles_and_protect_existing_directories() {
        let path = std::env::temp_dir().join(format!(
            "strata-material-export-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        export_materials(&path).unwrap();
        let runtime = materials::rgba_layers();
        let stride = (materials::TILE_SIZE * materials::TILE_SIZE * 4) as usize;
        for (layer, name) in NAMES.iter().enumerate() {
            let file = fs::File::open(path.join(format!("{layer:02}-{name}.png"))).unwrap();
            let mut decoder = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
            let mut decoded = vec![0; decoder.output_buffer_size().unwrap()];
            let info = decoder.next_frame(&mut decoded).unwrap();
            assert_eq!((info.width, info.height), (32, 32));
            assert_eq!(info.color_type, png::ColorType::Rgba);
            assert_eq!(
                &decoded[..info.buffer_size()],
                &runtime[layer * stride..(layer + 1) * stride]
            );
        }
        assert!(export_materials(&path).is_err());
        assert_eq!(fs::read_dir(&path).unwrap().count(), NAMES.len() + 2);
        for entry in fs::read_dir(&path).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(path).unwrap();
    }
}
