//! Namespaced block definitions loaded from TOML, the same way a data mod would.
//!
//! Builtins live at `assets/<namespace>/blocks/<name>.toml`. The id is the path,
//! not a field in the file. `Registry::load_from` runs once, assigns dense [`BlockId`]
//! values in file order, and freezes. Worldgen resolves names at `World::new`.
//! The hot path indexes [`BlockFlags`] by id. This module owns the only TOML parser.
//!
//! The embedded copies are the normal path. A data folder is used only when
//! `STRATA_DATA_DIR` or `--data-dir` names one. The working directory is not searched.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use toml::{Table, Value};

const MAX_FILE_BYTES: usize = 64 * 1024;
/// `BlockId` is a `u16`. Palette length is also capped at 65,535, so the
/// 65,536th definition is refused.
const MAX_BLOCKS: usize = 65_535;
/// wgpu `depth_or_array_layers` rejects a texture array taller than this.
pub const MAX_TEXTURE_LAYERS: usize = 256;
const BLOCKS_RELATIVE: &str = "assets/strata/blocks";
const IDENTITY_TINT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const MISSING_TEXTURE: &str = "strata:missing";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = [0_u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

/// Fixed builtin load order. Runtime ids, and schema-1 numbers for ids 0..=15,
/// are this order. Do not sort or insert.
pub const BUILTIN_FILES: &[&str] = &[
    "assets/strata/blocks/air.toml",
    "assets/strata/blocks/grass.toml",
    "assets/strata/blocks/dirt.toml",
    "assets/strata/blocks/stone.toml",
    "assets/strata/blocks/sand.toml",
    "assets/strata/blocks/water.toml",
    "assets/strata/blocks/wood.toml",
    "assets/strata/blocks/leaves.toml",
    "assets/strata/blocks/snow.toml",
    "assets/strata/blocks/clay.toml",
    "assets/strata/blocks/crystal.toml",
    "assets/strata/blocks/basalt.toml",
    "assets/strata/blocks/coal_ore.toml",
    "assets/strata/blocks/iron_ore.toml",
    "assets/strata/blocks/gold_ore.toml",
    "assets/strata/blocks/diamond_ore.toml",
    "assets/strata/blocks/missing.toml",
];

pub const AIR: BlockId = BlockId(0);
pub const GRASS: BlockId = BlockId(1);
pub const DIRT: BlockId = BlockId(2);
pub const STONE: BlockId = BlockId(3);
pub const SAND: BlockId = BlockId(4);
pub const WATER: BlockId = BlockId(5);
pub const WOOD: BlockId = BlockId(6);
pub const LEAVES: BlockId = BlockId(7);
pub const SNOW: BlockId = BlockId(8);
pub const CLAY: BlockId = BlockId(9);
pub const CRYSTAL: BlockId = BlockId(10);
pub const BASALT: BlockId = BlockId(11);
pub const COAL_ORE: BlockId = BlockId(12);
pub const IRON_ORE: BlockId = BlockId(13);
pub const GOLD_ORE: BlockId = BlockId(14);
pub const DIAMOND_ORE: BlockId = BlockId(15);
pub const MISSING: BlockId = BlockId(16);

static REGISTRY: OnceLock<Result<Arc<Registry>, String>> = OnceLock::new();

/// Dense runtime id. The voxel stores this, never the definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct BlockId(u16);

impl BlockId {
    pub const fn from_index(index: u16) -> Self {
        Self(index)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    /// Schema-1 block number. Only the original 16 ids survive that document.
    pub const fn legacy_u8(self) -> Option<u8> {
        if self.raw() <= 15 {
            Some(self.raw() as u8)
        } else {
            None
        }
    }

    /// Schema-1 decoder. `16` (`strata:missing`) and anything above it is rejected.
    pub const fn from_id(id: u8) -> Option<Self> {
        if id <= 15 {
            Some(Self(id as u16))
        } else {
            None
        }
    }

    /// Builtin definitions in id order, including `strata:missing`.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..registry().builtin_count()).map(|index| Self::from_index(index as u16))
    }

    pub fn is_solid(self) -> bool {
        registry().flags(self).solid
    }

    pub fn is_placeable(self) -> bool {
        registry().flags(self).placeable
    }

    // The mesh table is what the game reads. This query stays for tests and callers.
    #[allow(dead_code)]
    pub fn is_replaceable(self) -> bool {
        registry().flags(self).replaceable
    }

    /// Linear shader multiply. Builtins are identity white. This is not the hotbar color.
    /// The mesh table copies the value. This query stays for tests and callers.
    #[allow(dead_code)]
    pub fn tint(self) -> [f32; 4] {
        registry().flags(self).tint
    }

    /// Linear RGBA used by the hotbar and the material picker.
    pub fn swatch(self) -> [f32; 4] {
        registry().flags(self).swatch
    }

    /// Hotbar swatch.
    pub fn color(self) -> [f32; 4] {
        self.swatch()
    }

    /// English name stored for a builtin.
    ///
    /// A world-local placeholder keeps its original id string on the world.
    /// This returns the builtin `strata:missing` label for any id above the
    /// frozen table, which is the stand-in until the caller asks the world.
    pub fn display(self) -> &'static str {
        registry().display(self)
    }

    pub fn name(self) -> &'static str {
        self.display()
    }

    pub fn render_class(self) -> RenderClass {
        registry().render_class(self)
    }
}

/// How the mesher and, later, the shader classify a block.
///
/// `Cutout` is stored for leaves now. Until the cutout pipeline exists, the
/// mesher culls it like `Opaque`. `Fluid` is the water special case.
/// The string `translucent` is rejected by the parser and is not a variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RenderClass {
    Opaque = 0,
    Cutout = 1,
    Fluid = 2,
    Invisible = 3,
}

/// One row of the dense flag table. Indexed by [`BlockId::index`].
#[derive(Clone, Copy, Debug)]
pub struct BlockFlags {
    pub solid: bool,
    pub placeable: bool,
    pub replaceable: bool,
    pub render: RenderClass,
    /// Shader multiply. Identity white leaves the authored tile unchanged.
    pub tint: [f32; 4],
    /// Hotbar and picker color. Not uploaded as the shader multiply.
    pub swatch: [f32; 4],
    /// First texture variant for top, bottom, north, south, east, west.
    /// Layer numbers are assigned when the registry freezes, in sorted texture-id order.
    pub layers: [u32; 6],
}

/// Stored face. `side` is a parse-time fallback for the four cardinals and is not stored.
///
/// Outward normals follow the player: yaw 0 looks down −Z, so −Z is north,
/// +Z is south, +X is east, and −X is west.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFace {
    Top = 0,
    Bottom = 1,
    North = 2,
    South = 3,
    East = 4,
    West = 5,
}

impl BlockFace {
    pub const ALL: [Self; 6] = [
        Self::Top,
        Self::Bottom,
        Self::North,
        Self::South,
        Self::East,
        Self::West,
    ];

    pub fn from_normal(normal: [i32; 3]) -> Self {
        if normal[1] > 0 {
            Self::Top
        } else if normal[1] < 0 {
            Self::Bottom
        } else if normal[2] < 0 {
            Self::North
        } else if normal[2] > 0 {
            Self::South
        } else if normal[0] > 0 {
            Self::East
        } else {
            Self::West
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
        }
    }

    fn accepts_side(self) -> bool {
        matches!(self, Self::North | Self::South | Self::East | Self::West)
    }
}

/// Carried for the texture owner. The mesher does not rotate faces from this.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureRotation {
    None,
    Random,
}

#[derive(Clone, Debug)]
struct FaceLists {
    faces: [Vec<u32>; 6],
}

impl FaceLists {
    fn get(&self, face: BlockFace) -> &[u32] {
        &self.faces[face.index()]
    }

    fn firsts(&self) -> [u32; 6] {
        let mut layers = [0; 6];
        for (index, face) in self.faces.iter().enumerate() {
            layers[index] = face[0];
        }
        layers
    }

    fn filled(layer: u32) -> Self {
        Self {
            faces: [
                vec![layer],
                vec![layer],
                vec![layer],
                vec![layer],
                vec![layer],
                vec![layer],
            ],
        }
    }
}

/// Frozen builtin definitions. Placeholders are not inserted here: every
/// unknown name shares one flag template, and the world remembers the string.
#[derive(Debug)]
pub struct Registry {
    rows: Vec<BlockFlags>,
    names: Vec<String>,
    // Read by `name_key`. The running game uses display names; tests check the key.
    #[allow(dead_code)]
    name_keys: Vec<String>,
    displays: Vec<String>,
    by_name: HashMap<String, BlockId>,
    textures: Vec<String>,
    texture_index: HashMap<String, u32>,
    variants: Vec<FaceLists>,
    // Read by `texture_rotation`. The mesher does not rotate yet.
    #[allow(dead_code)]
    rotations: Vec<[TextureRotation; 6]>,
    placeholder: BlockFlags,
    placeholder_variants: FaceLists,
    content_hash: String,
    from_disk: bool,
    warnings: Vec<String>,
}

