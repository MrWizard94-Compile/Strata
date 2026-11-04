//! Namespaced block definitions loaded from TOML, the same way a data mod would.
//!
//! Builtins live at `assets/<namespace>/blocks/<name>.toml`. The id is the path,
//! not a field in the file. `Registry::load` runs once, assigns dense [`BlockId`]
//! values in file order, and freezes. Worldgen resolves names at `World::new`.
//! The hot path indexes [`BlockFlags`] by id. This module owns the only TOML parser.

#[path = "sha256.rs"]
mod sha256;

use sha256::{hex_encode, sha256};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::Path,
    sync::{Arc, OnceLock},
};
use toml::{Table, Value};

const MAX_FILE_BYTES: usize = 64 * 1024;
/// `BlockId` is a `u16`. Palette length is also capped at 65,535, so the
/// 65,536th definition is refused.
const MAX_BLOCKS: usize = 65_535;
const BLOCK_DIR: &str = "assets/strata/blocks";

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

    pub fn is_replaceable(self) -> bool {
        registry().flags(self).replaceable
    }

    pub fn tint(self) -> [f32; 4] {
        registry().flags(self).tint
    }

    /// Hotbar swatch. Same numbers as [`Self::tint`].
    pub fn color(self) -> [f32; 4] {
        self.tint()
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RenderClass {
    Opaque = 0,
    Cutout = 1,
    Fluid = 2,
    Translucent = 3,
    Invisible = 4,
}

/// One row of the dense flag table. Indexed by [`BlockId::index`].
#[derive(Clone, Copy, Debug)]
pub struct BlockFlags {
    pub solid: bool,
    pub placeable: bool,
    pub replaceable: bool,
    pub render: RenderClass,
    pub tint: [f32; 4],
    /// First texture variant for top, side, bottom. Layer numbers are assigned
    /// when the registry freezes, in sorted texture-id order.
    pub layers: [u32; 3],
}

/// Which face a texture list or rotation applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockFace {
    Top = 0,
    Side = 1,
    Bottom = 2,
}

impl BlockFace {
    pub fn from_normal(normal: [i32; 3]) -> Self {
        if normal[1] > 0 {
            Self::Top
        } else if normal[1] < 0 {
            Self::Bottom
        } else {
            Self::Side
        }
    }

    fn index(self) -> usize {
        self as usize
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
    top: Vec<u32>,
    side: Vec<u32>,
    bottom: Vec<u32>,
}

impl FaceLists {
    fn get(&self, face: BlockFace) -> &[u32] {
        match face {
            BlockFace::Top => &self.top,
            BlockFace::Side => &self.side,
            BlockFace::Bottom => &self.bottom,
        }
    }

