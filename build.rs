#[allow(dead_code)]
#[path = "src/texture_pack.rs"]
mod texture_pack;

use std::{env, fs, path::Path};

fn main() {
    let manifest_dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).to_path_buf();
    let assets = manifest_dir.join("assets");
    let manifest = assets.join("block-textures.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let pack = texture_pack::load_pack(&manifest, &assets).unwrap_or_else(|error| {
        panic!("block texture pack: {error}");
    });
    for layer in &pack.layers {
        let path = texture_pack::texture_path(&assets, &layer.namespace, &layer.file);
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("block_textures_gen.rs");
    fs::write(&out, emit(&pack)).unwrap();
}

fn emit(pack: &texture_pack::TexturePack) -> String {
    let mut rust = String::new();
    rust.push_str(&format!(
        "pub const TILE_SIZE: u32 = {};\npub const LAYER_COUNT: u32 = {};\n",
        pack.tile_size,
        pack.layers.len()
    ));
    rust.push_str("pub struct GenLayer {\n    pub namespace: &'static str,\n    pub file: &'static str,\n    pub png: &'static [u8],\n}\n");
    rust.push_str("pub struct GenFace {\n    pub id: &'static str,\n    pub face: u8,\n    pub base_layer: u32,\n    pub variant_count: u8,\n    pub random_top: bool,\n}\n");
    rust.push_str("pub static LAYERS: &[GenLayer] = &[\n");
    for layer in &pack.layers {
        let rel = format!("assets/{}/textures/block/{}", layer.namespace, layer.file);
        rust.push_str(&format!(
            "    GenLayer {{ namespace: {:?}, file: {:?}, png: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{rel}\")) }},\n",
            layer.namespace, layer.file
        ));
    }
    rust.push_str("];\n");
    rust.push_str("pub static FACES: &[GenFace] = &[\n");
    for face in &pack.bindings {
        rust.push_str(&format!(
            "    GenFace {{ id: \"{}\", face: {}, base_layer: {}, variant_count: {}, random_top: {} }},\n",
            face.id,
            face.face,
            face.base_layer,
            face.variant_count,
            face.random_top
        ));
    }
    rust.push_str("];\n");
    rust
}