/// Ids worldgen writes. Resolved once from names inside `World::new`.
#[derive(Clone, Copy, Debug)]
pub struct WorldgenBlocks {
    pub air: BlockId,
    pub grass: BlockId,
    pub dirt: BlockId,
    pub stone: BlockId,
    pub sand: BlockId,
    pub water: BlockId,
    pub wood: BlockId,
    pub leaves: BlockId,
    pub snow: BlockId,
    pub clay: BlockId,
    pub crystal: BlockId,
    pub basalt: BlockId,
    pub coal_ore: BlockId,
    pub iron_ore: BlockId,
    pub gold_ore: BlockId,
    pub diamond_ore: BlockId,
}

impl WorldgenBlocks {
    pub fn resolve(registry: &Registry) -> Self {
        Self {
            air: required(registry, "strata:air"),
            grass: required(registry, "strata:grass"),
            dirt: required(registry, "strata:dirt"),
            stone: required(registry, "strata:stone"),
            sand: required(registry, "strata:sand"),
            water: required(registry, "strata:water"),
            wood: required(registry, "strata:wood"),
            leaves: required(registry, "strata:leaves"),
            snow: required(registry, "strata:snow"),
            clay: required(registry, "strata:clay"),
            crystal: required(registry, "strata:crystal"),
            basalt: required(registry, "strata:basalt"),
            coal_ore: required(registry, "strata:coal_ore"),
            iron_ore: required(registry, "strata:iron_ore"),
            gold_ore: required(registry, "strata:gold_ore"),
            diamond_ore: required(registry, "strata:diamond_ore"),
        }
    }
}

fn required(registry: &Registry, name: &str) -> BlockId {
    registry
        .get(name)
        .unwrap_or_else(|| panic!("builtin block {name} is missing from the registry"))
}

/// `STRATA_DATA_DIR` when it is set and not empty. An empty value is ignored.
pub fn env_data_dir() -> Option<PathBuf> {
    data_dir_override(std::env::var_os("STRATA_DATA_DIR").as_deref())
}

/// Empty and missing values select the embedded registry. A non-empty value is
/// the data root, used as given.
fn data_dir_override(value: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

/// Load the builtin registry if this process has not already done so.
///
/// `data_dir` is the data root, used as given. Blocks are read from
/// `{data_dir}/assets/strata/blocks`. `None` uses the embedded copies and does
/// not look at the working directory. The first call in the process wins.
pub fn install_from(data_dir: Option<&Path>) -> Result<Arc<Registry>, String> {
    let owned = data_dir.map(Path::to_path_buf);
    match REGISTRY.get_or_init(|| Registry::load_from(owned.as_deref()).map(Arc::new)) {
        Ok(registry) => Ok(Arc::clone(registry)),
        Err(error) => Err(error.clone()),
    }
}

/// Load using [`env_data_dir`], or the embedded copies when that variable is unset.
pub fn install() -> Result<Arc<Registry>, String> {
    install_from(env_data_dir().as_deref())
}

pub fn registry() -> &'static Registry {
    match REGISTRY.get_or_init(|| Registry::load_from(env_data_dir().as_deref()).map(Arc::new)) {
        Ok(registry) => registry.as_ref(),
        Err(error) => panic!("block registry failed to load: {error}"),
    }
}

pub fn registry_handle() -> Arc<Registry> {
    match REGISTRY.get_or_init(|| Registry::load_from(env_data_dir().as_deref()).map(Arc::new)) {
        Ok(registry) => Arc::clone(registry),
        Err(error) => panic!("block registry failed to load: {error}"),
    }
}

/// Render class for `id`, read from the dense flag table.
pub fn render_class(id: BlockId) -> RenderClass {
    registry().render_class(id)
}

impl Registry {
    /// `None` embeds the builtin TOML. `Some` reads that directory as given and
    /// fails if `{data_dir}/assets/strata/blocks` is not a directory.
    pub fn load_from(data_dir: Option<&Path>) -> Result<Self, String> {
        let (sources, from_disk) = match data_dir {
            Some(dir) => {
                let blocks = dir.join(BLOCKS_RELATIVE);
                if !blocks.is_dir() {
                    return Err(format!("{}: block directory is missing", blocks.display()));
                }
                (read_block_directory(&blocks)?, true)
            }
            None => (embedded_sources_owned(), false),
        };
        let mut registry = assemble(sources)?;
        registry.from_disk = from_disk;
        Ok(registry)
    }

    pub fn get(&self, id: &str) -> Option<BlockId> {
        self.by_name.get(id).copied()
    }

    pub fn builtin_count(&self) -> usize {
        self.rows.len()
    }

    pub fn name(&self, id: BlockId) -> &str {
        self.names
            .get(id.index())
            .map(String::as_str)
            .unwrap_or("strata:missing")
    }

    // Tests and the localization contract. Gameplay reads `display`.
    #[allow(dead_code)]
    pub fn name_key(&self, id: BlockId) -> &str {
        self.name_keys
            .get(id.index())
            .map(String::as_str)
            .unwrap_or("block.strata.missing")
    }

    pub fn display(&self, id: BlockId) -> &str {
        self.displays
            .get(id.index())
            .map(String::as_str)
            .unwrap_or("Missing block")
    }

    pub fn flags(&self, id: BlockId) -> BlockFlags {
        self.rows
            .get(id.index())
            .copied()
            .unwrap_or(self.placeholder)
    }

    /// Builtin rows only. Placeholder ids are past the end and use one template.
    pub fn flag_rows(&self) -> &[BlockFlags] {
        &self.rows
    }

    pub fn render_class(&self, id: BlockId) -> RenderClass {
        self.flags(id).render
    }

    /// Texture ids in the order their array layers were assigned.
    pub fn textures_sorted(&self) -> &[String] {
        &self.textures
    }

    pub fn texture_count(&self) -> u32 {
        self.textures.len() as u32
    }

    pub fn texture_id(&self, layer: u32) -> &str {
        self.textures
            .get(layer as usize)
            .map(String::as_str)
            .unwrap_or("strata:missing")
    }

    pub fn texture_index(&self, texture_id: &str) -> Option<u32> {
        self.texture_index.get(texture_id).copied()
    }

    /// Every variant of one face, in file order. The mesher uses the first.
    pub fn texture_variants(&self, id: BlockId, face: BlockFace) -> &[u32] {
        self.variants
            .get(id.index())
            .unwrap_or(&self.placeholder_variants)
            .get(face)
    }

    // Tests and the texture owner. The mesher uses variant 0 and does not rotate.
    #[allow(dead_code)]
    pub fn texture_rotation(&self, id: BlockId, face: BlockFace) -> TextureRotation {
        self.rotations
            .get(id.index())
            .map(|faces| faces[face.index()])
            .unwrap_or(TextureRotation::None)
    }

    /// Array layer of the single missing-texture checker.
    pub fn missing_layer(&self) -> u32 {
        self.texture_index[MISSING_TEXTURE]
    }

    pub fn load_warnings(&self) -> &[String] {
        &self.warnings
    }

    /// SHA-256, lowercase hex, of the builtin TOML bytes in registry order.
    /// Each file contributes its exact UTF-8 and is padded with one `\n` when
    /// it does not already end in one, so every contribution ends with a single
    /// newline. A mismatch with a saved hash does not reject the world.
    pub fn builtin_content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn loaded_from_disk(&self) -> bool {
        self.from_disk
    }
}

fn embedded_sources() -> [(&'static str, &'static str); 17] {
    [
        (
            "assets/strata/blocks/air.toml",
            include_str!("../assets/strata/blocks/air.toml"),
        ),
        (
            "assets/strata/blocks/grass.toml",
            include_str!("../assets/strata/blocks/grass.toml"),
        ),
        (
            "assets/strata/blocks/dirt.toml",
            include_str!("../assets/strata/blocks/dirt.toml"),
        ),
        (
            "assets/strata/blocks/stone.toml",
            include_str!("../assets/strata/blocks/stone.toml"),
        ),
        (
            "assets/strata/blocks/sand.toml",
            include_str!("../assets/strata/blocks/sand.toml"),
        ),
        (
            "assets/strata/blocks/water.toml",
            include_str!("../assets/strata/blocks/water.toml"),
        ),
        (
            "assets/strata/blocks/wood.toml",
            include_str!("../assets/strata/blocks/wood.toml"),
        ),
        (
            "assets/strata/blocks/leaves.toml",
            include_str!("../assets/strata/blocks/leaves.toml"),
        ),
        (
            "assets/strata/blocks/snow.toml",
            include_str!("../assets/strata/blocks/snow.toml"),
        ),
        (
            "assets/strata/blocks/clay.toml",
            include_str!("../assets/strata/blocks/clay.toml"),
        ),
        (
            "assets/strata/blocks/crystal.toml",
            include_str!("../assets/strata/blocks/crystal.toml"),
        ),
        (
            "assets/strata/blocks/basalt.toml",
            include_str!("../assets/strata/blocks/basalt.toml"),
        ),
        (
            "assets/strata/blocks/coal_ore.toml",
            include_str!("../assets/strata/blocks/coal_ore.toml"),
        ),
        (
            "assets/strata/blocks/iron_ore.toml",
            include_str!("../assets/strata/blocks/iron_ore.toml"),
        ),
        (
            "assets/strata/blocks/gold_ore.toml",
            include_str!("../assets/strata/blocks/gold_ore.toml"),
        ),
        (
            "assets/strata/blocks/diamond_ore.toml",
            include_str!("../assets/strata/blocks/diamond_ore.toml"),
        ),
        (
            "assets/strata/blocks/missing.toml",
            include_str!("../assets/strata/blocks/missing.toml"),
        ),
    ]
}

