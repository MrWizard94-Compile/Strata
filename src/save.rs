use crate::daylight::{self, validate_time_of_day};
use crate::interaction::{self, HOTBAR_SLOTS};
use crate::registry::{self, BlockId};
use crate::world::{Block, GENERATOR_VERSION, World};
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
    #[serde(default = "default_hotbar_ids")]
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

fn default_hotbar_ids() -> [u8; HOTBAR_SLOTS] {
    interaction::default_hotbar().map(|block| {
        block
            .legacy_u8()
            .expect("default hotbar ids are schema-1 builtins")
    })
}

fn default_mods() -> Vec<ModRecord> {
    vec![ModRecord {
        id: "strata".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        hash: registry::registry().builtin_content_hash().to_string(),
    }]
}

fn legacy_names() -> Vec<String> {
    (0..16_u8)
        .map(|id| {
            registry::registry()
                .name(BlockId::from_id(id).expect("0..=15 are builtins"))
                .to_string()
        })
        .collect()
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
                return Err("World contains an invalid or duplicate block edit.".into());
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
                return Err("World contains an invalid or duplicate block edit.".into());
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

fn validate_palette(palette: &[String]) -> Result<(), String> {
    if palette.is_empty() || palette.len() > 65_535 {
        return Err("World palette has an invalid length. File preserved.".into());
    }
    if palette[0] != "strata:air" {
        return Err("World palette must start with strata:air. File preserved.".into());
    }
    let mut seen = HashSet::with_capacity(palette.len());
    for name in palette {
        if !registry::is_block_id(name) || !seen.insert(name) {
            return Err(
                "World palette contains an invalid or duplicate block id. File preserved.".into(),
            );
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
    pub fn world(&self) -> World {
        self.materialize().0
    }

    pub fn time_of_day(&self) -> f32 {
        self.time_of_day
    }

    pub fn hotbar(&self) -> [Block; HOTBAR_SLOTS] {
        self.materialize().1
    }

    pub fn selected_slot(&self) -> usize {
        self.selected_slot as usize
    }

    fn materialize(&self) -> (World, [Block; HOTBAR_SLOTS]) {
        let mut world = World::new(self.seed);
        let ids = world
            .install_palette(self.palette.clone())
            .expect("palette was validated");
        for (position, index) in &self.edits {
            world.set_block(*position, ids[usize::from(*index)]);
        }
        let hotbar = self
            .hotbar_names
            .clone()
            .map(|name| world.intern_block(&name).expect("hotbar was validated"));
        (world, hotbar)
    }
}

pub struct SaveStore {
    directory: PathBuf,
    generation: u64,
    mods: Vec<ModRecord>,
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
                    let snapshot = decode_save(&path, &data)?;
                    if newest
                        .as_ref()
                        .is_none_or(|old| snapshot.generation > old.generation)
                    {
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
        let (palette, edits) = world.saved_palette_and_edits();
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
            mods: self.mods.clone(),
        };
        file.validate()?;
        let bytes = serde_json::to_vec(&file).map_err(|e| format!("Cannot encode world: {e}"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Encoded world exceeds save size limit".into());
        }
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
        Ok(())
    }
}

fn decode_save(path: &Path, bytes: &[u8]) -> Result<Snapshot, String> {
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
            Ok(file.into_snapshot())
        }
        Some(2) => {
            let file: Schema2File = serde_json::from_value(value).map_err(|error| {
                format!(
                    "Cannot decode {}: {error}. File preserved; restore a backup to continue.",
                    path.display()
                )
            })?;
            file.validate()?;
            Ok(file.into_snapshot())
        }
        _ => Err(DIFFERENT_VERSION.into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{AIR, COAL_ORE, CRYSTAL, DIRT, GRASS, STONE, WATER};
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
        assert_eq!(loaded.world().block_at(-3, 90, 2), CRYSTAL);
        assert_eq!(loaded.hotbar(), interaction::default_hotbar());
        assert_eq!(loaded.selected_slot(), 0);
        assert_eq!(loaded.time_of_day(), daylight::MORNING);
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
            hotbar: default_hotbar_ids(),
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
        snapshot.hotbar = default_hotbar_ids();
        snapshot.hotbar[0] = WATER.legacy_u8().unwrap();
        assert!(snapshot.validate().is_err());
        snapshot.hotbar = default_hotbar_ids();
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
        assert_eq!(loaded.hotbar()[2], DIRT);
        assert_eq!(loaded.hotbar()[7], COAL_ORE);
        assert_eq!(loaded.player.eye, player().eye);
        assert_eq!(loaded.world().block_at(2, 40, -4), STONE);
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
        assert_eq!(loaded.hotbar(), interaction::default_hotbar());
        assert_eq!(loaded.selected_slot(), 0);
        assert_eq!(loaded.seed, 42);
        assert!(loaded.player.flying);
        assert_eq!(loaded.player.eye, [1.0, 80.0, 2.0]);
        assert_eq!(loaded.world().block_at(4, 70, -2), DIRT);
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

        value["hotbar"] = serde_json::json!(default_hotbar_ids());
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
        assert_eq!(loaded.world().block_at(1, 2, 3), STONE);
        assert_eq!(loaded.world().block_key(STONE), "strata:stone");
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
        let world = loaded.world();
        let block = world.block_at(4, 80, 5);
        assert_ne!(block, AIR);
        assert_ne!(block, STONE);
        assert!(block.is_solid());
        assert_eq!(world.block_key(block), "example:gone");
        assert_eq!(world.display_name(block), "example:gone");
        let hotbar = loaded.hotbar();
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
            again.world().block_key(again.world().block_at(4, 80, 5)),
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
    fn a_well_formed_strata_hash_mismatch_still_loads() {
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
            &loaded.world(),
            loaded.player.clone(),
            loaded.time_of_day(),
            loaded.hotbar(),
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
        assert!(saved.contains(&"ab".repeat(32)), "{saved}");
        fs::remove_dir_all(path).unwrap();
    }
}
