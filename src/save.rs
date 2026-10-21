use crate::daylight::{self, validate_time_of_day};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerSave {
    pub eye: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
}

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
}

fn default_time_of_day() -> f32 {
    daylight::MORNING
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        if self.generator < GENERATOR_VERSION {
            return Err(
                "This world was made with an older version of the world generator and can't be opened in this version. Create a new world to play. Your old world files were left as they are."
                    .into(),
            );
        }
        if self.schema != 1 || self.generator != GENERATOR_VERSION {
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
    ) -> Result<(), String> {
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("World generation counter exhausted")?;
        let snapshot = Snapshot {
            schema: 1,
            generator: GENERATOR_VERSION,
            generation,
            seed: world.seed(),
            player,
            edits: world.edits(),
            time_of_day,
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
    #[test]
    fn roundtrip_retains_seed_negative_coordinate_edits_and_player() {
        let path = directory();
        let (mut store, initial) = SaveStore::open(&path).unwrap();
        assert!(initial.is_none());
        let mut world = World::new(42);
        world.set_block([-3, 90, 2], Block::Crystal);
        store.save(&world, player(), daylight::MORNING).unwrap();
        drop(store);
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.player.eye, player().eye);
        assert_eq!(loaded.world().block_at(-3, 90, 2), Block::Crystal);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn interrupted_pending_save_does_not_replace_committed_generation() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let mut world = World::new(9);
        store.save(&world, player(), daylight::MORNING).unwrap();
        world.set_block([0, 110, 0], Block::Stone);
        store.save(&world, player(), daylight::MORNING).unwrap();
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
            schema: 9,
            generator: 1,
            generation: 1,
            seed: 0,
            player: player(),
            edits: vec![],
            time_of_day: daylight::MORNING,
        };
        assert!(snapshot.validate().is_err());
        snapshot.schema = 1;
        snapshot.generator = 1;
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
    }

    #[test]
    fn time_of_day_round_trips() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        let world = World::new(11);
        store.save(&world, player(), 0.75).unwrap();
        drop(store);
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(
            daylight::DayClock::from_fraction(loaded.time_of_day())
                .unwrap()
                .tick(),
            daylight::TICKS_PER_DAY * 3 / 4
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn old_save_without_time_of_day_loads_at_morning() {
        let path = directory();
        fs::create_dir_all(&path).unwrap();
        fs::write(
            path.join("world-a.json"),
            format!(
                r#"{{
                "schema": 1,
                "generator": {GENERATOR_VERSION},
                "generation": 3,
                "seed": 99,
                "player": {{"eye": [4.0, 70.0, -2.0], "yaw": 0.5, "pitch": -0.2, "flying": true}},
                "edits": [[[-8, 40, 1], 1]]
            }}"#
            ),
        )
        .unwrap();
        let (_, loaded) = SaveStore::open(&path).unwrap();
        let loaded = loaded.unwrap();
        assert_eq!(loaded.time_of_day(), daylight::MORNING);
        assert_eq!(
            daylight::DayClock::from_fraction(loaded.time_of_day())
                .unwrap()
                .tick(),
            daylight::DayClock::morning().tick()
        );
        assert_eq!(loaded.seed, 99);
        assert!(loaded.player.flying);
        assert_eq!(loaded.world().block_at(-8, 40, 1), Block::Grass);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn invalid_time_of_day_is_rejected_and_the_file_is_preserved() {
        let path = directory();
        let (mut store, _) = SaveStore::open(&path).unwrap();
        store.save(&World::new(3), player(), 0.5).unwrap();
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
            store.save(&World::new(3), player(), 0.5).unwrap();
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