struct SourceFile {
    /// `assets/<namespace>/blocks/<name>.toml`, used only to form the block id.
    logical: String,
    /// Path named in errors. Equal to `logical` for embedded files.
    display: String,
    bytes: Vec<u8>,
}

fn embedded_sources_owned() -> Vec<SourceFile> {
    embedded_sources()
        .into_iter()
        .map(|(path, text)| SourceFile {
            logical: path.to_string(),
            display: path.to_string(),
            bytes: text.as_bytes().to_vec(),
        })
        .collect()
}

fn read_block_directory(dir: &Path) -> Result<Vec<SourceFile>, String> {
    let expected: Vec<&str> = BUILTIN_FILES
        .iter()
        .map(|path| {
            path.rsplit('/')
                .next()
                .expect("builtin path has a file name")
        })
        .collect();
    let mut seen = Vec::new();
    let display_dir = dir.display().to_string();
    let entries = fs::read_dir(dir).map_err(|error| format!("{display_dir}: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{display_dir}: {error}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".toml") {
            continue;
        }
        if !expected.contains(&name.as_ref()) {
            return Err(format!("{display_dir}/{name}: unexpected block file"));
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("{display_dir}/{name}: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("{display_dir}/{name}: not a file"));
        }
        seen.push(name.into_owned());
    }
    for name in &expected {
        if !seen.iter().any(|found| found == name) {
            return Err(format!("{display_dir}/{name}: missing builtin block file"));
        }
    }
    let mut sources = Vec::with_capacity(BUILTIN_FILES.len());
    for path in BUILTIN_FILES {
        let file_name = path
            .rsplit('/')
            .next()
            .expect("builtin path has a file name");
        let full = dir.join(file_name);
        let bytes = read_limited_file(&full)?;
        sources.push(SourceFile {
            logical: (*path).to_string(),
            display: full.display().to_string(),
            bytes,
        });
    }
    Ok(sources)
}

#[derive(Debug)]
struct Parsed {
    path: String,
    id: String,
    name_key: String,
    display: String,
    solid: bool,
    placeable: bool,
    replaceable: bool,
    render: RenderClass,
    tint: [f32; 4],
    swatch: [f32; 4],
    faces: Option<FaceNames>,
    rotation: [TextureRotation; 6],
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct FaceNames {
    names: [Vec<String>; 6],
    /// TOML key that supplied each face, after `all` / `side` / specific precedence.
    keys: [&'static str; 6],
}

fn assemble(sources: Vec<SourceFile>) -> Result<Registry, String> {
    if sources.len() > MAX_BLOCKS {
        return Err(format!(
            "block registry refuses the 65536th block ({MAX_BLOCKS} is the maximum)"
        ));
    }
    let mut parsed = Vec::with_capacity(sources.len());
    for source in sources {
        parsed.push(parse_block_file(
            &source.logical,
            &source.display,
            &source.bytes,
        )?);
    }
    let mut seen = HashMap::with_capacity(parsed.len());
    for block in &parsed {
        if seen.insert(block.id.clone(), block.path.clone()).is_some() {
            return Err(format!(
                "{}: id: duplicate block id {}",
                block.path, block.id
            ));
        }
    }
    validate_builtin_set(&parsed)?;
    freeze(parsed)
}

fn validate_builtin_set(blocks: &[Parsed]) -> Result<(), String> {
    if blocks.len() != BUILTIN_FILES.len() {
        return Err(format!(
            "builtin block count is {}, expected {}",
            blocks.len(),
            BUILTIN_FILES.len()
        ));
    }
    for (index, block) in blocks.iter().enumerate() {
        let expected = block_id_from_path(BUILTIN_FILES[index])?;
        if block.id != expected {
            return Err(format!(
                "{}: id: expected {expected} at registry index {index}",
                block.path
            ));
        }
    }
    let air = &blocks[0];
    if air.solid || air.placeable || !air.replaceable || air.render != RenderClass::Invisible {
        return Err(format!(
            "{}: solid: strata:air must be non-solid, non-placeable, replaceable, and invisible",
            air.path
        ));
    }
    if air.name_key != "block.strata.air" {
        return Err(format!(
            "{}: name_key: strata:air must use block.strata.air",
            air.path
        ));
    }
    let grass = find_block(blocks, "strata:grass")?;
    if !single_texture(grass, BlockFace::Top, "strata:block/grass_top") {
        return Err(format!(
            "{}: textures.top: strata:grass top must be strata:block/grass_top",
            grass.path
        ));
    }
    let water = find_block(blocks, "strata:water")?;
    if water.solid || water.render != RenderClass::Fluid {
        return Err(format!(
            "{}: render: strata:water must be non-solid and fluid",
            water.path
        ));
    }
    if water.tint != IDENTITY_TINT {
        return Err(format!(
            "{}: tint: strata:water tint must be 1.0, 1.0, 1.0, 1.0",
            water.path
        ));
    }
    let leaves = find_block(blocks, "strata:leaves")?;
    if !leaves.solid || leaves.render != RenderClass::Cutout {
        return Err(format!(
            "{}: render: strata:leaves must be solid and cutout",
            leaves.path
        ));
    }
    let stone = find_block(blocks, "strata:stone")?;
    if stone.rotation[BlockFace::North.index()] != TextureRotation::None {
        return Err(format!(
            "{}: texture_rotation: strata:stone north face must be \"none\"",
            stone.path
        ));
    }
    let missing = find_block(blocks, "strata:missing")?;
    if !missing.solid || missing.placeable || missing.render != RenderClass::Opaque {
        return Err(format!(
            "{}: placeable: strata:missing must be solid, opaque, and not placeable",
            missing.path
        ));
    }
    Ok(())
}

fn single_texture(block: &Parsed, face: BlockFace, expected: &str) -> bool {
    let Some(faces) = &block.faces else {
        return false;
    };
    let names = &faces.names[face.index()];
    names.len() == 1 && names[0] == expected
}

fn find_block<'a>(blocks: &'a [Parsed], id: &str) -> Result<&'a Parsed, String> {
    blocks
        .iter()
        .find(|block| block.id == id)
        .ok_or_else(|| format!("{id}: missing builtin block"))
}

fn freeze(blocks: Vec<Parsed>) -> Result<Registry, String> {
    let mut warnings = Vec::new();
    let mut warned = BTreeSet::new();
    let mut texture_names = BTreeSet::new();
    texture_names.insert(MISSING_TEXTURE.to_string());
    for block in &blocks {
        let Some(faces) = &block.faces else {
            continue;
        };
        for (face_index, list) in faces.names.iter().enumerate() {
            let key = faces.keys[face_index];
            for id in list {
                if crate::materials::authored_texture(id) {
                    texture_names.insert(id.clone());
                } else {
                    let token = format!("{}|{key}|{id}", block.path);
                    if warned.insert(token) {
                        let message = format!(
                            "{}: textures.{key}: `{id}` has no authored tile; using {MISSING_TEXTURE}",
                            block.path
                        );
                        eprintln!("STRATA_REGISTRY_WARN {message}");
                        warnings.push(message);
                    }
                }
            }
        }
    }
    let (textures, texture_index) = index_texture_layers(texture_names)?;
    let missing_layer = texture_index[MISSING_TEXTURE];
    let mut rows = Vec::with_capacity(blocks.len());
    let mut names = Vec::with_capacity(blocks.len());
    let mut name_keys = Vec::with_capacity(blocks.len());
    let mut displays = Vec::with_capacity(blocks.len());
    let mut by_name = HashMap::with_capacity(blocks.len());
    let mut variants = Vec::with_capacity(blocks.len());
    let mut rotations = Vec::with_capacity(blocks.len());
    let mut preimage = Vec::new();
    for (index, block) in blocks.into_iter().enumerate() {
        let lists = match &block.faces {
            Some(faces) => FaceLists {
                faces: std::array::from_fn(|face| {
                    map_layers(&texture_index, &faces.names[face], missing_layer)
                }),
            },
            None => FaceLists::filled(missing_layer),
        };
        let id = BlockId(index as u16);
        rows.push(BlockFlags {
            solid: block.solid,
            placeable: block.placeable,
            replaceable: block.replaceable,
            render: block.render,
            tint: block.tint,
            swatch: block.swatch,
            layers: lists.firsts(),
        });
        by_name.insert(block.id.clone(), id);
        names.push(block.id);
        name_keys.push(block.name_key);
        displays.push(block.display);
        rotations.push(block.rotation);
        variants.push(lists);
        preimage.extend_from_slice(&block.bytes);
        if !block.bytes.ends_with(b"\n") {
            preimage.push(b'\n');
        }
    }
    let missing_swatch = rows
        .get(MISSING.index())
        .map(|row| row.swatch)
        .unwrap_or([0.85, 0.15, 0.45, 1.0]);
    Ok(Registry {
        rows,
        names,
        name_keys,
        displays,
        by_name,
        textures,
        texture_index,
        variants,
        rotations,
        placeholder: BlockFlags {
            solid: true,
            placeable: false,
            replaceable: false,
            render: RenderClass::Opaque,
            tint: IDENTITY_TINT,
            swatch: missing_swatch,
            layers: [missing_layer; 6],
        },
        placeholder_variants: FaceLists::filled(missing_layer),
        content_hash: hex_encode(&sha256(&preimage)),
        from_disk: false,
        warnings,
    })
}

fn index_texture_layers(
    names: BTreeSet<String>,
) -> Result<(Vec<String>, HashMap<String, u32>), String> {
    if names.len() > MAX_TEXTURE_LAYERS {
        return Err(format!(
            "{} texture layers exceed {MAX_TEXTURE_LAYERS}",
            names.len()
        ));
    }
    let textures: Vec<String> = names.into_iter().collect();
    let texture_index = textures
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index as u32))
        .collect();
    Ok((textures, texture_index))
}

