use crate::daylight::{self, validate_time_of_day};
use crate::interaction::{self, HOTBAR_SLOTS};
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

/// Schema of [`PlayerSave`] and [`Snapshot`]. Bump this when the document shape changes.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSave {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
}

/// Schema 1 world save. `time_of_day`, `hotbar`, and `selected_slot` are optional.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    schema: u32,
    generator: u32,
    generation: u64,
    pub seed: u64,
    pub player: PlayerSave,
    pub edits: Vec<([i32; 3], u8)>,
    /// Fraction of the day in `[0, 1)`, midnight at 0. Absent in older saves.
    #[serde(default = "default_time_of_day")]
    time_of_day: f32,
    /// Placeable block ids, slot 0 first. Older saves omit this and use the default bar.
    #[serde(default = "default_hotbar_ids")]
    hotbar: [u8; HOTBAR_SLOTS],
    /// Selected hotbar index, `0..HOTBAR_SLOTS`. Older saves omit this and use slot 0.
    #[serde(default)]
    selected_slot: u8,
}

fn default_time_of_day() -> f32 {
    daylight::MORNING
}

fn default_hotbar_ids() -> [u8; HOTBAR_SLOTS] {
    interaction::default_hotbar().map(|block| block as u8)
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        if self.generator < GENERATOR_VERSION {
            return Err(
                "This world was made with an older version of the world generator and can't be opened in this version. Create a new world to play. Your old world files were left as they are."
                    .into(),
            );
        }
        if self.schema != SCHEMA_VERSION || self.generator != GENERATOR_VERSION {
            return Err(
                "This world was saved by a different version of STRATA and can't be opened. Create a new world to play. Your old world files were left as they are."
                    .into(),
            );
        }
        if self.edits.len() > MAX_EDITS {
            return Err(format!(
                "World exceeds the current {MAX_EDITS} edited-block limit."
            ));
        }
        if !self
            .player
            .eye
            .iter()
            .all(|v| v.is_finite() && v.abs() <= COORD_LIMIT as f32)
            || !self.player.yaw.is_finite()
            || !self.player.pitch.is_finite()
            || self.player.pitch.abs() > 1.55
            || self.player.yaw.abs() > std::f32::consts::TAU
        {
            return Err("World contains an invalid player position or orientation.".into());
        }
        validate_time_of_day(self.time_of_day)?;
        if self.selected_slot as usize >= HOTBAR_SLOTS {
            return Err(
                "World contains a hotbar selection outside its eight slots. File preserved.".into(),
            );
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
                .any(|v| v.unsigned_abs() > COORD_LIMIT as u32)
                || Block::from_id(*id).is_none()
                || !seen.insert(*position)
            {
                return Err("World contains an invalid or duplicate block edit.".into());
            }
        }
        Ok(())
    }

    pub fn world(&self) -> World {
        let mut world = World::new(self.seed);
        for (position, id) in &self.edits {
            if let Some(block) = Block::from_id(*id) {
                world.set_block(*position, block);
            }
        }
        world
    }

    pub fn time_of_day(&self) -> f32 {
        self.time_of_day
    }

    pub fn hotbar(&self) -> [Block; HOTBAR_SLOTS] {
        self.hotbar.map(|id| {
            Block::from_id(id)
                .filter(|block| block.is_placeable())
                .expect("hotbar was validated")
        })
    }

    pub fn selected_slot(&self) -> usize {
        self.selected_slot as usize
    }
}

pub struct SaveStore {
    directory: PathBuf,
    generation: u64,
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
                    let snapshot: Snapshot = serde_json::from_slice(&data).map_err(|e| {
                        format!(
                            "Cannot decode {}: {e}. File preserved; restore a backup to continue.",
                            path.display()
                        )
                    })?;
                    snapshot.validate()?;
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
        Ok((
            Self {
                directory: directory.to_owned(),
                generation,
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
        let snapshot = Snapshot {
            schema: SCHEMA_VERSION,
            generator: GENERATOR_VERSION,
            generation,
            seed: world.seed(),
            player,
            edits: world.edits(),
            time_of_day,
            hotbar: hotbar.map(|block| block as u8),
            selected_slot,
        };
        snapshot.validate()?;
        let bytes =
            serde_json::to_vec(&snapshot).map_err(|e| format!("Cannot encode world: {e}"))?;
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
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|e| format!("Cannot stage save: {e}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("Cannot flush save: {e}"))?;
        drop(file);
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

#[cfg(test)]
mod tests {
    use super::*;
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
        world.set_block([-3, 90, 2], Block::Crystal);
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
        assert_eq!(loaded.world().block_at(-3, 90, 2), Block::Crystal);
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
        world.set_block([0, 110, 0], Block::Stone);
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
        let mut snapshot = Snapshot {
            schema: SCHEMA_VERSION + 8,
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
        snapshot.schema = SCHEMA_VERSION;
        snapshot.generator = GENERATOR_VERSION.saturating_sub(1);
        assert!(snapshot.validate().is_err());
        snapshot.generator = GENERATOR_VERSION + 1;
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
        snapshot.hotbar[0] = Block::Water as u8;
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
        world.set_block([2, 40, -4], Block::Stone);
        let mut slots = interaction::default_hotbar();
        slots[2] = Block::Dirt;
        slots[7] = Block::CoalOre;
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
        assert_eq!(loaded.hotbar()[2], Block::Dirt);
        assert_eq!(loaded.hotbar()[7], Block::CoalOre);
        assert_eq!(loaded.player.eye, player().eye);
        assert_eq!(loaded.world().block_at(2, 40, -4), Block::Stone);
        let slot = path.join("world-b.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        assert_eq!(value["schema"], serde_json::json!(SCHEMA_VERSION));
        assert_eq!(value["generator"], serde_json::json!(GENERATOR_VERSION));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn save_without_time_hotbar_or_slot_loads_defaults() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let mut world = World::new(42);
        world.set_block([4, 70, -2], Block::Dirt);
        let mut body = player();
        body.eye = [1.0, 80.0, 2.0];
        body.yaw = 0.2;
        body.pitch = -0.1;
        body.flying = true;
        save_world(
            &mut store,
            &world,
            body.clone(),
            0.75,
            interaction::default_hotbar(),
            3,
        );
        drop(store);
        let slot = path.join("world-b.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&slot).unwrap()).unwrap();
        assert_eq!(value["schema"], serde_json::json!(SCHEMA_VERSION));
        assert_eq!(value["generator"], serde_json::json!(GENERATOR_VERSION));
        let object = value.as_object_mut().expect("save is a JSON object");
        assert!(object.remove("time_of_day").is_some());
        assert!(object.remove("hotbar").is_some());
        assert!(object.remove("selected_slot").is_some());
        fs::write(&slot, serde_json::to_vec(&value).unwrap()).unwrap();
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
        assert_eq!(loaded.player.eye, body.eye);
        assert_eq!(loaded.world().block_at(4, 70, -2), Block::Dirt);
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

        value["hotbar"][0] = serde_json::json!(Block::Grass as u8);
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
}
