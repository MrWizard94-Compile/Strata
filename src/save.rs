use crate::daylight::{self, validate_time_of_day};
use crate::interaction::{self, HOTBAR_SLOTS};
use crate::registry;
use crate::world::{AIR, Block, GENERATOR_VERSION, World};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EDITS: usize = 100_000;
const COORD_LIMIT: i32 = 1_000_000;
const OLDER_GENERATOR: &str = "This world was made with an older version of the world generator and can't be opened in this version. Create a new world to play. Your old world files were left as they are.";
const DIFFERENT_VERSION: &str = "This world was saved by a different version of STRATA and can't be opened. Create a new world to play. Your old world files were left as they are.";

/// Schema written by a new save. Schema 1 still loads and migrates in memory.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSave {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ModRecord {
    id: String,
    version: String,
    hash: String,
}

/// Schema 1 document. Optional time, hotbar, and slot. Numeric block ids.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema1File {
    schema: u32,
    generator: u32,
    generation: u64,
    seed: u64,
    player: PlayerSave,
    edits: Vec<([i32; 3], u8)>,
    #[serde(default = "default_time_of_day")]
    time_of_day: f32,
    /// Placeable block ids, slot 0 first. Older saves omit this and use the default bar.
    #[serde(default = "serde_default_hotbar_ids")]
    hotbar: [u8; HOTBAR_SLOTS],
    #[serde(default)]
    selected_slot: u8,
}

/// Schema 2 document. Names, a world palette, and the mod list are required.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Schema2File {
    schema: u32,
    generator: u32,
    generation: u64,
    seed: u64,
    player: PlayerSave,
    time_of_day: f32,
    selected_slot: u8,
    hotbar: [String; HOTBAR_SLOTS],
    palette: Vec<String>,
    edits: Vec<([i32; 3], u16)>,
    mods: Vec<ModRecord>,
}

/// Loaded world. Schema 1 has already been mapped onto names. `open` does not write.
#[derive(Clone, Debug)]
pub struct Snapshot {
    generation: u64,
    pub seed: u64,
    pub player: PlayerSave,
    edits: Vec<([i32; 3], u16)>,
    time_of_day: f32,
    hotbar_names: [String; HOTBAR_SLOTS],
    selected_slot: u8,
    palette: Vec<String>,
    mods: Vec<ModRecord>,
}

fn default_time_of_day() -> f32 {
    daylight::MORNING
}

fn serde_default_hotbar_ids() -> [u8; HOTBAR_SLOTS] {
    // serde's default hook cannot return `Result`. The builtin bar is schema 1.
    default_hotbar_ids().expect("builtin hotbar fits schema 1")
}

fn default_hotbar_ids() -> Result<[u8; HOTBAR_SLOTS], String> {
    // The default bar is eight schema-1 builtins. Their raw ids are 1, 3, 6, 7,
    // 4, 9, 10 and 11. An id above 15 returns an error instead of truncating.
    let bar = interaction::default_hotbar();
    Ok([
        schema1_hotbar_id(bar[0])?,
        schema1_hotbar_id(bar[1])?,
        schema1_hotbar_id(bar[2])?,
        schema1_hotbar_id(bar[3])?,
        schema1_hotbar_id(bar[4])?,
        schema1_hotbar_id(bar[5])?,
        schema1_hotbar_id(bar[6])?,
        schema1_hotbar_id(bar[7])?,
    ])
}

fn schema1_hotbar_id(block: Block) -> Result<u8, String> {
    block
        .legacy_u8()
        .ok_or_else(|| format!("Schema 1 cannot store hotbar block {}.", block.raw()))
}

fn default_mods() -> Vec<ModRecord> {
    vec![ModRecord {
        id: "strata".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        hash: registry::registry().builtin_content_hash().to_string(),
    }]
}

fn legacy_names() -> Vec<String> {
    crate::world::legacy_palette()
}

impl Schema1File {
    fn validate(&self) -> Result<(), String> {
        if self.generator < GENERATOR_VERSION {
            return Err(OLDER_GENERATOR.into());
        }
        if self.schema != 1 || self.generator != GENERATOR_VERSION {
            return Err(DIFFERENT_VERSION.into());
        }
        validate_player(&self.player)?;
        validate_time_of_day(self.time_of_day)?;
        validate_slot(self.selected_slot)?;
        if self.edits.len() > MAX_EDITS {
            return Err(format!(
                "World exceeds the current {MAX_EDITS} edited-block limit."
            ));
        }
        for id in self.hotbar {
            match Block::from_id(id) {
                Some(block) if block.is_placeable() => {}
                Some(_) => {
                    return Err(
                        "World contains a hotbar material that cannot be placed. File preserved."
                            .into(),
                    );
                }
                None => {
                    return Err("World contains an unknown hotbar material. File preserved.".into());
                }
            }
        }
        let mut seen = HashSet::with_capacity(self.edits.len());
        for (position, id) in &self.edits {
            if position
                .iter()
                .any(|value| value.unsigned_abs() > COORD_LIMIT as u32)
                || Block::from_id(*id).is_none()
                || !seen.insert(*position)
            {
                return Err(
                    "World contains an invalid or duplicate block edit. File preserved.".into(),
                );
            }
        }
        Ok(())
    }

    fn into_snapshot(self) -> Snapshot {
        let names = legacy_names();
        let hotbar_names = self.hotbar.map(|id| {
            registry::registry()
                .name(Block::from_id(id).expect("hotbar was validated"))
                .to_string()
        });
        Snapshot {
            generation: self.generation,
            seed: self.seed,
            player: self.player,
            edits: self
                .edits
                .into_iter()
                .map(|(position, id)| (position, u16::from(id)))
                .collect(),
            time_of_day: self.time_of_day,
            hotbar_names,
            selected_slot: self.selected_slot,
            palette: names,
            mods: default_mods(),
        }
    }
}

impl Schema2File {
    fn validate(&self) -> Result<(), String> {
        if self.generator < GENERATOR_VERSION {
            return Err(OLDER_GENERATOR.into());
        }
        if self.schema != SCHEMA_VERSION || self.generator != GENERATOR_VERSION {
            return Err(DIFFERENT_VERSION.into());
        }
        validate_player(&self.player)?;
        validate_time_of_day(self.time_of_day)?;
        validate_slot(self.selected_slot)?;
        if self.edits.len() > MAX_EDITS {
            return Err(format!(
                "World exceeds the current {MAX_EDITS} edited-block limit."
            ));
        }
        validate_palette(&self.palette)?;
        for name in &self.hotbar {
            validate_hotbar_name(name)?;
        }
        let mut seen = HashSet::with_capacity(self.edits.len());
        for (position, index) in &self.edits {
            if position
                .iter()
                .any(|value| value.unsigned_abs() > COORD_LIMIT as u32)
                || usize::from(*index) >= self.palette.len()
                || !seen.insert(*position)
            {
                return Err(
                    "World contains an invalid or duplicate block edit. File preserved.".into(),
                );
            }
        }
        validate_mods(&self.mods)?;
        Ok(())
    }