fn map_layers(index: &HashMap<String, u32>, names: &[String], missing_layer: u32) -> Vec<u32> {
    names
        .iter()
        .map(|name| index.get(name).copied().unwrap_or(missing_layer))
        .collect()
}

/// Rejects a block file from its metadata length, then reads at most 64 KiB + 1.
fn read_limited_file(path: &Path) -> Result<Vec<u8>, String> {
    let display = path.display().to_string();
    let file = fs::File::open(path).map_err(|error| format!("{display}: {error}"))?;
    let length = file
        .metadata()
        .map_err(|error| format!("{display}: {error}"))?
        .len();
    if length > MAX_FILE_BYTES as u64 {
        return Err(format!("{display}: file exceeds 64 KiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{display}: {error}"))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("{display}: file exceeds 64 KiB"));
    }
    Ok(bytes)
}

fn normalize_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            normalized.push(b'\n');
            index += 2;
        } else {
            normalized.push(bytes[index]);
            index += 1;
        }
    }
    normalized
}

fn parse_block_file(logical: &str, display: &str, bytes: &[u8]) -> Result<Parsed, String> {
    let id = block_id_from_path(logical)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("{display}: file exceeds 64 KiB"));
    }
    let bytes = normalize_newlines(bytes);
    if bytes.contains(&b'\r') {
        return Err(format!(
            "{display}: carriage return is not allowed unless it is part of CRLF"
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| format!("{display}: file is not UTF-8"))?;
    let table: Table = text
        .parse()
        .map_err(|error| format!("{display}: {error}"))?;
    reject_unknown(
        display,
        &table,
        &[
            "name_key",
            "display",
            "solid",
            "placeable",
            "replaceable",
            "render",
            "tint",
            "swatch",
            "textures",
            "texture_rotation",
        ],
        "",
    )?;
    let name_key = require_string(display, &table, "name_key")?;
    if !is_name_key(&name_key) {
        return Err(format!(
            "{display}: name_key: must match [a-z0-9_.]{{1,64}}"
        ));
    }
    let label = require_string(display, &table, "display")?;
    if label.is_empty() {
        return Err(format!("{display}: display: must not be empty"));
    }
    if label.len() > 64 {
        return Err(format!("{display}: display: longer than 64 bytes"));
    }
    let solid = require_bool(display, &table, "solid")?;
    let placeable = require_bool(display, &table, "placeable")?;
    let replaceable = require_bool(display, &table, "replaceable")?;
    let render = parse_render(display, &require_string(display, &table, "render")?)?;
    let tint = optional_color(display, &table, "tint")?.unwrap_or(IDENTITY_TINT);
    if render == RenderClass::Fluid && tint[3] != 1.0 {
        return Err(format!("{display}: tint: fluid tint alpha must be 1.0"));
    }
    let swatch = require_color(display, &table, "swatch")?;
    let faces = parse_textures(display, table.get("textures"))?;
    let rotation_specified = table.contains_key("texture_rotation");
    let rotation = parse_rotation(display, table.get("texture_rotation"))?;
    if render == RenderClass::Invisible {
        if faces.is_some() {
            return Err(format!(
                "{display}: textures: invisible blocks cannot name a texture"
            ));
        }
        if rotation_specified {
            return Err(format!(
                "{display}: texture_rotation: invisible blocks cannot rotate a texture"
            ));
        }
    } else if faces.is_none() {
        return Err(format!(
            "{display}: textures: set `all`, or cover every face with `top`, `bottom`, `side`, or a cardinal face"
        ));
    }
    Ok(Parsed {
        path: display.to_string(),
        id,
        name_key,
        display: label,
        solid,
        placeable,
        replaceable,
        render,
        tint,
        swatch,
        faces,
        rotation,
        bytes,
    })
}

fn parse_textures(path: &str, value: Option<&Value>) -> Result<Option<FaceNames>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(table) = value.as_table() else {
        return Err(format!("{path}: textures: expected a table"));
    };
    reject_unknown(
        path,
        table,
        &[
            "all", "top", "bottom", "side", "north", "south", "east", "west",
        ],
        "textures",
    )?;
    let all = optional_texture_list(path, table, "all")?;
    let side = optional_texture_list(path, table, "side")?;
    let mut names: [Vec<String>; 6] = Default::default();
    let mut keys = [""; 6];
    for face in BlockFace::ALL {
        let specific = optional_texture_list(path, table, face.name())?;
        let face_name = face.name();
        let resolved = if let Some(list) = specific {
            (face_name, list)
        } else if face.accepts_side() {
            if let Some(list) = side.clone() {
                ("side", list)
            } else if let Some(list) = all.clone() {
                ("all", list)
            } else {
                return Err(format!(
                    "{path}: textures.{face_name}: set `{face_name}`, `side`, or `all`"
                ));
            }
        } else if let Some(list) = all.clone() {
            ("all", list)
        } else {
            return Err(format!(
                "{path}: textures.{face_name}: set `{face_name}` or `all`"
            ));
        };
        keys[face.index()] = resolved.0;
        names[face.index()] = resolved.1;
    }
    Ok(Some(FaceNames { names, keys }))
}

fn optional_texture_list(
    path: &str,
    table: &Table,
    key: &str,
) -> Result<Option<Vec<String>>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(texture_list(path, key, value)?)),
    }
}

fn texture_list(path: &str, key: &str, value: &Value) -> Result<Vec<String>, String> {
    let raw = if let Some(text) = value.as_str() {
        vec![text.to_string()]
    } else if let Some(items) = value.as_array() {
        if items.is_empty() {
            return Err(format!("{path}: textures.{key}: variant list is empty"));
        }
        let mut names = Vec::with_capacity(items.len());
        for item in items {
            let Some(text) = item.as_str() else {
                return Err(format!("{path}: textures.{key}: expected a texture id"));
            };
            names.push(text.to_string());
        }
        names
    } else {
        return Err(format!(
            "{path}: textures.{key}: expected a texture id or a list of ids"
        ));
    };
    for id in &raw {
        if !is_texture_ref(id) {
            return Err(format!(
                "{path}: textures.{key}: `{id}` is not a texture ref"
            ));
        }
    }
    Ok(raw)
}

fn parse_rotation(path: &str, value: Option<&Value>) -> Result<[TextureRotation; 6], String> {
    let Some(value) = value else {
        return Ok([TextureRotation::None; 6]);
    };
    if let Some(text) = value.as_str() {
        let rotation = rotation_value(path, "texture_rotation", text)?;
        return Ok([rotation; 6]);
    }
    let Some(table) = value.as_table() else {
        return Err(format!(
            "{path}: texture_rotation: expected \"none\", \"random\", or a table"
        ));
    };
    reject_unknown(
        path,
        table,
        &[
            "all", "top", "bottom", "side", "north", "south", "east", "west",
        ],
        "texture_rotation",
    )?;
    let all = optional_rotation(path, table, "all")?;
    let side = optional_rotation(path, table, "side")?;
    let mut rotation = [TextureRotation::None; 6];
    for face in BlockFace::ALL {
        let specific = optional_rotation(path, table, face.name())?;
        rotation[face.index()] = if let Some(value) = specific {
            value
        } else if face.accepts_side() {
            side.or(all).unwrap_or(TextureRotation::None)
        } else {
            all.unwrap_or(TextureRotation::None)
        };
    }
    Ok(rotation)
}

fn optional_rotation(
    path: &str,
    table: &Table,
    key: &str,
) -> Result<Option<TextureRotation>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => {
            let Some(text) = value.as_str() else {
                return Err(format!(
                    "{path}: texture_rotation.{key}: expected \"none\" or \"random\""
                ));
            };
            Ok(Some(rotation_value(
                path,
                &format!("texture_rotation.{key}"),
                text,
            )?))
        }
    }
}

fn rotation_value(path: &str, key: &str, text: &str) -> Result<TextureRotation, String> {
    match text {
        "none" => Ok(TextureRotation::None),
        "random" => Ok(TextureRotation::Random),
        _ => Err(format!("{path}: {key}: expected \"none\" or \"random\"")),
    }
}

fn parse_render(path: &str, text: &str) -> Result<RenderClass, String> {
    match text {
        "opaque" => Ok(RenderClass::Opaque),
        "cutout" => Ok(RenderClass::Cutout),
        "translucent" => Err(format!(
            "{path}: render: translucent is not available until a translucent render pass exists"
        )),
        "fluid" => Ok(RenderClass::Fluid),
        "invisible" => Ok(RenderClass::Invisible),
        _ => Err(format!(
            "{path}: render: expected one of opaque, cutout, fluid, invisible"
        )),
    }
}