    fn firsts(&self) -> [u32; 3] {
        [self.top[0], self.side[0], self.bottom[0]]
    }
}

/// Frozen builtin definitions. Placeholders are not inserted here: every
/// unknown name shares one flag template, and the world remembers the string.
#[derive(Debug)]
pub struct Registry {
    rows: Vec<BlockFlags>,
    names: Vec<String>,
    name_keys: Vec<String>,
    displays: Vec<String>,
    by_name: HashMap<String, BlockId>,
    textures: Vec<String>,
    texture_index: HashMap<String, u32>,
    variants: Vec<FaceLists>,
    rotations: Vec<[TextureRotation; 3]>,
    placeholder: BlockFlags,
    placeholder_variants: FaceLists,
    content_hash: String,
    from_disk: bool,
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

/// Load the builtin registry if this process has not already done so.
/// A packaged executable with no `assets/` tree uses the embedded copies.
pub fn install() -> Result<Arc<Registry>, String> {
    match REGISTRY.get_or_init(load_arc) {
        Ok(registry) => Ok(Arc::clone(registry)),
        Err(error) => Err(error.clone()),
    }
}

pub fn registry() -> &'static Registry {
    match REGISTRY.get_or_init(load_arc) {
        Ok(registry) => registry.as_ref(),
        Err(error) => panic!("block registry failed to load: {error}"),
    }
}

pub fn registry_handle() -> Arc<Registry> {
    match REGISTRY.get_or_init(load_arc) {
        Ok(registry) => Arc::clone(registry),
        Err(error) => panic!("block registry failed to load: {error}"),
    }
}

/// Render class for `id`, read from the dense flag table.
pub fn render_class(id: BlockId) -> RenderClass {
    registry().render_class(id)
}

fn load_arc() -> Result<Arc<Registry>, String> {
    Registry::load().map(Arc::new)
}

impl Registry {
    fn load() -> Result<Self, String> {
        let from_disk = Path::new(BLOCK_DIR).is_dir();
        let sources = if from_disk {
            read_builtin_directory()?
        } else {
            embedded_bytes()
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

    pub fn texture_rotation(&self, id: BlockId, face: BlockFace) -> TextureRotation {
        self.rotations
            .get(id.index())
            .map(|faces| faces[face.index()])
            .unwrap_or(TextureRotation::None)
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

fn embedded_bytes() -> Vec<(String, Vec<u8>)> {
    embedded_sources()
        .into_iter()
        .map(|(path, text)| (path.to_string(), text.as_bytes().to_vec()))
        .collect()
}

fn read_builtin_directory() -> Result<Vec<(String, Vec<u8>)>, String> {
    let dir = Path::new(BLOCK_DIR);
    let expected: Vec<&str> = BUILTIN_FILES
        .iter()
        .map(|path| {
            path.rsplit('/')
                .next()
                .expect("builtin path has a file name")
        })
        .collect();
    let mut seen = Vec::new();
    let entries = fs::read_dir(dir).map_err(|error| format!("{BLOCK_DIR}: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{BLOCK_DIR}: {error}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !expected.contains(&name.as_ref()) {
            return Err(format!("{BLOCK_DIR}/{name}: unexpected block file"));
        }
        let metadata = entry
            .metadata()
            .map_err(|error| format!("{BLOCK_DIR}/{name}: {error}"))?;
        if !metadata.is_file() {
            return Err(format!("{BLOCK_DIR}/{name}: not a file"));
        }
        seen.push(name.into_owned());
    }
    for name in &expected {
        if !seen.iter().any(|found| found == name) {
            return Err(format!("{BLOCK_DIR}/{name}: missing builtin block file"));
        }
    }
    let mut sources = Vec::with_capacity(BUILTIN_FILES.len());
    for path in BUILTIN_FILES {
        let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        sources.push(((*path).to_string(), bytes));
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
    faces: Option<FaceNames>,
    rotation: [TextureRotation; 3],
    bytes: Vec<u8>,
}

#[derive(Debug)]
struct FaceNames {
    top: Vec<String>,
    side: Vec<String>,
    bottom: Vec<String>,
}

fn assemble(sources: Vec<(String, Vec<u8>)>) -> Result<Registry, String> {
    if sources.len() > MAX_BLOCKS {
        return Err(format!(
            "block registry refuses the 65536th block ({MAX_BLOCKS} is the maximum)"
        ));
    }
    let mut parsed = Vec::with_capacity(sources.len());
    for (path, bytes) in sources {
        parsed.push(parse_block_file(&path, &bytes)?);
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
    let air = &blocks[0];
    if air.id != "strata:air" {
        return Err(format!("{}: strata:air must be registry index 0", air.path));
    }
    if air.solid || air.placeable || !air.replaceable || air.render != RenderClass::Invisible {
        return Err(format!(
            "{}: solid: strata:air must be non-solid, non-placeable, replaceable, and invisible",
            air.path
        ));
    }
    let water = find_block(blocks, "strata:water")?;
    if water.solid || water.render != RenderClass::Fluid {
        return Err(format!(
            "{}: render: strata:water must be non-solid and fluid",
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
    let missing = find_block(blocks, "strata:missing")?;
    if !missing.solid || missing.placeable || missing.render != RenderClass::Opaque {
        return Err(format!(
            "{}: placeable: strata:missing must be solid, opaque, and not placeable",
            missing.path
        ));
    }
    Ok(())
}

fn find_block<'a>(blocks: &'a [Parsed], id: &str) -> Result<&'a Parsed, String> {
    blocks
        .iter()
        .find(|block| block.id == id)
        .ok_or_else(|| format!("{id}: missing builtin block"))
}

fn freeze(blocks: Vec<Parsed>) -> Result<Registry, String> {
    let mut texture_names = BTreeSet::new();
    for block in &blocks {
        if let Some(faces) = &block.faces {
            for list in [&faces.top, &faces.side, &faces.bottom] {
                texture_names.extend(list.iter().cloned());
            }
        }
    }
    let textures: Vec<String> = texture_names.into_iter().collect();
    let texture_index: HashMap<String, u32> = textures
        .iter()
        .enumerate()
        .map(|(index, id)| (id.clone(), index as u32))
        .collect();
    let limestone = texture_index.get("strata:limestone").copied().unwrap_or(0);
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
                top: map_layers(&texture_index, &faces.top),
                side: map_layers(&texture_index, &faces.side),
                bottom: map_layers(&texture_index, &faces.bottom),
            },
            None => FaceLists {
                top: vec![limestone],
                side: vec![limestone],
                bottom: vec![limestone],
            },
        };
        let id = BlockId(index as u16);
        rows.push(BlockFlags {
            solid: block.solid,
            placeable: block.placeable,
            replaceable: block.replaceable,
            render: block.render,
            tint: block.tint,
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
    let missing_layer = texture_index
        .get("strata:missing")
        .copied()
        .unwrap_or(limestone);
    let missing_tint = rows
        .get(MISSING.index())
        .map(|row| row.tint)
        .unwrap_or([0.85, 0.15, 0.45, 1.0]);
    let placeholder_variants = FaceLists {
        top: vec![missing_layer],
        side: vec![missing_layer],
        bottom: vec![missing_layer],
    };
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
            placeable: true,
            replaceable: false,
            render: RenderClass::Opaque,
            tint: missing_tint,
            layers: [missing_layer; 3],
        },
        placeholder_variants,
        content_hash: hex_encode(&sha256(&preimage)),
        from_disk: false,
    })
}

fn map_layers(index: &HashMap<String, u32>, names: &[String]) -> Vec<u32> {
    names.iter().map(|name| index[name]).collect()
}

fn parse_block_file(path: &str, bytes: &[u8]) -> Result<Parsed, String> {
    let id = block_id_from_path(path)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("{path}: file exceeds 64 KiB"));
    }
    if bytes.contains(&b'\r') {
        return Err(format!("{path}: carriage return is not allowed"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{path}: file is not UTF-8"))?;
    let table: Table = text.parse().map_err(|error| format!("{path}: {error}"))?;
    reject_unknown(
        path,
        &table,
        &[
            "name_key",
            "display",
            "solid",
            "placeable",
            "replaceable",
            "render",
            "tint",
            "textures",
            "texture_rotation",
        ],
        "",
    )?;
    let name_key = require_string(path, &table, "name_key")?;
    if !is_name_key(&name_key) {
        return Err(format!("{path}: name_key: must match [a-z0-9_.]{{1,64}}"));
    }
    let display = require_string(path, &table, "display")?;
    if display.is_empty() {
        return Err(format!("{path}: display: must not be empty"));
    }
    if display.len() > 64 {
        return Err(format!("{path}: display: longer than 64 bytes"));
    }
    let solid = require_bool(path, &table, "solid")?;
    let placeable = require_bool(path, &table, "placeable")?;
    let replaceable = require_bool(path, &table, "replaceable")?;
    let render = parse_render(path, &require_string(path, &table, "render")?)?;
    let tint = require_tint(path, &table)?;
    let faces = parse_textures(path, table.get("textures"))?;
    let rotation_specified = table.contains_key("texture_rotation");
    let rotation = parse_rotation(path, table.get("texture_rotation"))?;
    if render == RenderClass::Invisible {
        if faces.is_some() {
            return Err(format!(
                "{path}: textures: invisible blocks cannot name a texture"
            ));
        }
        if rotation_specified {
            return Err(format!(
                "{path}: texture_rotation: invisible blocks cannot rotate a texture"
            ));
        }
    } else if faces.is_none() {
        return Err(format!(
            "{path}: textures: set `all`, or all of `top`, `side`, and `bottom`"
        ));
    }
    Ok(Parsed {
        path: path.to_string(),
        id,
        name_key,
        display,
        solid,
        placeable,
        replaceable,
        render,
        tint,
        faces,
        rotation,
        bytes: bytes.to_vec(),
    })
}

fn parse_textures(path: &str, value: Option<&Value>) -> Result<Option<FaceNames>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(table) = value.as_table() else {
        return Err(format!("{path}: textures: expected a table"));
    };
    reject_unknown(path, table, &["all", "top", "side", "bottom"], "textures")?;
    let all = optional_texture_list(path, table, "all")?;
    let top = optional_texture_list(path, table, "top")?;
    let side = optional_texture_list(path, table, "side")?;
    let bottom = optional_texture_list(path, table, "bottom")?;
    let has_all = all.is_some();
    let has_three = top.is_some() && side.is_some() && bottom.is_some();
    if !has_all && !has_three {
        return Err(format!(
            "{path}: textures: set `all`, or all of `top`, `side`, and `bottom`"
        ));
    }
    // Precedence is `all`, then `side` for side faces, then the specific face.
    let top = top.or_else(|| all.clone()).expect("all or top is set");
    let side = side.or_else(|| all.clone()).expect("all or side is set");
    let bottom = bottom
        .or_else(|| all.clone())
        .expect("all or bottom is set");
    Ok(Some(FaceNames { top, side, bottom }))
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
        if !is_block_id(id) {
            return Err(format!(
                "{path}: textures.{key}: `{id}` does not match the id regex"
            ));
        }
    }
    Ok(raw)
}

fn parse_rotation(path: &str, value: Option<&Value>) -> Result<[TextureRotation; 3], String> {
    let Some(value) = value else {
        return Ok([TextureRotation::None; 3]);
    };
    if let Some(text) = value.as_str() {
        let rotation = rotation_value(path, "texture_rotation", text)?;
        return Ok([rotation; 3]);
    }
    let Some(table) = value.as_table() else {
        return Err(format!(
            "{path}: texture_rotation: expected \"none\", \"random\", or a table"
        ));
    };
    reject_unknown(
        path,
        table,
        &["all", "top", "side", "bottom"],
        "texture_rotation",
    )?;
    let all = optional_rotation(path, table, "all")?.unwrap_or(TextureRotation::None);
    let top = optional_rotation(path, table, "top")?.unwrap_or(all);
    let side = optional_rotation(path, table, "side")?.unwrap_or(all);
    let bottom = optional_rotation(path, table, "bottom")?.unwrap_or(all);
    Ok([top, side, bottom])
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
        "translucent" => Ok(RenderClass::Translucent),
        "fluid" => Ok(RenderClass::Fluid),
        "invisible" => Ok(RenderClass::Invisible),
        _ => Err(format!(
            "{path}: render: expected one of opaque, cutout, translucent, fluid, invisible"
        )),
    }
}

fn require_tint(path: &str, table: &Table) -> Result<[f32; 4], String> {
    let value = require(path, table, "tint")?;
    let Some(items) = value.as_array() else {
        return Err(format!("{path}: tint: expected four numbers in 0.0..=1.0"));
    };
    if items.len() != 4 {
        return Err(format!("{path}: tint: expected four numbers in 0.0..=1.0"));
    }
    let mut tint = [0.0; 4];
    for (index, item) in items.iter().enumerate() {
        let number = if let Some(float) = item.as_float() {
            float
        } else if let Some(integer) = item.as_integer() {
            integer as f64
        } else {
            return Err(format!("{path}: tint: expected four numbers in 0.0..=1.0"));
        };
        if !number.is_finite() || !(0.0..=1.0).contains(&number) {
            return Err(format!("{path}: tint: expected four numbers in 0.0..=1.0"));
        }
        tint[index] = number as f32;
    }
    Ok(tint)
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
    if !is_ident(parts[1]) || !is_ident(name) {
        return Err(format!(
            "{path}: namespace and name must match [a-z][a-z0-9_]{{0,31}}"
        ));
    }
    Ok(format!("{}:{name}", parts[1]))
}

/// `namespace:name`, each `[a-z][a-z0-9_]{0,31}`.
pub fn is_block_id(id: &str) -> bool {
    let Some((namespace, name)) = id.split_once(':') else {
        return false;
    };
    is_ident(namespace) && is_ident(name)
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

    fn embedded_list() -> Vec<(String, Vec<u8>)> {
        embedded_bytes()
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
        let error = parse_block_file(stone_path(), with_id.as_bytes()).unwrap_err();
        assert!(error.contains(stone_path()), "{error}");
        assert!(error.contains("id"), "{error}");
        assert!(error.contains("unknown"), "{error}");

        let missing_solid = text.replace("solid = true\n", "");
        let error = parse_block_file(stone_path(), missing_solid.as_bytes()).unwrap_err();
        assert!(error.contains("solid"), "{error}");
        assert!(error.contains("missing"), "{error}");

        let bad_render = text.replace("render = \"opaque\"", "render = \"glow\"");
        let error = parse_block_file(stone_path(), bad_render.as_bytes()).unwrap_err();
        assert!(error.contains("render"), "{error}");

        let bad_tint = text.replace(
            "tint = [0.51, 0.54, 0.56, 1.0]",
            "tint = [0.51, 1.4, 0.56, 1.0]",
        );
        let error = parse_block_file(stone_path(), bad_tint.as_bytes()).unwrap_err();
        assert!(error.contains("tint"), "{error}");

        let short_tint = text.replace(
            "tint = [0.51, 0.54, 0.56, 1.0]",
            "tint = [0.51, 0.54, 0.56]",
        );
        assert!(parse_block_file(stone_path(), short_tint.as_bytes()).is_err());

        let integer_tint = text.replace("tint = [0.51, 0.54, 0.56, 1.0]", "tint = [1, 1, 1, 1]");
        assert!(parse_block_file(stone_path(), integer_tint.as_bytes()).is_ok());

        let empty_display = text.replace("display = \"Limestone\"", "display = \"\"");
        let error = parse_block_file(stone_path(), empty_display.as_bytes()).unwrap_err();
        assert!(error.contains("display"), "{error}");

        let bad_key = text.replace("block.strata.stone", "Block.Stone");
        let error = parse_block_file(stone_path(), bad_key.as_bytes()).unwrap_err();
        assert!(error.contains("name_key"), "{error}");

        let mut huge = original.clone();
        huge.extend(std::iter::repeat_n(b'\n', MAX_FILE_BYTES));
        let error = parse_block_file(stone_path(), &huge).unwrap_err();
        assert!(error.contains("64 KiB"), "{error}");

        let mut crlf = original.clone();
        crlf.insert(0, b'\r');
        assert!(
            parse_block_file(stone_path(), &crlf)
                .unwrap_err()
                .contains("carriage")
        );
    }

    #[test]
    fn texture_rules_cover_invisible_faces_variants_and_rotation() {
        let air = fs::read("assets/strata/blocks/air.toml").unwrap();
        let with_texture =
            String::from_utf8(air).unwrap() + "\n[textures]\nall = \"strata:limestone\"\n";
        let error =
            parse_block_file("assets/strata/blocks/air.toml", with_texture.as_bytes()).unwrap_err();
        assert!(error.contains("textures"), "{error}");

        let stone = String::from_utf8(fs::read(stone_path()).unwrap()).unwrap();
        let bare = stone.replace("[textures]\nall = \"strata:limestone\"\n", "");
        let error = parse_block_file(stone_path(), bare.as_bytes()).unwrap_err();
        assert!(error.contains("textures"), "{error}");

        let bad_id = stone.replace("strata:limestone", "Strata:Limestone");
        let error = parse_block_file(stone_path(), bad_id.as_bytes()).unwrap_err();
        assert!(error.contains("textures.all"), "{error}");

        let empty_list = stone.replace("all = \"strata:limestone\"", "all = []");
        assert!(
            parse_block_file(stone_path(), empty_list.as_bytes())
                .unwrap_err()
                .contains("empty")
        );

        let variants = stone.replace(
            "all = \"strata:limestone\"",
            "all = [\"strata:limestone\", \"strata:basalt\"]",
        );
        let parsed = parse_block_file(stone_path(), variants.as_bytes()).unwrap();
        assert_eq!(
            parsed.faces.unwrap().top,
            vec!["strata:limestone".to_string(), "strata:basalt".to_string()]
        );

        let rotated = format!("{stone}\n[texture_rotation]\nside = \"random\"\n");
        let parsed = parse_block_file(stone_path(), rotated.as_bytes()).unwrap();
        assert_eq!(
            parsed.rotation,
            [
                TextureRotation::None,
                TextureRotation::Random,
                TextureRotation::None
            ]
        );
        let uniform = stone.replacen(
            "[textures]",
            "texture_rotation = \"random\"\n\n[textures]",
            1,
        );
        let parsed = parse_block_file(stone_path(), uniform.as_bytes()).unwrap();
        assert_eq!(parsed.rotation, [TextureRotation::Random; 3]);
        let bad = stone.replacen("[textures]", "texture_rotation = \"spin\"\n\n[textures]", 1);
        assert!(
            parse_block_file(stone_path(), bad.as_bytes())
                .unwrap_err()
                .contains("texture_rotation")
        );
    }

    #[test]
    fn duplicate_ids_and_broken_builtin_rules_fail_closed() {
        let stone = fs::read(stone_path()).unwrap();
        let error = assemble(vec![
            (stone_path().to_string(), stone.clone()),
            (stone_path().to_string(), stone),
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
            .position(|(path, _)| path.ends_with("water.toml"))
            .unwrap();
        let text = String::from_utf8(sources[water].1.clone()).unwrap();
        sources[water].1 = text.replace("solid = false", "solid = true").into_bytes();
        let error = assemble(sources).unwrap_err();
        assert!(error.contains("strata:water"), "{error}");
        assert!(error.contains("water.toml"), "{error}");
    }

    #[test]
    fn builtins_freeze_in_file_order_with_sorted_layers() {
        let registry = registry();
        assert!(registry.loaded_from_disk());
        assert_eq!(registry.builtin_count(), 17);
        assert_eq!(registry.get("strata:stone"), Some(STONE));
        assert_eq!(registry.get("strata:air"), Some(AIR));
        assert_eq!(registry.get("strata:missing"), Some(MISSING));
        assert_eq!(STONE.raw(), 3);
        assert_eq!(render_class(WATER), RenderClass::Fluid);
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
        assert_eq!(textures.len(), 18);
        assert_eq!(registry.texture_id(9), "strata:grass_top");
        assert_eq!(registry.texture_index("strata:missing"), Some(13));
        assert_eq!(registry.texture_index("strata:limestone"), Some(12));
        assert_ne!(registry.flags(MISSING).layers, registry.flags(STONE).layers);
        assert_eq!(registry.texture_variants(GRASS, BlockFace::Top), &[9]);
        assert_eq!(registry.texture_variants(GRASS, BlockFace::Side), &[8]);
        assert_eq!(registry.texture_variants(GRASS, BlockFace::Bottom), &[16]);
        assert_eq!(
            registry.texture_rotation(STONE, BlockFace::Side),
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
        assert_eq!(registry.flag_rows().len(), 17);
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
    fn placeholder_template_is_solid_placeable_and_uses_the_missing_texture() {
        let registry = registry();
        let unknown = BlockId(17);
        let flags = registry.flags(unknown);
        assert!(flags.solid);
        assert!(flags.placeable);
        assert_eq!(flags.render, RenderClass::Opaque);
        assert_eq!(flags.tint, registry.flags(MISSING).tint);
        assert_eq!(flags.layers, registry.flags(MISSING).layers);
        assert_eq!(
            registry.texture_variants(unknown, BlockFace::Side),
            registry.texture_variants(MISSING, BlockFace::Side)
        );
    }
}