    fn into_snapshot(self) -> Snapshot {
        Snapshot {
            generation: self.generation,
            seed: self.seed,
            player: self.player,
            edits: self.edits,
            time_of_day: self.time_of_day,
            hotbar_names: self.hotbar,
            selected_slot: self.selected_slot,
            palette: self.palette,
            mods: self.mods,
        }
    }
}

fn validate_player(player: &PlayerSave) -> Result<(), String> {
    if !player
        .eye
        .iter()
        .all(|value| value.is_finite() && value.abs() <= COORD_LIMIT as f32)
        || !player.yaw.is_finite()
        || !player.pitch.is_finite()
        || player.pitch.abs() > 1.55
        || player.yaw.abs() > std::f32::consts::TAU
    {
        return Err("World contains an invalid player position or orientation.".into());
    }
    Ok(())
}

fn validate_slot(slot: u8) -> Result<(), String> {
    if slot as usize >= HOTBAR_SLOTS {
        Err("World contains a hotbar selection outside its eight slots. File preserved.".into())
    } else {
        Ok(())
    }
}

/// Unknown names still need a block id above the frozen builtins. `intern_block`
/// refuses once builtins plus placeholders reach 65,535.
fn max_palette_placeholders() -> usize {
    65_535usize.saturating_sub(registry::registry().builtin_count())
}

fn validate_palette(palette: &[String]) -> Result<(), String> {
    if palette.is_empty() || palette.len() > 65_535 {
        return Err("World palette has an invalid length. File preserved.".into());
    }
    if palette[0] != "strata:air" {
        return Err("World palette must start with strata:air. File preserved.".into());
    }
    let registry = registry::registry();
    let mut seen = HashSet::with_capacity(palette.len());
    let mut unknown = 0usize;
    for name in palette {
        if !registry::is_block_id(name) || !seen.insert(name) {
            return Err(
                "World palette contains an invalid or duplicate block id. File preserved.".into(),
            );
        }
        if registry.get(name).is_none() {
            unknown += 1;
            if unknown > max_palette_placeholders() {
                return Err(crate::world::PALETTE_TOO_LARGE.into());
            }
        }
    }
    Ok(())
}

fn validate_hotbar_name(name: &str) -> Result<(), String> {
    if !registry::is_block_id(name) {
        return Err("World contains an unknown hotbar material. File preserved.".into());
    }
    if let Some(block) = registry::registry().get(name)
        && !block.is_placeable()
    {
        return Err(
            "World contains a hotbar material that cannot be placed. File preserved.".into(),
        );
    }
    Ok(())
}

fn validate_mods(mods: &[ModRecord]) -> Result<(), String> {
    if mods.is_empty() || mods.len() > 64 {
        return Err("World mod list has an invalid length. File preserved.".into());
    }
    let mut saw_strata = false;
    for record in mods {
        if !is_namespace(&record.id) {
            return Err("World mod list contains an invalid id. File preserved.".into());
        }
        if !is_version(&record.version) {
            return Err("World mod list contains an invalid version. File preserved.".into());
        }
        if !is_content_hash(&record.hash) {
            return Err("World mod list contains an invalid content hash. File preserved.".into());
        }
        if record.id == "strata" {
            saw_strata = true;
        }
    }
    if !saw_strata {
        return Err("World mod list is missing strata. File preserved.".into());
    }
    Ok(())
}

fn is_namespace(text: &str) -> bool {
    registry::is_block_id(&format!("{text}:air"))
}

fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 32
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '+' | '-')
        })
}