fn require_color(path: &str, table: &Table, key: &str) -> Result<[f32; 4], String> {
    let value = require(path, table, key)?;
    parse_color(path, key, value)
}

fn optional_color(path: &str, table: &Table, key: &str) -> Result<Option<[f32; 4]>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => Ok(Some(parse_color(path, key, value)?)),
    }
}

fn parse_color(path: &str, key: &str, value: &Value) -> Result<[f32; 4], String> {
    let Some(items) = value.as_array() else {
        return Err(format!("{path}: {key}: expected four numbers in 0.0..=1.0"));
    };
    if items.len() != 4 {
        return Err(format!("{path}: {key}: expected four numbers in 0.0..=1.0"));
    }
    let mut color = [0.0; 4];
    for (index, item) in items.iter().enumerate() {
        let number = if let Some(float) = item.as_float() {
            float
        } else if let Some(integer) = item.as_integer() {
            integer as f64
        } else {
            return Err(format!("{path}: {key}: expected four numbers in 0.0..=1.0"));
        };
        if !number.is_finite() || !(0.0..=1.0).contains(&number) {
            return Err(format!("{path}: {key}: expected four numbers in 0.0..=1.0"));
        }
        color[index] = number as f32;
    }
    Ok(color)
}

fn require_string(path: &str, table: &Table, key: &str) -> Result<String, String> {
    let value = require(path, table, key)?;
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{path}: {key}: expected a string"))
}

fn require_bool(path: &str, table: &Table, key: &str) -> Result<bool, String> {
    let value = require(path, table, key)?;
    value
        .as_bool()
        .ok_or_else(|| format!("{path}: {key}: expected a boolean"))
}

fn require<'a>(path: &str, table: &'a Table, key: &str) -> Result<&'a Value, String> {
    table
        .get(key)
        .ok_or_else(|| format!("{path}: {key}: missing field"))
}

fn reject_unknown(path: &str, table: &Table, allowed: &[&str], prefix: &str) -> Result<(), String> {
    for key in table.keys() {
        if !allowed.contains(&key.as_str()) {
            let label = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            return Err(format!("{path}: {label}: unknown field"));
        }
    }
    Ok(())
}

fn block_id_from_path(path: &str) -> Result<String, String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains("://")
        || path.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(format!(
            "{path}: path must be a relative assets/<namespace>/blocks/<name>.toml path"
        ));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 4 || parts[0] != "assets" || parts[2] != "blocks" {
        return Err(format!(
            "{path}: path must be assets/<namespace>/blocks/<name>.toml"
        ));
    }
    let Some(name) = parts[3].strip_suffix(".toml") else {
        return Err(format!("{path}: file name must end in .toml"));
    };
    let id = format!("{}:{name}", parts[1]);
    if !is_block_id(&id) {
        return Err(format!(
            "{path}: namespace and name must match [a-z][a-z0-9_]{{0,31}}"
        ));
    }
    Ok(id)
}

/// `namespace:name`, each `[a-z][a-z0-9_]{0,31}`. Slashes are not block ids.
pub fn is_block_id(id: &str) -> bool {
    let Some((namespace, name)) = id.split_once(':') else {
        return false;
    };
    is_ident(namespace) && is_ident(name) && !name.contains('/')
}

/// `namespace:segment(/segment)*`. The namespace uses the block-id rule.
/// Each segment is `[a-z0-9_]+`.
pub fn is_texture_ref(id: &str) -> bool {
    let Some((namespace, path)) = id.split_once(':') else {
        return false;
    };
    if !is_ident(namespace) || path.is_empty() || path.starts_with('/') || path.ends_with('/') {
        return false;
    }
    path.split('/').all(is_texture_segment)
}