fn is_content_hash(text: &str) -> bool {
    text.len() == 64
        && text
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

impl Snapshot {
    #[cfg(test)]
    pub fn world(&self) -> Result<World, String> {
        self.materialize().map(|(world, _)| world)
    }

    pub fn time_of_day(&self) -> f32 {
        self.time_of_day
    }

    #[cfg(test)]
    pub fn hotbar(&self) -> Result<[Block; HOTBAR_SLOTS], String> {
        self.materialize().map(|(_, hotbar)| hotbar)
    }

    pub fn selected_slot(&self) -> usize {
        self.selected_slot as usize
    }

    pub(crate) fn materialize(&self) -> Result<(World, [Block; HOTBAR_SLOTS]), String> {
        let mut world = World::new(self.seed);
        let ids = world
            .install_palette(self.palette.clone())
            .map_err(clean_intern_error)?;
        for (position, index) in &self.edits {
            let Some(block) = ids.get(usize::from(*index)).copied() else {
                return Err(
                    "World contains an invalid or duplicate block edit. File preserved.".into(),
                );
            };
            world.set_block(*position, block);
        }
        let mut hotbar = [AIR; HOTBAR_SLOTS];
        for (slot, name) in self.hotbar_names.iter().enumerate() {
            hotbar[slot] = world.intern_block(name).map_err(clean_intern_error)?;
        }
        Ok((world, hotbar))
    }
}

fn clean_intern_error(error: String) -> String {
    if error == "block registry refuses the 65536th block" {
        crate::world::PALETTE_TOO_LARGE.to_string()
    } else if error.ends_with("File preserved.") {
        error
    } else {
        format!("{error} File preserved.")
    }
}

pub struct SaveStore {
    directory: PathBuf,
    generation: u64,
    mods: Vec<ModRecord>,
    /// Exact bytes of the schema-1 file that was loaded. Cleared after `world.v1.bak`
    /// is written, or when that file already exists. `None` for schema 2 and new worlds.
    schema1_original: Option<Vec<u8>>,
    // OS lock releases on close/crash; the on-disk marker need not be deleted.
    _lock: File,
}

impl SaveStore {
    pub fn open(directory: &Path) -> Result<(Self, Option<Snapshot>), String> {
        fs::create_dir_all(directory).map_err(|e| format!("Cannot create world directory: {e}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("world.lock"))
            .map_err(|e| format!("Cannot open world lock: {e}"))?;
        lock.try_lock().map_err(|e| {
            format!("Cannot lock world. Another STRATA client may have it open: {e}")
        })?;
        let mut newest: Option<Snapshot> = None;
        let mut schema1_original: Option<Vec<u8>> = None;
        for name in ["world-a.json", "world-b.json"] {
            let path = directory.join(name);
            match File::open(&path) {
                Ok(file) => {
                    let mut data = Vec::new();
                    file.take(MAX_BYTES + 1)
                        .read_to_end(&mut data)
                        .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
                    if data.len() as u64 > MAX_BYTES {
                        return Err(format!(
                            "{} exceeds the save size limit. File preserved.",
                            path.display()
                        ));
                    }
                    let (snapshot, schema) = decode_save(&path, &data)?;
                    if newest
                        .as_ref()
                        .is_none_or(|old| snapshot.generation > old.generation)
                    {
                        schema1_original = if schema == 1 { Some(data) } else { None };
                        newest = Some(snapshot);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Cannot open {}: {e}", path.display())),
            }
        }
        let generation = newest.as_ref().map_or(0, |save| save.generation);
        let mods = newest
            .as_ref()
            .map(|save| save.mods.clone())
            .unwrap_or_else(default_mods);
        Ok((
            Self {
                directory: directory.to_owned(),
                generation,
                mods,
                schema1_original,
                _lock: lock,
            },
            newest,
        ))
    }

    pub fn save(
        &mut self,
        world: &World,
        player: PlayerSave,
        time_of_day: f32,
        hotbar: [Block; HOTBAR_SLOTS],
        selected_slot: u8,
    ) -> Result<(), String> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("World generation counter exhausted")?;
        let (palette, edits) = world.saved_palette_and_edits()?;
        // The record describes the content that is writing this file.
        let mods = default_mods();
        let file = Schema2File {
            schema: SCHEMA_VERSION,
            generator: GENERATOR_VERSION,
            generation,
            seed: world.seed(),
            player,
            time_of_day,
            selected_slot,
            hotbar: hotbar.map(|block| world.block_key(block).to_string()),
            palette,
            edits,
            mods,
        };
        file.validate()?;
        let bytes = serde_json::to_vec(&file).map_err(|e| format!("Cannot encode world: {e}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Encoded world exceeds save size limit".into());
        }
        self.write_schema1_backup()?;
        fs::create_dir_all(&self.directory)
            .map_err(|e| format!("Cannot create save directory: {e}"))?;
        let name = if generation.is_multiple_of(2) {
            "world-a.json"
        } else {
            "world-b.json"
        };
        let target = self.directory.join(name);
        let temporary = self.directory.join(format!("{name}.pending"));
        let mut staged = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| format!("Cannot stage save: {e}"))?;
        staged
            .write_all(&bytes)
            .and_then(|()| staged.sync_all())
            .map_err(|e| format!("Cannot flush save: {e}"))?;
        drop(staged);
        // The opposite slot remains the last committed generation throughout replacement.
        match fs::remove_file(&target) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot replace inactive save slot: {e}")),
        }
        fs::rename(&temporary, &target)
            .map_err(|e| format!("Cannot commit save; previous slot retained: {e}"))?;
        self.generation = generation;
        self.mods = default_mods();
        Ok(())
    }

    /// One-time copy of the loaded schema-1 bytes. An existing backup is left alone.
    /// A failure leaves the obligation in place and must stop the schema-2 write.
    fn write_schema1_backup(&mut self) -> Result<(), String> {
        let Some(original) = self.schema1_original.as_deref() else {
            return Ok(());
        };
        let target = self.directory.join("world.v1.bak");
        if target.exists() {
            self.schema1_original = None;
            return Ok(());
        }
        let temporary = self.directory.join("world.v1.bak.pending");
        let mut staged = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| format!("Cannot stage world.v1.bak: {e}"))?;
        staged
            .write_all(original)
            .and_then(|()| staged.sync_all())
            .map_err(|e| format!("Cannot flush world.v1.bak: {e}"))?;
        drop(staged);
        fs::rename(&temporary, &target).map_err(|e| format!("Cannot commit world.v1.bak: {e}"))?;
        self.schema1_original = None;
        Ok(())
    }
}

fn decode_save(path: &Path, bytes: &[u8]) -> Result<(Snapshot, u32), String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| {
        format!(
            "Cannot decode {}: {error}. File preserved; restore a backup to continue.",
            path.display()
        )
    })?;
    let generator = value.get("generator").and_then(serde_json::Value::as_u64);
    let schema = value.get("schema").and_then(serde_json::Value::as_u64);
    match generator {
        Some(value) if value < u64::from(GENERATOR_VERSION) => return Err(OLDER_GENERATOR.into()),
        Some(value) if value != u64::from(GENERATOR_VERSION) => {
            return Err(DIFFERENT_VERSION.into());
        }
        None => return Err(DIFFERENT_VERSION.into()),
        Some(_) => {}
    }
    match schema {
        Some(1) => {
            let file: Schema1File = serde_json::from_value(value).map_err(|error| {
                format!(
                    "Cannot decode {}: {error}. File preserved; restore a backup to continue.",
                    path.display()
                )
            })?;
            file.validate()?;
            Ok((file.into_snapshot(), 1))
        }
        Some(2) => {
            let file: Schema2File = serde_json::from_value(value).map_err(|error| {
                format!(
                    "Cannot decode {}: {error}. File preserved; restore a backup to continue.",
                    path.display()
                )
            })?;
            file.validate()?;
            Ok((file.into_snapshot(), 2))
        }
        _ => Err(DIFFERENT_VERSION.into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{AIR, COAL_ORE, CRYSTAL, DIAMOND_ORE, DIRT, GRASS, STONE, WATER};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    fn directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "strata-save-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn player() -> PlayerSave {
        PlayerSave {
            eye: [0.0, 80.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            flying: false,
        }
    }
    fn save_world(
        store: &mut SaveStore,
        world: &World,
        player: PlayerSave,
        time_of_day: f32,
        hotbar: [Block; HOTBAR_SLOTS],
        selected_slot: u8,
    ) {
        store
            .save(world, player, time_of_day, hotbar, selected_slot)
            .unwrap();
    }
    #[test]
    fn roundtrip_retains_seed_negative_coordinate_edits_and_player() {
        let path = directory();
        let (mut store, initial) = SaveStore::open(&path).unwrap();
        assert!(initial.is_none());
        let mut world = World::new(42);
        world.set_block([-3, 90, 2], CRYSTAL);
        save_world(
            &mut store,
            &world,
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.player.eye, player().eye);
        assert_eq!(loaded.world().unwrap().block_at(-3, 90, 2), CRYSTAL);
        assert_eq!(loaded.hotbar().unwrap(), interaction::default_hotbar());
        assert_eq!(loaded.selected_slot(), 0);
        assert_eq!(loaded.time_of_day(), daylight::MORNING);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn schema2_save_rejects_an_unplaceable_builtin_on_the_hotbar() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let mut hotbar = interaction::default_hotbar();
        hotbar[0] = crate::registry::MISSING;
        let error = store
            .save(&World::new(1), player(), daylight::MORNING, hotbar, 0)
            .unwrap_err();
        assert!(
            error.contains("cannot be placed") && error.contains("File preserved."),
            "{error}"
        );
        assert!(!path.join("world-a.json").exists());
        assert!(!path.join("world-b.json").exists());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn schema1_hotbar_id_above_15_is_rejected_and_the_file_is_preserved() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = br#"{"schema":1,"generator":3,"generation":1,"seed":7,"player":{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false},"hotbar":[16,1,3,6,7,4,9,10],"edits":[]}"#;
        fs::write(path.join("world-a.json"), body).unwrap();
        let error = match SaveStore::open(&path) {
            Err(error) => error,
            Ok(_) => panic!("schema 1 hotbar id 16 was accepted"),
        };
        assert!(
            error.contains("unknown hotbar") && error.contains("File preserved."),
            "{error}"
        );
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn interrupted_pending_save_does_not_replace_committed_generation() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let mut world = World::new(9);
        save_world(
            &mut store,
            &world,
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        world.set_block([0, 110, 0], STONE);
        save_world(
            &mut store,
            &world,
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        fs::write(path.join("world-b.json.pending"), b"{incomplete").unwrap();
        drop(store);
        let (_, loaded) = SaveStore::open(&path).unwrap();
        assert_eq!(loaded.unwrap().generation, 2);
        assert!(path.join("world-a.json").exists());
        assert!(path.join("world-b.json").exists());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn corrupt_slot_is_reported_and_never_silently_replaced() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("world-a.json"), b"broken").unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), b"broken");
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn malformed_versions_positions_and_duplicate_edits_are_rejected() {
        let mut snapshot = Schema1File {
            schema: 9,
            generator: GENERATOR_VERSION,
            generation: 1,
            seed: 0,
            player: player(),
            edits: vec![],
            time_of_day: daylight::MORNING,
            hotbar: default_hotbar_ids().unwrap(),
            selected_slot: 0,
        };
        assert!(snapshot.validate().is_err());
        snapshot.schema = 1;
        snapshot.generator = GENERATOR_VERSION.saturating_sub(1);
        assert!(snapshot.validate().is_err());
        snapshot.generator = GENERATOR_VERSION + 1;
        assert!(snapshot.validate().is_err());
        snapshot.generator = 2;
        assert!(snapshot.validate().is_err());
        snapshot.generator = GENERATOR_VERSION;
        snapshot.player.eye[0] = f32::NAN;
        assert!(snapshot.validate().is_err());
        snapshot.player = player();
        snapshot.edits = vec![([1, 2, 3], 1), ([1, 2, 3], 2)];
        assert!(snapshot.validate().is_err());
        snapshot.edits = vec![([1, 2, 3], 255)];
        assert!(snapshot.validate().is_err());
        snapshot.edits.clear();
        snapshot.time_of_day = f32::NAN;
        assert!(snapshot.validate().is_err());
        snapshot.time_of_day = f32::INFINITY;
        assert!(snapshot.validate().is_err());
        snapshot.time_of_day = -0.01;
        assert!(snapshot.validate().is_err());
        snapshot.time_of_day = 1.0;
        assert!(snapshot.validate().is_err());
        snapshot.time_of_day = 0.0;
        assert!(snapshot.validate().is_ok());
        snapshot.hotbar[0] = 255;
        assert!(snapshot.validate().is_err());
        snapshot.hotbar = default_hotbar_ids().unwrap();
        snapshot.hotbar[0] = WATER.legacy_u8().unwrap();
        assert!(snapshot.validate().is_err());
        snapshot.hotbar = default_hotbar_ids().unwrap();
        snapshot.selected_slot = HOTBAR_SLOTS as u8;
        assert!(snapshot.validate().is_err());
        snapshot.selected_slot = 0;
        assert!(snapshot.validate().is_ok());
    }

    #[test]
    fn time_hotbar_and_slot_round_trip_together() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let mut world = World::new(11);
        world.set_block([2, 40, -4], STONE);
        let mut slots = interaction::default_hotbar();
        slots[2] = DIRT;
        slots[7] = COAL_ORE;
        save_world(&mut store, &world, player(), 0.75, slots, 6);
        drop(store);
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(
            daylight::DayClock::from_fraction(loaded.time_of_day())
                .unwrap()
                .tick(),
            daylight::TICKS_PER_DAY * 3 / 4
        );
        assert_eq!(loaded.selected_slot(), 6);
        assert_eq!(loaded.hotbar().unwrap()[2], DIRT);
        assert_eq!(loaded.hotbar().unwrap()[7], COAL_ORE);
        assert_eq!(loaded.player.eye, player().eye);
        assert_eq!(loaded.world().unwrap().block_at(2, 40, -4), STONE);
        let slot = path.join("world-b.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        assert_eq!(value["schema"], serde_json::json!(SCHEMA_VERSION));
        assert_eq!(value["generator"], serde_json::json!(GENERATOR_VERSION));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn save_without_time_hotbar_or_slot_loads_defaults() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = br#"{"schema":1,"generator":3,"generation":1,"seed":42,"player":{"eye":[1.0,80.0,2.0],"yaw":0.2,"pitch":-0.1,"flying":true},"edits":[[[4,70,-2],2]]}"#;
        fs::write(path.join("world-a.json"), body).unwrap();
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.time_of_day(), daylight::MORNING);
        assert_eq!(
            daylight::DayClock::from_fraction(loaded.time_of_day())
                .unwrap()
                .tick(),
            daylight::DayClock::morning().tick()
        );
        assert_eq!(loaded.hotbar().unwrap(), interaction::default_hotbar());
        assert_eq!(loaded.selected_slot(), 0);
        assert_eq!(loaded.seed, 42);
        assert!(loaded.player.flying);
        assert_eq!(loaded.player.eye, [1.0, 80.0, 2.0]);
        assert_eq!(loaded.world().unwrap().block_at(4, 70, -2), DIRT);
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn invalid_time_of_day_is_rejected_and_the_file_is_preserved() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(3),
            player(),
            0.5,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        let slot = path.join("world-b.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        value["time_of_day"] = serde_json::json!(4.0);
        let tampered = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &tampered).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), tampered);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn null_negative_and_exact_one_time_of_day_are_rejected_without_overwrite() {
        for bad in [
            serde_json::Value::Null,
            serde_json::json!(-0.25),
            serde_json::json!(1.0),
        ] {
            let path = directory();
            let (mut store, _) = SaveStore::open(&path).unwrap();
            save_world(
                &mut store,
                &World::new(3),
                player(),
                0.5,
                interaction::default_hotbar(),
                0,
            );
            drop(store);
            let slot = path.join("world-b.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
            value["time_of_day"] = bad;
            let tampered = serde_json::to_vec_pretty(&value).unwrap();
            fs::write(&slot, &tampered).unwrap();
            assert!(SaveStore::open(&path).is_err(), "{tampered:?}");
            assert_eq!(fs::read(&slot).unwrap(), tampered);
            fs::remove_dir_all(path).unwrap();
        }
    }

    /// Minimal schema-1 JSON. Optional fields (time of day, and any later
    /// optional hotbar) stay absent so a sibling change can add them with
    /// defaults without this rejection depending on those fields.
    fn rejected_generator_file_is_unchanged(generator: u32, message: &str) {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = format!(
            r#"{{"schema":1,"generator":{generator},"generation":1,"seed":7,"player":{{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false}},"edits":[]}}"#
        );
        fs::write(path.join("world-a.json"), &body).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("generator {generator} save was accepted");
        };
        assert_eq!(error, message);
        assert_eq!(
            fs::read(path.join("world-a.json")).unwrap(),
            body.as_bytes()
        );
        assert!(!path.join("world-a.json.pending").exists());
        assert!(!path.join("world-b.json").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn generator_2_save_is_rejected_and_preserved() {
        rejected_generator_file_is_unchanged(
            2,
            "This world was made with an older version of the world generator and can't be opened in this version. Create a new world to play. Your old world files were left as they are.",
        );
    }

    #[test]
    fn generator_4_save_is_rejected_and_preserved() {
        rejected_generator_file_is_unchanged(
            4,
            "This world was saved by a different version of STRATA and can't be opened. Create a new world to play. Your old world files were left as they are.",
        );
    }

    #[test]
    fn invalid_hotbar_is_rejected_and_the_file_is_preserved() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(3),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            1,
        );
        drop(store);
        let slot = path.join("world-b.json");
        let original = fs::read(&slot).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
        value["hotbar"][0] = serde_json::json!(255);
        let unknown = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &unknown).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), unknown);

        value["hotbar"][0] = serde_json::json!(GRASS.legacy_u8().unwrap());
        value["selected_slot"] = serde_json::json!(HOTBAR_SLOTS);
        let out_of_range = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &out_of_range).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), out_of_range);

        value["selected_slot"] = serde_json::json!(1);
        value["hotbar"] = serde_json::json!([1, 2, 3]);
        let short_bar = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &short_bar).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), short_bar);

        value["hotbar"] = serde_json::Value::Null;
        let null_bar = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &null_bar).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), null_bar);

        value["hotbar"] = serde_json::json!(default_hotbar_ids().unwrap());
        value["selected_slot"] = serde_json::Value::Null;
        let null_slot = serde_json::to_vec_pretty(&value).unwrap();
        fs::write(&slot, &null_slot).unwrap();
        assert!(SaveStore::open(&path).is_err());
        assert_eq!(fs::read(&slot).unwrap(), null_slot);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn newer_generator_save_is_rejected_and_preserved() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(3),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        let slot = path.join("world-b.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        assert_eq!(value["generator"], serde_json::json!(GENERATOR_VERSION));
        value["generator"] = serde_json::json!(GENERATOR_VERSION + 1);
        let tampered = serde_json::to_vec(&value).unwrap();
        fs::write(&slot, &tampered).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("a newer generator save was accepted");
        };
        assert!(error.contains("different version"), "{error}");
        assert!(error.contains("can't be opened"), "{error}");
        assert_eq!(fs::read(&slot).unwrap(), tampered);
        assert!(!path.join("world-b.json.pending").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn generator_1_save_is_rejected_and_preserved() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = br#"{"schema":1,"generator":1,"generation":1,"seed":7,"player":{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false},"edits":[]}"#;
        fs::write(path.join("world-a.json"), body).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("generator 1 save was accepted");
        };
        assert!(
            error.contains("older version of the world generator"),
            "{error}"
        );
        assert!(error.contains("can't be opened"), "{error}");
        assert!(error.contains("Create a new world"), "{error}");
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        assert!(!path.join("world-a.json.pending").exists());
        assert!(!path.join("world-b.json").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unknown_field_is_rejected_and_the_file_is_preserved() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(3),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        let slot = path.join("world-b.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        value["not_a_save_field"] = serde_json::json!(true);
        let tampered = serde_json::to_vec(&value).unwrap();
        fs::write(&slot, &tampered).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("unknown field was accepted");
        };
        assert!(error.contains("unknown field"), "{error}");
        assert_eq!(fs::read(&slot).unwrap(), tampered);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn second_writer_is_rejected_and_close_releases_world_lock() {
        let path = directory();
        let (first, _) = SaveStore::open(&path).unwrap();
        assert!(SaveStore::open(&path).is_err());
        drop(first);
        let (second, _) = SaveStore::open(&path).unwrap();
        drop(second);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn schema1_stone_edit_loads_without_rewriting_the_file() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = br#"{"schema":1,"generator":3,"generation":1,"seed":7,"player":{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false},"edits":[[[1,2,3],3]]}"#;
        fs::write(path.join("world-a.json"), body).unwrap();
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.world().unwrap().block_at(1, 2, 3), STONE);
        assert_eq!(loaded.world().unwrap().block_key(STONE), "strata:stone");
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        assert!(!path.join("world-b.json").exists());
        assert!(!path.join("world-a.json.pending").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unknown_palette_name_round_trips_instead_of_becoming_air_or_missing() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let hash = "ab".repeat(32);
        let body = format!(
            r#"{{"schema":2,"generator":3,"generation":1,"seed":7,"player":{{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false}},"time_of_day":0.25,"selected_slot":0,"hotbar":["strata:grass","strata:stone","strata:wood","strata:leaves","strata:sand","strata:clay","strata:crystal","example:gone"],"palette":["strata:air","strata:grass","strata:dirt","strata:stone","strata:sand","strata:water","strata:wood","strata:leaves","strata:snow","strata:clay","strata:crystal","strata:basalt","strata:coal_ore","strata:iron_ore","strata:gold_ore","strata:diamond_ore","example:gone"],"edits":[[[4,80,5],16]],"mods":[{{"id":"strata","version":"0.1.0","hash":"{hash}"}}]}}"#
        );
        fs::write(path.join("world-a.json"), &body).unwrap();
        let (mut store, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        let world = loaded.world().unwrap();
        let block = world.block_at(4, 80, 5);
        assert_ne!(block, AIR);
        assert_ne!(block, STONE);
        assert!(block.is_solid());
        assert_eq!(world.block_key(block), "example:gone");
        assert_eq!(world.display_name(block), "example:gone");
        let hotbar = loaded.hotbar().unwrap();
        assert_eq!(world.block_key(hotbar[7]), "example:gone");
        assert!(hotbar[7].is_placeable());
        assert!(!crate::registry::MISSING.is_placeable());
        save_world(
            &mut store,
            &world,
            loaded.player.clone(),
            loaded.time_of_day(),
            hotbar,
            0,
        );
        drop(store);
        let saved = fs::read_dir(&path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|file| {
                file.file_name()
                    .is_some_and(|name| name.to_string_lossy().ends_with(".json"))
                    && fs::read_to_string(file)
                        .unwrap()
                        .contains("\"generation\":2")
            })
            .expect("schema 2 save");
        let saved = fs::read_to_string(saved).unwrap();
        assert!(saved.contains("example:gone"), "{saved}");
        assert!(!saved.contains("\"strata:missing\""));
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(value["palette"][16], "example:gone");
        assert_eq!(value["edits"][0][1], serde_json::json!(16));
        assert_eq!(value["hotbar"][7], "example:gone");
        let (_, again) = SaveStore::open(&path).unwrap();
        let again = again.unwrap();
        assert_eq!(
            again
                .world()
                .unwrap()
                .block_key(again.world().unwrap().block_at(4, 80, 5)),
            "example:gone"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn schema2_generator_2_and_4_keep_the_rejection_sentences_and_bytes() {
        for (generator, message) in [
            (
                2,
                "This world was made with an older version of the world generator and can't be opened in this version. Create a new world to play. Your old world files were left as they are.",
            ),
            (
                4,
                "This world was saved by a different version of STRATA and can't be opened. Create a new world to play. Your old world files were left as they are.",
            ),
        ] {
            let path = directory();
            fs::create_dir_all(&path).unwrap();
            let body = format!(
                r#"{{"schema":2,"generator":{generator},"generation":1,"seed":7,"player":{{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false}},"time_of_day":0.0,"selected_slot":0,"hotbar":["strata:grass","strata:stone","strata:wood","strata:leaves","strata:sand","strata:clay","strata:crystal","strata:basalt"],"palette":["strata:air"],"edits":[],"mods":[{{"id":"strata","version":"0.1.0","hash":"{}"}}]}}"#,
                "ab".repeat(32)
            );
            fs::write(path.join("world-a.json"), &body).unwrap();
            let Err(error) = SaveStore::open(&path) else {
                panic!("schema 2 generator {generator} was accepted");
            };
            assert_eq!(error, message);
            assert_eq!(
                fs::read(path.join("world-a.json")).unwrap(),
                body.as_bytes()
            );
            assert!(!path.join("world-b.json").exists());
            assert!(!path.join("world-a.json.pending").exists());
            fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn schema1_with_palette_and_schema2_without_palette_are_rejected() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let with_palette = br#"{"schema":1,"generator":3,"generation":1,"seed":7,"player":{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false},"edits":[],"palette":["strata:air"]}"#;
        fs::write(path.join("world-a.json"), with_palette).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("schema 1 palette was accepted");
        };
        assert!(error.contains("unknown field"), "{error}");
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), with_palette);
        fs::remove_file(path.join("world-a.json")).unwrap();

        let missing_palette = br#"{"schema":2,"generator":3,"generation":2,"seed":7,"player":{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false},"time_of_day":0.0,"selected_slot":0,"hotbar":["strata:grass","strata:stone","strata:wood","strata:leaves","strata:sand","strata:clay","strata:crystal","strata:basalt"],"edits":[],"mods":[{"id":"strata","version":"0.1.0","hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        fs::write(path.join("world-b.json"), missing_palette).unwrap();
        let Err(error) = SaveStore::open(&path) else {
            panic!("schema 2 without palette was accepted");
        };
        assert!(error.contains("missing field"), "{error}");
        assert_eq!(
            fs::read(path.join("world-b.json")).unwrap(),
            missing_palette
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_hash_mismatch_still_loads_and_the_next_save_records_this_build() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = format!(
            r#"{{"schema":2,"generator":3,"generation":1,"seed":7,"player":{{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false}},"time_of_day":0.0,"selected_slot":0,"hotbar":["strata:grass","strata:stone","strata:wood","strata:leaves","strata:sand","strata:clay","strata:crystal","strata:basalt"],"palette":["strata:air","strata:grass","strata:dirt","strata:stone","strata:sand","strata:water","strata:wood","strata:leaves","strata:snow","strata:clay","strata:crystal","strata:basalt","strata:coal_ore","strata:iron_ore","strata:gold_ore","strata:diamond_ore"],"edits":[],"mods":[{{"id":"strata","version":"0.1.0","hash":"{}"}}]}}"#,
            "ab".repeat(32)
        );
        fs::write(path.join("world-a.json"), &body).unwrap();
        let (mut store, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        save_world(
            &mut store,
            &loaded.world().unwrap(),
            loaded.player.clone(),
            loaded.time_of_day(),
            loaded.hotbar().unwrap(),
            0,
        );
        drop(store);
        let saved = fs::read_dir(&path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|file| {
                file.extension().is_some_and(|ext| ext == "json")
                    && fs::read_to_string(file)
                        .unwrap()
                        .contains("\"generation\":2")
            })
            .expect("rewritten save");
        let saved = fs::read_to_string(saved).unwrap();
        let value: serde_json::Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            value["mods"][0]["id"],
            serde_json::json!("strata"),
            "{saved}"
        );
        assert_eq!(
            value["mods"][0]["version"],
            serde_json::json!(env!("CARGO_PKG_VERSION")),
            "{saved}"
        );
        assert_eq!(
            value["mods"][0]["hash"],
            serde_json::json!(registry::registry().builtin_content_hash()),
            "{saved}"
        );
        assert!(!saved.contains(&"ab".repeat(32)), "{saved}");
        fs::remove_dir_all(path).unwrap();
    }

    fn schema1_bytes(generation: u64) -> Vec<u8> {
        format!(
            r#"{{"schema":1,"generator":3,"generation":{generation},"seed":7,"player":{{"eye":[0.0,80.0,0.0],"yaw":0.0,"pitch":0.0,"flying":false}},"edits":[[[1,2,3],3]]}}"#
        )
        .into_bytes()
    }

    fn refuse_open(path: &Path) -> String {
        match SaveStore::open(path) {
            Err(error) => error,
            Ok(_) => panic!("open succeeded for {}", path.display()),
        }
    }

    fn hotbar_json() -> serde_json::Value {
        serde_json::json!([
            "strata:grass",
            "strata:stone",
            "strata:wood",
            "strata:leaves",
            "strata:sand",
            "strata:clay",
            "strata:crystal",
            "strata:basalt"
        ])
    }

    fn schema2_document(palette: Vec<String>, edits: serde_json::Value) -> Vec<u8> {
        let value = serde_json::json!({
            "schema": 2,
            "generator": 3,
            "generation": 1,
            "seed": 7,
            "player": {"eye": [0.0, 80.0, 0.0], "yaw": 0.0, "pitch": 0.0, "flying": false},
            "time_of_day": 0.0,
            "selected_slot": 0,
            "hotbar": hotbar_json(),
            "palette": palette,
            "edits": edits,
            "mods": [{"id": "strata", "version": "0.1.0", "hash": "ab".repeat(32)}]
        });
        serde_json::to_vec(&value).unwrap()
    }

    fn palette_with_unknowns(unknown: usize) -> Vec<String> {
        let mut names = Vec::with_capacity(unknown + 1);
        names.push("strata:air".to_string());
        for index in 0..unknown {
            names.push(format!("m:b{index:05}"));
        }
        names
    }

    #[test]
    fn schema1_in_its_parity_slot_is_backed_up_once_before_the_second_save() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(2);
        fs::write(path.join("world-a.json"), &original).unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert_eq!(fs::read(path.join("world.v1.bak")).unwrap(), original);
        let upgraded: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("world-a.json")).unwrap()).unwrap();
        assert_eq!(upgraded["schema"], 2);
        let other: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("world-b.json")).unwrap()).unwrap();
        assert_eq!(other["schema"], 2);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn schema1_in_the_wrong_parity_slot_is_backed_up_before_the_first_save() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(5);
        fs::write(path.join("world-a.json"), &original).unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert_eq!(fs::read(path.join("world.v1.bak")).unwrap(), original);
        let replaced: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("world-a.json")).unwrap()).unwrap();
        assert_eq!(replaced["schema"], 2);
        assert_eq!(replaced["generation"], 6);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn repeated_saves_and_a_reopen_leave_the_schema1_backup_unchanged() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(2);
        fs::write(path.join("world-a.json"), &original).unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        let backup = path.join("world.v1.bak");
        let modified = fs::metadata(&backup).unwrap().modified().unwrap();
        for _ in 0..6 {
            save_world(
                &mut store,
                &World::new(7),
                player(),
                daylight::MORNING,
                interaction::default_hotbar(),
                0,
            );
        }
        drop(store);
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        assert_eq!(fs::read(&backup).unwrap(), original);
        assert_eq!(fs::metadata(&backup).unwrap().modified().unwrap(), modified);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_fresh_world_and_a_schema2_world_do_not_create_a_backup() {
        let fresh = directory();
        let (mut store, initial) = SaveStore::open(&fresh).unwrap();
        assert!(initial.is_none());
        save_world(
            &mut store,
            &World::new(3),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert!(!fresh.join("world.v1.bak").exists());
        drop(store);
        fs::remove_dir_all(fresh).unwrap();

        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = schema2_document(palette_with_unknowns(0), serde_json::json!([]));
        fs::write(path.join("world-a.json"), body).unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert!(!path.join("world.v1.bak").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn an_existing_schema1_backup_is_left_unchanged() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(5);
        fs::write(path.join("world-a.json"), &original).unwrap();
        fs::write(path.join("world.v1.bak"), b"keep-me").unwrap();
        let modified = fs::metadata(path.join("world.v1.bak"))
            .unwrap()
            .modified()
            .unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert_eq!(fs::read(path.join("world.v1.bak")).unwrap(), b"keep-me");
        assert_eq!(
            fs::metadata(path.join("world.v1.bak"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn a_failed_schema1_backup_blocks_the_schema2_write_until_the_next_save() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(2);
        fs::write(path.join("world-a.json"), &original).unwrap();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        fs::create_dir_all(path.join("world.v1.bak.pending")).unwrap();
        let error = store
            .save(
                &World::new(7),
                player(),
                daylight::MORNING,
                interaction::default_hotbar(),
                0,
            )
            .unwrap_err();
        assert!(error.contains("world.v1.bak"), "{error}");
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), original);
        assert!(!path.join("world-b.json").exists());
        assert!(!path.join("world.v1.bak").exists());
        fs::remove_dir_all(path.join("world.v1.bak.pending")).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        assert_eq!(fs::read(path.join("world.v1.bak")).unwrap(), original);
        assert!(path.join("world-b.json").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn copying_the_schema1_backup_back_loads_the_original_world() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let original = schema1_bytes(2);
        fs::write(path.join("world-a.json"), &original).unwrap();
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.world().unwrap().block_at(1, 2, 3), STONE);
        drop(loaded);
        let (mut store, _) = SaveStore::open(&path).unwrap();
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        save_world(
            &mut store,
            &World::new(7),
            player(),
            daylight::MORNING,
            interaction::default_hotbar(),
            0,
        );
        drop(store);
        fs::remove_file(path.join("world-a.json")).unwrap();
        fs::remove_file(path.join("world-b.json")).unwrap();
        fs::copy(path.join("world.v1.bak"), path.join("world-a.json")).unwrap();
        let (_, restored) = SaveStore::open(&path).unwrap();
        let restored = restored.unwrap();
        assert_eq!(restored.seed, 7);
        assert_eq!(restored.world().unwrap().block_at(1, 2, 3), STONE);
        assert_eq!(restored.player.eye, [0.0, 80.0, 0.0]);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn palette_size_follows_the_builtin_count_and_oversized_files_stay_put() {
        let builtin = registry::registry().builtin_count();
        let max_unknown = 65_535 - builtin;
        let largest_names = 1 + max_unknown;
        assert!(
            largest_names < 65_520,
            "builtins={builtin} largest={largest_names}"
        );
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = schema2_document(palette_with_unknowns(max_unknown), serde_json::json!([]));
        fs::write(path.join("world-a.json"), &body).unwrap();
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let _world = loaded.unwrap().world().unwrap();
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        fs::remove_dir_all(&path).unwrap();

        for names in [largest_names + 1, 65_520, 65_535] {
            let unknown = names - 1;
            let rejected = directory();
            fs::create_dir_all(&rejected).unwrap();
            let body = schema2_document(palette_with_unknowns(unknown), serde_json::json!([]));
            fs::write(rejected.join("world-a.json"), &body).unwrap();
            let error = refuse_open(&rejected);
            assert!(
                error.contains(crate::world::PALETTE_TOO_LARGE),
                "names={names} builtins={builtin} error={error}"
            );
            assert_eq!(fs::read(rejected.join("world-a.json")).unwrap(), body);
            fs::remove_dir_all(rejected).unwrap();
        }
    }

    #[test]
    fn materialize_returns_the_palette_error_instead_of_panicking() {
        let names = palette_with_unknowns(65_535);
        let snapshot = Snapshot {
            generation: 1,
            seed: 7,
            player: player(),
            edits: Vec::new(),
            time_of_day: 0.0,
            hotbar_names: std::array::from_fn(|_| "strata:grass".to_string()),
            selected_slot: 0,
            palette: names,
            mods: default_mods(),
        };
        let error = match snapshot.materialize() {
            Err(error) => error,
            Ok(_) => panic!("oversized palette materialized"),
        };
        assert_eq!(error, crate::world::PALETTE_TOO_LARGE);
    }

    #[test]
    fn palette_order_is_independent_of_builtin_order() {
        let names = vec![
            "strata:air".to_string(),
            "strata:diamond_ore".to_string(),
            "strata:stone".to_string(),
            "strata:water".to_string(),
            "example:x".to_string(),
        ];
        // y=200 is above the generator's air ceiling, so terrain cannot imitate the edit.
        let edits = serde_json::json!([
            [[1, 200, 1], 1],
            [[2, 200, 2], 2],
            [[3, 200, 3], 3],
            [[4, 200, 4], 4]
        ]);
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("world-a.json"),
            schema2_document(names.clone(), edits),
        )
        .unwrap();
        let (mut store, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        let world = loaded.world().unwrap();
        assert_eq!(world.block_at(1, 200, 1), DIAMOND_ORE);
        assert_eq!(world.block_at(2, 200, 2), STONE);
        assert_eq!(world.block_at(3, 200, 3), WATER);
        assert_eq!(world.block_key(world.block_at(4, 200, 4)), "example:x");
        assert!(world.block_at(4, 200, 4).is_solid());
        assert_ne!(world.block_at(4, 200, 4), AIR);
        save_world(
            &mut store,
            &world,
            loaded.player.clone(),
            loaded.time_of_day(),
            loaded.hotbar().unwrap(),
            0,
        );
        drop(store);
        let saved = fs::read_dir(&path)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|file| {
                file.extension().is_some_and(|ext| ext == "json")
                    && serde_json::from_slice::<serde_json::Value>(&fs::read(file).unwrap())
                        .ok()
                        .is_some_and(|value| value["generation"] == 2)
            })
            .expect("resave");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(saved).unwrap()).unwrap();
        let saved_names: Vec<String> = value["palette"]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect();
        assert_eq!(saved_names, names);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unknown_schema_values_keep_the_different_version_message_and_the_bytes() {
        let samples = [
            ("3", Some(serde_json::json!(3))),
            ("99", Some(serde_json::json!(99))),
            ("0", Some(serde_json::json!(0))),
            ("string", Some(serde_json::json!("2"))),
            ("null", Some(serde_json::Value::Null)),
            ("float", Some(serde_json::json!(2.0))),
            ("missing", None),
        ];
        for (label, schema) in samples {
            let path = directory();
            fs::create_dir_all(&path).unwrap();
            let mut value = serde_json::json!({
                "schema": 1,
                "generator": 3,
                "generation": 1,
                "seed": 7,
                "player": {"eye": [0.0, 80.0, 0.0], "yaw": 0.0, "pitch": 0.0, "flying": false},
                "edits": []
            });
            match schema {
                Some(schema) => value["schema"] = schema,
                None => {
                    value.as_object_mut().unwrap().remove("schema");
                }
            }
            let body = serde_json::to_vec(&value).unwrap();
            fs::write(path.join("world-a.json"), &body).unwrap();
            let error = refuse_open(&path);
            assert_eq!(error, DIFFERENT_VERSION, "{label}");
            assert_eq!(
                fs::read(path.join("world-a.json")).unwrap(),
                body,
                "{label}"
            );
            fs::remove_dir_all(path).unwrap();
        }
    }

    #[test]
    fn duplicate_palette_names_are_rejected_and_the_file_is_untouched() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = schema2_document(
            vec![
                "strata:air".to_string(),
                "strata:stone".to_string(),
                "strata:stone".to_string(),
            ],
            serde_json::json!([]),
        );
        fs::write(path.join("world-a.json"), &body).unwrap();
        let error = refuse_open(&path);
        assert!(
            error.contains("invalid or duplicate block id") && error.contains("File preserved."),
            "{error}"
        );
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn an_edit_index_past_the_palette_is_rejected_and_the_file_is_untouched() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        let body = schema2_document(
            vec!["strata:air".to_string()],
            serde_json::json!([[[1, 2, 3], 1]]),
        );
        fs::write(path.join("world-a.json"), &body).unwrap();
        let error = refuse_open(&path);
        assert!(
            error.contains("invalid or duplicate block edit") && error.contains("File preserved."),
            "{error}"
        );
        assert_eq!(fs::read(path.join("world-a.json")).unwrap(), body);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn schema1_names_come_from_the_registry_palette() {
        let from_registry: Vec<String> = (0..16_u8)
            .map(|id| {
                registry::registry()
                    .name(Block::from_id(id).unwrap())
                    .to_string()
            })
            .collect();
        assert_eq!(crate::world::legacy_palette(), from_registry);
        assert_eq!(legacy_names(), crate::world::legacy_palette());
        assert_eq!(from_registry[0], "strata:air");
        assert_eq!(from_registry[3], "strata:stone");
        assert_eq!(from_registry.len(), 16);
        assert!(!from_registry.iter().any(|name| name == "strata:missing"));
        let (palette, _) = World::new(1).saved_palette_and_edits().unwrap();
        assert_eq!(palette, legacy_names());
    }
}