fn is_texture_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn is_ident(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_lowercase()
        && text.len() <= 32
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn is_name_key(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '_'
                || character == '.'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded_list() -> Vec<SourceFile> {
        embedded_sources_owned()
    }

    fn parse_at(path: &str, bytes: &[u8]) -> Result<Parsed, String> {
        parse_block_file(path, path, bytes)
    }

    fn stone_path() -> &'static str {
        "assets/strata/blocks/stone.toml"
    }

    #[test]
    fn id_regex_matches_the_path_shape_and_rejects_scout_extras() {
        assert!(is_block_id("strata:stone"));
        assert!(is_block_id("strata:coal_ore"));
        assert!(is_block_id("a:b"));
        assert!(!is_block_id("Strata:Stone"));
        assert!(!is_block_id("strata:stone.toml"));
        assert!(!is_block_id("mod:foo/bar"));
        assert!(is_texture_ref("strata:block/grass_top"));
        assert!(!is_texture_ref("strata:block/grass_top.alt"));
        assert!(!is_texture_ref("..."));
        assert!(!is_texture_ref("x."));
        assert!(!is_texture_ref("a-b"));
        assert!(!is_texture_ref("strata:..."));
        assert!(!is_texture_ref("strata:x."));
        assert!(!is_texture_ref("strata:a-b"));
        assert!(is_texture_ref("a:b"));
        assert!(!is_texture_ref("strata:block/../grass_top"));
        assert!(!is_texture_ref("strata:block/./grass_top"));
        assert!(!is_texture_ref("strata:block//grass_top"));
        assert!(!is_texture_ref("strata:/grass_top"));
        assert!(!is_texture_ref("strata:block/"));
        assert!(!is_texture_ref("strata:Block/grass"));
        assert!(!is_block_id("strata:"));
        assert!(!is_block_id(":stone"));
        assert!(!is_block_id("1strata:stone"));
        assert!(!is_block_id("strata:1stone"));
        assert!(!is_block_id("strata:stone:extra"));
        assert!(!is_block_id("strata:Stone"));
        let long = format!("strata:{}", "a".repeat(32));
        assert!(is_block_id(&long));
        let too_long = format!("strata:{}", "a".repeat(33));
        assert!(!is_block_id(&too_long));
    }

    #[test]
    fn paths_reject_absolute_parent_backslash_and_urls() {
        assert_eq!(
            block_id_from_path("assets/strata/blocks/stone.toml").unwrap(),
            "strata:stone"
        );
        for path in [
            "/assets/strata/blocks/stone.toml",
            "assets/strata/blocks/../stone.toml",
            "assets/strata/blocks\\stone.toml",
            "https://example.com/stone.toml",
            "assets/strata/blocks/Stone.toml",
            "assets/strata/extra/blocks/stone.toml",
            "assets/strata/blocks/stone.png",
        ] {
            assert!(block_id_from_path(path).is_err(), "{path}");
        }
    }

    #[test]
    fn manifest_errors_name_the_file_and_the_key() {
        let original = fs::read(stone_path()).unwrap();
        let text = String::from_utf8(original.clone()).unwrap();
        let with_id = format!("id = \"strata:stone\"\n{text}");
        let error = parse_at(stone_path(), with_id.as_bytes()).unwrap_err();
        assert!(error.contains(stone_path()), "{error}");
        assert!(error.contains("id"), "{error}");
        assert!(error.contains("unknown"), "{error}");

        let missing_solid = text.replace("solid = true\n", "");
        let error = parse_at(stone_path(), missing_solid.as_bytes()).unwrap_err();
        assert!(error.contains("solid"), "{error}");
        assert!(error.contains("missing"), "{error}");

        let bad_render = text.replace("render = \"opaque\"", "render = \"glow\"");
        let error = parse_at(stone_path(), bad_render.as_bytes()).unwrap_err();
        assert!(error.contains("render"), "{error}");

        let bad_tint = text.replace(
            "swatch = [0.51, 0.54, 0.56, 1.0]",
            "swatch = [0.51, 1.4, 0.56, 1.0]",
        );
        let error = parse_at(stone_path(), bad_tint.as_bytes()).unwrap_err();
        assert!(error.contains("swatch"), "{error}");

        let short_tint = text.replace(
            "swatch = [0.51, 0.54, 0.56, 1.0]",
            "swatch = [0.51, 0.54, 0.56]",
        );
        assert!(parse_at(stone_path(), short_tint.as_bytes()).is_err());

        let integer_tint =
            text.replace("swatch = [0.51, 0.54, 0.56, 1.0]", "swatch = [1, 1, 1, 1]");
        assert!(parse_at(stone_path(), integer_tint.as_bytes()).is_ok());

        let empty_display = text.replace("display = \"Limestone\"", "display = \"\"");
        let error = parse_at(stone_path(), empty_display.as_bytes()).unwrap_err();
        assert!(error.contains("display"), "{error}");

        let bad_key = text.replace("block.strata.stone", "Block.Stone");
        let error = parse_at(stone_path(), bad_key.as_bytes()).unwrap_err();
        assert!(error.contains("name_key"), "{error}");

        let mut huge = original.clone();
        huge.extend(std::iter::repeat_n(b'\n', MAX_FILE_BYTES));
        let error = parse_at(stone_path(), &huge).unwrap_err();
        assert!(error.contains("64 KiB"), "{error}");

        let mut bare_cr = original.clone();
        bare_cr.insert(0, b'\r');
        assert!(
            parse_at(stone_path(), &bare_cr)
                .unwrap_err()
                .contains("carriage")
        );

        let mut crlf = Vec::new();
        for &byte in &original {
            if byte == b'\n' {
                crlf.push(b'\r');
            }
            crlf.push(byte);
        }
        let parsed = parse_at(stone_path(), &crlf).expect("CRLF TOML parses");
        let lf = parse_at(stone_path(), &original).unwrap();
        assert_eq!(parsed.id, lf.id);
        assert_eq!(parsed.swatch, lf.swatch);
        assert_eq!(parsed.bytes, lf.bytes);
    }

    fn owned_source(path: &str, bytes: Vec<u8>) -> SourceFile {
        SourceFile {
            logical: path.to_string(),
            display: path.to_string(),
            bytes,
        }
    }

    #[test]
    fn texture_rules_cover_invisible_faces_variants_and_rotation() {
        let air = fs::read("assets/strata/blocks/air.toml").unwrap();
        let with_texture =
            String::from_utf8(air).unwrap() + "\n[textures]\nall = \"strata:block/limestone\"\n";
        let error = parse_at("assets/strata/blocks/air.toml", with_texture.as_bytes()).unwrap_err();
        assert!(error.contains("textures"), "{error}");

        let stone = String::from_utf8(fs::read(stone_path()).unwrap()).unwrap();
        let bare = stone.replace("[textures]\nall = \"strata:block/limestone\"\n", "");
        let error = parse_at(stone_path(), bare.as_bytes()).unwrap_err();
        assert!(error.contains("textures"), "{error}");

        let bad_id = stone.replace("strata:block/limestone", "Strata:Limestone");
        let error = parse_at(stone_path(), bad_id.as_bytes()).unwrap_err();
        assert!(error.contains("textures.all"), "{error}");

        let empty_list = stone.replace("all = \"strata:block/limestone\"", "all = []");
        assert!(
            parse_at(stone_path(), empty_list.as_bytes())
                .unwrap_err()
                .contains("empty")
        );

        let variants = stone.replace(
            "all = \"strata:block/limestone\"",
            "all = [\"strata:block/limestone\", \"strata:block/basalt\"]",
        );
        let parsed = parse_at(stone_path(), variants.as_bytes()).unwrap();
        assert_eq!(
            parsed.faces.unwrap().names[BlockFace::Top.index()],
            vec![
                "strata:block/limestone".to_string(),
                "strata:block/basalt".to_string()
            ]
        );

        let rotated = format!("{stone}\n[texture_rotation]\nside = \"random\"\n");
        let parsed = parse_at(stone_path(), rotated.as_bytes()).unwrap();
        assert_eq!(
            parsed.rotation,
            [
                TextureRotation::None,
                TextureRotation::None,
                TextureRotation::Random,
                TextureRotation::Random,
                TextureRotation::Random,
                TextureRotation::Random,
            ]
        );
        let uniform = stone.replacen(
            "[textures]",
            "texture_rotation = \"random\"\n\n[textures]",
            1,
        );
        let parsed = parse_at(stone_path(), uniform.as_bytes()).unwrap();
        assert_eq!(parsed.rotation, [TextureRotation::Random; 6]);
        let bad = stone.replacen("[textures]", "texture_rotation = \"spin\"\n\n[textures]", 1);
        assert!(
            parse_at(stone_path(), bad.as_bytes())
                .unwrap_err()
                .contains("texture_rotation")
        );
    }

    #[test]
    fn duplicate_ids_and_broken_builtin_rules_fail_closed() {
        let stone = fs::read(stone_path()).unwrap();
        let error = assemble(vec![
            owned_source(stone_path(), stone.clone()),
            owned_source(stone_path(), stone),
        ])
        .unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
        assert!(error.contains("strata:stone"), "{error}");

        let mut sources = embedded_list();
        sources.swap(0, 3);
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("strata:air"), "{error}");

        let mut sources = embedded_list();
        let water = sources
            .iter()
            .position(|source| source.logical.ends_with("water.toml"))
            .unwrap();
        let text = String::from_utf8(sources[water].bytes.clone()).unwrap();
        sources[water].bytes = text.replace("solid = false", "solid = true").into_bytes();
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("strata:water"), "{error}");
        assert!(error.contains("water.toml"), "{error}");
    }

    #[test]
    fn builtins_freeze_in_file_order_with_sorted_layers() {
        let registry = registry();
        if env_data_dir().is_none() {
            assert!(!registry.loaded_from_disk());
        }
        assert_eq!(registry.builtin_count(), BUILTIN_FILES.len());
        assert_eq!(registry.flag_rows().len(), registry.builtin_count());
        assert_eq!(AIR.raw(), 0);
        assert_eq!(registry.get("strata:air"), Some(AIR));
        assert_eq!(registry.get("strata:stone"), Some(STONE));
        assert_eq!(STONE.raw(), 3);
        assert_eq!(DIAMOND_ORE.raw(), 15);
        assert_eq!(registry.get("strata:diamond_ore"), Some(DIAMOND_ORE));
        assert_eq!(MISSING.raw(), 16);
        assert_eq!(registry.get("strata:missing"), Some(MISSING));
        assert!(install().is_ok());
        assert_eq!(WATER.tint(), IDENTITY_TINT);
        assert_eq!(WATER.render_class(), RenderClass::Fluid);
        assert_eq!(render_class(WATER), RenderClass::Fluid);
        assert_eq!(LEAVES.render_class(), RenderClass::Cutout);
        assert_eq!(render_class(LEAVES), RenderClass::Cutout);
        assert_eq!(render_class(STONE), RenderClass::Opaque);
        assert_eq!(render_class(AIR), RenderClass::Invisible);
        assert!(!WATER.is_solid());
        assert!(LEAVES.is_solid());
        assert!(LEAVES.is_placeable());
        assert!(!MISSING.is_placeable());
        assert!(MISSING.is_solid());
        assert!(AIR.is_replaceable());
        assert!(!GRASS.is_replaceable());
        let textures = registry.textures_sorted();
        let mut sorted = textures.to_vec();
        sorted.sort();
        assert_eq!(textures, sorted.as_slice());
        assert!(registry.load_warnings().is_empty());
        for texture in textures {
            assert!(
                crate::materials::authored_texture(texture),
                "{texture} is not an authored tile"
            );
        }
        let grass_top = registry.texture_index("strata:block/grass_top").unwrap();
        let grass_side = registry.texture_index("strata:block/grass_side").unwrap();
        let soil = registry.texture_index("strata:block/soil").unwrap();
        let missing = registry.missing_layer();
        assert_eq!(registry.texture_index(MISSING_TEXTURE), Some(missing));
        assert_ne!(registry.flags(MISSING).layers, registry.flags(STONE).layers);
        assert_eq!(
            registry.texture_variants(GRASS, BlockFace::Top),
            &[grass_top]
        );
        assert_eq!(registry.texture_variants(GRASS, BlockFace::Bottom), &[soil]);
        for face in [
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            assert_eq!(registry.texture_variants(GRASS, face), &[grass_side]);
        }
        assert_eq!(
            registry.texture_rotation(STONE, BlockFace::North),
            TextureRotation::None
        );
        assert_eq!(registry.display(STONE), "Limestone");
        assert_eq!(registry.name_key(AIR), "block.strata.air");
        assert_eq!(registry.builtin_content_hash().len(), 64);
        assert!(
            registry
                .builtin_content_hash()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(
            registry.flag_rows()[WATER.index()].render,
            RenderClass::Fluid
        );
        let again = assemble(embedded_list()).unwrap();
        assert_eq!(
            again.builtin_content_hash(),
            registry.builtin_content_hash()
        );
        for (path, text) in embedded_sources() {
            assert_eq!(fs::read(path).unwrap(), text.as_bytes(), "{path}");
        }
    }

    #[test]
    fn placeholder_template_is_solid_unplaceable_and_uses_the_missing_texture() {
        let registry = registry();
        let unknown = BlockId(17);
        let flags = registry.flags(unknown);
        assert!(flags.solid);
        assert!(!flags.placeable);
        assert_eq!(flags.render, RenderClass::Opaque);
        assert_eq!(flags.tint, IDENTITY_TINT);
        assert_eq!(flags.swatch, registry.flags(MISSING).swatch);
        assert_eq!(flags.layers, registry.flags(MISSING).layers);
        assert_eq!(
            registry.texture_variants(unknown, BlockFace::North),
            registry.texture_variants(MISSING, BlockFace::North)
        );
    }

    #[test]
    fn builtin_texture_layer_count_is_pinned_at_18() {
        let count = assemble(embedded_list()).unwrap().texture_count();
        assert_eq!(
            count, 18,
            "builtin texture layer count is {count}, and the pin is 18. Update this test only when a texture layer is added or removed on purpose."
        );
    }

    #[test]
    fn empty_data_dir_override_selects_the_embedded_registry() {
        assert_eq!(data_dir_override(None), None);
        assert_eq!(data_dir_override(Some(std::ffi::OsStr::new(""))), None);
        assert_eq!(
            data_dir_override(Some(std::ffi::OsStr::new("packs"))),
            Some(PathBuf::from("packs"))
        );
    }

    #[test]
    fn grass_top_set_to_snow_is_a_load_error() {
        let sources = replace_embedded(
            "grass.toml",
            "top = \"strata:block/grass_top\"",
            "top = \"strata:block/snow\"",
        );
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("grass.toml"), "{error}");
        assert!(error.contains("textures.top"), "{error}");
    }

    #[test]
    fn non_white_water_tint_is_a_load_error() {
        let sources = replace_embedded(
            "water.toml",
            "tint = [1.0, 1.0, 1.0, 1.0]",
            "tint = [0.2, 0.4, 0.9, 1.0]",
        );
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("water.toml"), "{error}");
        assert!(error.contains("tint"), "{error}");
    }

    #[test]
    fn random_stone_rotation_is_a_load_error() {
        let sources = replace_embedded(
            "stone.toml",
            "[textures]",
            "texture_rotation = \"random\"\n\n[textures]",
        );
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("stone.toml"), "{error}");
        assert!(error.contains("texture_rotation"), "{error}");
    }

    #[test]
    fn oversized_block_file_is_rejected_from_its_length() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("strata-oversize-{nonce}"));
        let blocks = root.join(BLOCKS_RELATIVE);
        fs::create_dir_all(&blocks).unwrap();
        for (path, text) in embedded_sources() {
            let name = path.rsplit('/').next().unwrap();
            fs::write(blocks.join(name), text).unwrap();
        }
        let stone = blocks.join("stone.toml");
        let mut big = fs::read(&stone).unwrap();
        big.resize(MAX_FILE_BYTES + 1, b'\n');
        fs::write(&stone, big).unwrap();
        let error = Registry::load_from(Some(&root)).unwrap_err();
        assert!(error.contains("stone.toml"), "{error}");
        assert!(error.contains("64 KiB"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn texture_layer_cap_is_256() {
        let mut names = BTreeSet::new();
        for index in 0..MAX_TEXTURE_LAYERS {
            names.insert(format!("mod:block/layer{index}"));
        }
        assert!(index_texture_layers(names).is_ok());
        let mut over = BTreeSet::new();
        for index in 0..=MAX_TEXTURE_LAYERS {
            over.insert(format!("mod:block/layer{index}"));
        }
        assert_eq!(
            index_texture_layers(over).unwrap_err(),
            format!(
                "{} texture layers exceed {MAX_TEXTURE_LAYERS}",
                MAX_TEXTURE_LAYERS + 1
            )
        );
    }

    #[test]
    fn unknown_texture_ref_adds_no_layer_and_uses_the_missing_checker() {
        let mut sources = embedded_list();
        let grass = sources
            .iter_mut()
            .find(|source| source.logical.ends_with("grass.toml"))
            .unwrap();
        let text = String::from_utf8(grass.bytes.clone()).unwrap();
        grass.bytes = text
            .replace(
                "side = \"strata:block/grass_side\"",
                "side = [\"strata:block/grass_side\", \"strata:block/not_a_tile\"]",
            )
            .into_bytes();
        let broken = assemble(sources).unwrap();
        let clean = assemble(embedded_list()).unwrap();
        assert_eq!(broken.texture_count(), clean.texture_count());
        assert!(broken.texture_index("strata:block/not_a_tile").is_none());
        let side = broken.texture_index("strata:block/grass_side").unwrap();
        assert_eq!(
            broken.texture_variants(GRASS, BlockFace::North),
            &[side, broken.missing_layer()]
        );
        let warning = broken.load_warnings().join("\n");
        assert!(warning.contains("grass.toml"), "{warning}");
        assert!(warning.contains("textures.side"), "{warning}");
        assert!(warning.contains("strata:block/not_a_tile"), "{warning}");
        let startup = crate::startup_failure_text(&warning);
        assert!(startup.starts_with(crate::STRATA_START_FAILED));
        assert!(startup.contains("grass.toml"));
        assert!(startup.contains("textures.side"));
        assert!(startup.ends_with('\n'));
    }

    #[test]
    fn every_builtin_texture_ref_names_an_authored_tile() {
        for (path, text) in embedded_sources() {
            let parsed = parse_at(path, text.as_bytes()).unwrap();
            let Some(faces) = parsed.faces else {
                continue;
            };
            for list in &faces.names {
                for id in list {
                    assert!(
                        crate::materials::authored_texture(id),
                        "{path} names unknown texture {id}"
                    );
                }
            }
        }
    }

    #[test]
    fn cardinal_faces_use_all_then_side_then_the_specific_face() {
        let stone = String::from_utf8(fs::read(stone_path()).unwrap()).unwrap();
        let custom = stone.replace(
            "[textures]\nall = \"strata:block/limestone\"\n",
            "[textures]\nall = \"strata:block/limestone\"\nside = \"strata:block/basalt\"\ntop = \"strata:block/sand\"\nnorth = \"strata:block/clay\"\n",
        );
        let parsed = parse_at(stone_path(), custom.as_bytes()).unwrap();
        let faces = parsed.faces.unwrap();
        assert_eq!(faces.names[BlockFace::Top.index()], ["strata:block/sand"]);
        assert_eq!(faces.keys[BlockFace::Top.index()], "top");
        assert_eq!(
            faces.names[BlockFace::Bottom.index()],
            ["strata:block/limestone"]
        );
        assert_eq!(faces.keys[BlockFace::Bottom.index()], "all");
        assert_eq!(faces.names[BlockFace::North.index()], ["strata:block/clay"]);
        assert_eq!(faces.keys[BlockFace::North.index()], "north");
        for face in [BlockFace::South, BlockFace::East, BlockFace::West] {
            assert_eq!(faces.names[face.index()], ["strata:block/basalt"]);
            assert_eq!(faces.keys[face.index()], "side");
        }
        let rotated = format!(
            "{custom}\n[texture_rotation]\nall = \"none\"\nside = \"random\"\nnorth = \"none\"\n"
        );
        let parsed = parse_at(stone_path(), rotated.as_bytes()).unwrap();
        assert_eq!(
            parsed.rotation[BlockFace::Top.index()],
            TextureRotation::None
        );
        assert_eq!(
            parsed.rotation[BlockFace::Bottom.index()],
            TextureRotation::None
        );
        assert_eq!(
            parsed.rotation[BlockFace::North.index()],
            TextureRotation::None
        );
        assert_eq!(
            parsed.rotation[BlockFace::East.index()],
            TextureRotation::Random
        );
    }

    #[test]
    fn translucent_render_and_fluid_tint_alpha_are_rejected() {
        let snow = fs::read("assets/strata/blocks/snow.toml").unwrap();
        let text = String::from_utf8(snow).unwrap();
        let translucent = text.replace("render = \"opaque\"", "render = \"translucent\"");
        let error = parse_at("assets/strata/blocks/snow.toml", translucent.as_bytes()).unwrap_err();
        assert!(error.contains("snow.toml"), "{error}");
        assert!(error.contains("render"), "{error}");
        assert!(error.contains("translucent"), "{error}");

        let water = fs::read("assets/strata/blocks/water.toml").unwrap();
        let text = String::from_utf8(water).unwrap();
        let alpha = text.replace(
            "tint = [1.0, 1.0, 1.0, 1.0]",
            "tint = [0.10, 0.44, 0.53, 0.72]",
        );
        let error = parse_at("assets/strata/blocks/water.toml", alpha.as_bytes()).unwrap_err();
        assert!(error.contains("water.toml"), "{error}");
        assert!(error.contains("tint"), "{error}");
        assert!(error.contains("1.0"), "{error}");
    }

    #[test]
    fn data_dir_ignores_non_toml_parses_crlf_and_names_a_bad_key() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("strata-data-{}-{nonce}", std::process::id()));
        let blocks = root.join(BLOCKS_RELATIVE);
        fs::create_dir_all(&blocks).unwrap();
        for (path, text) in embedded_sources() {
            let name = path.rsplit('/').next().unwrap();
            fs::write(blocks.join(name), text.replace('\n', "\r\n")).unwrap();
        }
        fs::write(blocks.join("desktop.ini"), b"[.ShellClassInfo]\r\n").unwrap();
        let loaded = Registry::load_from(Some(&root)).unwrap();
        let embedded = assemble(embedded_list()).unwrap();
        assert!(loaded.loaded_from_disk());
        assert_eq!(
            loaded.builtin_content_hash(),
            embedded.builtin_content_hash()
        );
        assert!(loaded.load_warnings().is_empty());
        fs::write(blocks.join("notes.toml"), b"x = 1\n").unwrap();
        let unexpected = Registry::load_from(Some(&root)).unwrap_err();
        assert!(unexpected.contains("notes.toml"), "{unexpected}");
        assert!(unexpected.contains("unexpected"), "{unexpected}");
        fs::remove_file(blocks.join("notes.toml")).unwrap();
        let stone = blocks.join("stone.toml");
        let text = fs::read_to_string(&stone).unwrap();
        fs::write(&stone, text.replace("solid = true", "solid = \"yes\"")).unwrap();
        let error = Registry::load_from(Some(&root)).unwrap_err();
        assert!(error.contains("stone.toml"), "{error}");
        assert!(error.contains("solid"), "{error}");
        let missing = root.join("does-not-exist");
        let error = Registry::load_from(Some(&missing)).unwrap_err();
        assert!(error.contains("block directory is missing"), "{error}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn frozen_block_properties_match_the_builtin_table() {
        let registry = assemble(embedded_list()).expect("builtin blocks");
        let rows = [
            row(
                AIR,
                "strata:air",
                (false, false, true),
                RenderClass::Invisible,
                [0.0, 0.0, 0.0, 0.0],
                ["strata:missing"; 6],
            ),
            row(
                GRASS,
                "strata:grass",
                (true, true, false),
                RenderClass::Opaque,
                [0.38, 0.58, 0.22, 1.0],
                [
                    "strata:block/grass_top",
                    "strata:block/soil",
                    "strata:block/grass_side",
                    "strata:block/grass_side",
                    "strata:block/grass_side",
                    "strata:block/grass_side",
                ],
            ),
            row(
                DIRT,
                "strata:dirt",
                (true, true, false),
                RenderClass::Opaque,
                [0.39, 0.26, 0.16, 1.0],
                ["strata:block/soil"; 6],
            ),
            row(
                STONE,
                "strata:stone",
                (true, true, false),
                RenderClass::Opaque,
                [0.51, 0.54, 0.56, 1.0],
                ["strata:block/limestone"; 6],
            ),
            row(
                SAND,
                "strata:sand",
                (true, true, false),
                RenderClass::Opaque,
                [0.80, 0.72, 0.48, 1.0],
                ["strata:block/sand"; 6],
            ),
            row(
                WATER,
                "strata:water",
                (false, false, false),
                RenderClass::Fluid,
                [0.10, 0.44, 0.53, 0.72],
                ["strata:block/water"; 6],
            ),
            row(
                WOOD,
                "strata:wood",
                (true, true, false),
                RenderClass::Opaque,
                [0.38, 0.24, 0.14, 1.0],
                [
                    "strata:block/endgrain",
                    "strata:block/endgrain",
                    "strata:block/bark",
                    "strata:block/bark",
                    "strata:block/bark",
                    "strata:block/bark",
                ],
            ),
            row(
                LEAVES,
                "strata:leaves",
                (true, true, false),
                RenderClass::Cutout,
                [0.22, 0.43, 0.22, 1.0],
                ["strata:block/leaves"; 6],
            ),
            row(
                SNOW,
                "strata:snow",
                (true, true, false),
                RenderClass::Opaque,
                [0.87, 0.91, 0.91, 1.0],
                ["strata:block/snow"; 6],
            ),
            row(
                CLAY,
                "strata:clay",
                (true, true, false),
                RenderClass::Opaque,
                [0.66, 0.34, 0.22, 1.0],
                ["strata:block/clay"; 6],
            ),
            row(
                CRYSTAL,
                "strata:crystal",
                (true, true, false),
                RenderClass::Opaque,
                [0.28, 0.86, 0.89, 1.0],
                ["strata:block/crystal"; 6],
            ),
            row(
                BASALT,
                "strata:basalt",
                (true, true, false),
                RenderClass::Opaque,
                [0.24, 0.28, 0.31, 1.0],
                ["strata:block/basalt"; 6],
            ),
            row(
                COAL_ORE,
                "strata:coal_ore",
                (true, true, false),
                RenderClass::Opaque,
                [0.16, 0.16, 0.17, 1.0],
                ["strata:block/coal_ore"; 6],
            ),
            row(
                IRON_ORE,
                "strata:iron_ore",
                (true, true, false),
                RenderClass::Opaque,
                [0.66, 0.48, 0.40, 1.0],
                ["strata:block/iron_ore"; 6],
            ),
            row(
                GOLD_ORE,
                "strata:gold_ore",
                (true, true, false),
                RenderClass::Opaque,
                [0.86, 0.72, 0.28, 1.0],
                ["strata:block/gold_ore"; 6],
            ),
            row(
                DIAMOND_ORE,
                "strata:diamond_ore",
                (true, true, false),
                RenderClass::Opaque,
                [0.45, 0.86, 0.84, 1.0],
                ["strata:block/diamond_ore"; 6],
            ),
            row(
                MISSING,
                "strata:missing",
                (true, false, false),
                RenderClass::Opaque,
                [0.85, 0.15, 0.45, 1.0],
                ["strata:missing"; 6],
            ),
        ];
        assert_eq!(rows.len(), registry.builtin_count());
        for expected in rows {
            let flags = registry.flags(expected.id);
            assert_eq!(
                registry.name(expected.id),
                expected.name,
                "{}",
                expected.name
            );
            assert_eq!(flags.solid, expected.solid, "{}", expected.name);
            assert_eq!(flags.placeable, expected.placeable, "{}", expected.name);
            assert_eq!(flags.replaceable, expected.replaceable, "{}", expected.name);
            assert_eq!(flags.render, expected.render, "{}", expected.name);
            assert_eq!(flags.tint, IDENTITY_TINT, "{}", expected.name);
            assert_eq!(
                flags.swatch,
                cast_color(expected.swatch),
                "{}",
                expected.name
            );
            for face in BlockFace::ALL {
                let layer = registry
                    .texture_index(expected.faces[face.index()])
                    .unwrap();
                assert_eq!(
                    registry.texture_variants(expected.id, face),
                    &[layer],
                    "{} {:?}",
                    expected.name,
                    face
                );
                assert_eq!(
                    registry.texture_rotation(expected.id, face),
                    TextureRotation::None,
                    "{}",
                    expected.name
                );
            }
        }
    }

    fn replace_embedded(file_name: &str, from: &str, to: &str) -> Vec<SourceFile> {
        let mut sources = embedded_list();
        let source = sources
            .iter_mut()
            .find(|source| source.logical.ends_with(file_name))
            .unwrap();
        let text = String::from_utf8(source.bytes.clone()).unwrap();
        assert!(text.contains(from), "{file_name} does not contain {from:?}");
        source.bytes = text.replacen(from, to, 1).into_bytes();
        sources
    }

    /// The exact property table must notice each review mutation. Translucent
    /// snow and a non-1 fluid tint fail in the parser, so they never freeze.
    #[test]
    fn exact_table_rejects_the_review_mutations() {
        let clean = assemble(embedded_list()).unwrap();

        let crystal = assemble(replace_embedded(
            "crystal.toml",
            "tint = [1.0, 1.0, 1.0, 1.0]",
            "tint = [0.28, 0.86, 0.89, 1.0]",
        ))
        .unwrap();
        assert_ne!(crystal.flags(CRYSTAL).tint, clean.flags(CRYSTAL).tint);
        assert_ne!(crystal.flags(CRYSTAL).tint, IDENTITY_TINT);

        let clay = assemble(replace_embedded(
            "clay.toml",
            "replaceable = false",
            "replaceable = true",
        ))
        .unwrap();
        assert!(clay.flags(CLAY).replaceable);
        assert!(!clean.flags(CLAY).replaceable);

        let basalt = assemble(replace_embedded(
            "basalt.toml",
            "render = \"opaque\"",
            "render = \"cutout\"",
        ))
        .unwrap();
        assert_eq!(basalt.flags(BASALT).render, RenderClass::Cutout);
        assert_eq!(clean.flags(BASALT).render, RenderClass::Opaque);

        let snow = assemble(replace_embedded(
            "snow.toml",
            "render = \"opaque\"",
            "render = \"translucent\"",
        ));
        let error = snow.unwrap_err();
        assert!(error.contains("snow.toml"), "{error}");
        assert!(error.contains("translucent"), "{error}");

        let water_tint = assemble(replace_embedded(
            "water.toml",
            "tint = [1.0, 1.0, 1.0, 1.0]",
            "tint = [0.10, 0.44, 0.53, 0.72]",
        ));
        let error = water_tint.unwrap_err();
        assert!(error.contains("water.toml"), "{error}");
        assert!(error.contains("fluid tint alpha"), "{error}");

        let water_swatch = assemble(replace_embedded(
            "water.toml",
            "swatch = [0.10, 0.44, 0.53, 0.72]",
            "swatch = [0.10, 0.44, 0.53, 1.0]",
        ))
        .unwrap();
        assert_ne!(
            water_swatch.flags(WATER).swatch[3],
            clean.flags(WATER).swatch[3]
        );
        assert_eq!(clean.flags(WATER).swatch[3], 0.72);
    }

    struct ExpectedBlock {
        id: BlockId,
        name: &'static str,
        solid: bool,
        placeable: bool,
        replaceable: bool,
        render: RenderClass,
        swatch: [f64; 4],
        faces: [&'static str; 6],
    }

    fn row(
        id: BlockId,
        name: &'static str,
        solid_placeable_replaceable: (bool, bool, bool),
        render: RenderClass,
        swatch: [f64; 4],
        faces: [&'static str; 6],
    ) -> ExpectedBlock {
        let (solid, placeable, replaceable) = solid_placeable_replaceable;
        ExpectedBlock {
            id,
            name,
            solid,
            placeable,
            replaceable,
            render,
            swatch,
            faces,
        }
    }

    fn cast_color(value: [f64; 4]) -> [f32; 4] {
        value.map(|channel| channel as f32)
    }

    #[test]
    fn sha256_matches_fips_180_4_vectors() {
        assert_eq!(
            hex_encode(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_encode(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
