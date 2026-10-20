use crate::world::{CHUNK_SIZE, ChunkKey, Mesh, World};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

struct Job {
    key: ChunkKey,
    revision: u64,
    world: Arc<World>,
}
pub struct CompletedMesh {
    pub key: ChunkKey,
    pub mesh: Mesh,
    revision: u64,
    pub milliseconds: f32,
}

pub struct Streaming {
    sender: Option<SyncSender<Job>>,
    receiver: Option<Receiver<CompletedMesh>>,
    threads: Vec<JoinHandle<()>>,
    /// Chunks already delivered to the renderer, including old meshes awaiting
    /// replacement after an edit. Keep residency until update emits an eviction.
    pub loaded: HashSet<ChunkKey>,
    invalidated: HashSet<ChunkKey>,
    pending: HashSet<ChunkKey>,
    desired: Vec<ChunkKey>,
    center: Option<(i32, i32, i32)>,
    revision: u64,
    pub workers: usize,
    pub last_mesh_ms: f32,
}

impl Streaming {
    pub fn new() -> Self {
        let workers = thread::available_parallelism()
            .map_or(2, usize::from)
            .saturating_sub(2)
            .clamp(1, 4);
        let (sender, jobs) = mpsc::sync_channel::<Job>(workers * 2);
        let jobs = Arc::new(Mutex::new(jobs));
        let (results, receiver) = mpsc::sync_channel(workers * 2);
        let mut threads = Vec::new();
        for _ in 0..workers {
            let jobs = jobs.clone();
            let results = results.clone();
            threads.push(thread::spawn(move || {
                loop {
                    let job = match jobs.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => break,
                    };
                    let Ok(job) = job else {
                        break;
                    };
                    let start = Instant::now();
                    let mesh = job.world.mesh_chunk(job.key);
                    if results
                        .send(CompletedMesh {
                            key: job.key,
                            mesh,
                            revision: job.revision,
                            milliseconds: start.elapsed().as_secs_f32() * 1000.0,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        Self {
            sender: Some(sender),
            receiver: Some(receiver),
            threads,
            loaded: HashSet::new(),
            invalidated: HashSet::new(),
            pending: HashSet::new(),
            desired: Vec::new(),
            center: None,
            revision: 0,
            workers,
            last_mesh_ms: 0.0,
        }
    }

    pub fn update(&mut self, eye: [f32; 3], radius: i32, world: &Arc<World>) -> Vec<ChunkKey> {
        let cx = (eye[0].floor() as i32).div_euclid(CHUNK_SIZE);
        let cz = (eye[2].floor() as i32).div_euclid(CHUNK_SIZE);
        let mut removed = Vec::new();
        if self.center != Some((cx, cz, radius)) {
            self.center = Some((cx, cz, radius));
            self.desired.clear();
            for z in -radius..=radius {
                for x in -radius..=radius {
                    if x * x + z * z > radius * radius {
                        continue;
                    }
                    for y in -1..=4 {
                        self.desired.push((cx + x, y, cz + z));
                    }
                }
            }
            let cy = (eye[1].floor() as i32).div_euclid(CHUNK_SIZE);
            self.desired.sort_by_key(|&(x, y, z)| {
                ((x - cx).pow(2) + (z - cz).pow(2)) * 4 + (y - cy).pow(2)
            });
            let desired: HashSet<_> = self.desired.iter().copied().collect();
            self.loaded.retain(|key| {
                if desired.contains(key) {
                    true
                } else {
                    removed.push(*key);
                    false
                }
            });
            self.invalidated.retain(|key| desired.contains(key));
        }
        for &key in &self.desired {
            if (self.loaded.contains(&key) && !self.invalidated.contains(&key))
                || self.pending.contains(&key)
            {
                continue;
            }
            let Some(sender) = &self.sender else {
                break;
            };
            match sender.try_send(Job {
                key,
                revision: self.revision,
                world: Arc::clone(world),
            }) {
                Ok(()) => {
                    self.pending.insert(key);
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => break,
            }
        }
        removed
    }

    pub fn take_ready(&mut self) -> Vec<CompletedMesh> {
        let mut ready = Vec::new();
        let Some(receiver) = &self.receiver else {
            return ready;
        };
        // Upload at most two chunks per frame, keeping GPU transfers bounded.
        while ready.len() < 2 {
            let Ok(result) = receiver.try_recv() else {
                break;
            };
            if result.revision != self.revision {
                continue;
            }
            self.pending.remove(&result.key);
            if !self.desired.contains(&result.key) {
                continue;
            }
            self.loaded.insert(result.key);
            self.invalidated.remove(&result.key);
            self.last_mesh_ms = result.milliseconds;
            ready.push(result);
        }
        ready
    }

    pub fn invalidate_edit(&mut self, position: [i32; 3]) {
        self.revision = self.revision.wrapping_add(1);
        self.pending.clear();
        let center = position.map(|c| c.div_euclid(CHUNK_SIZE));
        // Meshing reads a halo for occlusion and neighbouring faces.
        self.invalidated
            .extend(self.loaded.iter().copied().filter(|&(x, y, z)| {
                (x - center[0]).abs() <= 1
                    && (y - center[1]).abs() <= 1
                    && (z - center[2]).abs() <= 1
            }));
    }

    pub fn progress(&self) -> (usize, usize) {
        (
            self.loaded.len() - self.invalidated.len(),
            self.desired.len(),
        )
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        self.sender.take();
        self.receiver.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn harness(
        job_capacity: usize,
        result_capacity: usize,
    ) -> (Streaming, Receiver<Job>, SyncSender<CompletedMesh>) {
        let (sender, jobs) = mpsc::sync_channel(job_capacity);
        let (results, receiver) = mpsc::sync_channel(result_capacity);
        (
            Streaming {
                sender: Some(sender),
                receiver: Some(receiver),
                threads: Vec::new(),
                loaded: HashSet::new(),
                invalidated: HashSet::new(),
                pending: HashSet::new(),
                desired: Vec::new(),
                center: None,
                revision: 0,
                workers: 1,
                last_mesh_ms: 0.0,
            },
            jobs,
            results,
        )
    }

    fn result(key: ChunkKey, revision: u64) -> CompletedMesh {
        CompletedMesh {
            key,
            mesh: Mesh::default(),
            revision,
            milliseconds: 3.0,
        }
    }

    #[test]
    fn a_full_job_queue_keeps_frames_nonblocking_and_does_not_mark_unsent_work_pending() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, _results) = harness(1, 1);
        streaming.update([0.0; 3], 0, &world);
        assert_eq!(streaming.pending.len(), 1);
        assert_eq!(streaming.desired.len(), 6);
        let first = jobs.try_recv().expect("first job queued");
        assert!(Arc::ptr_eq(&first.world, &world));
        assert!(jobs.try_recv().is_err());
        streaming.update([0.0; 3], 0, &world);
        let second = jobs
            .try_recv()
            .expect("queue resumes after capacity is available");
        assert_ne!(first.key, second.key);
        assert_eq!(streaming.pending.len(), 2);
    }

    #[test]
    fn repeated_updates_deduplicate_in_flight_chunks_before_results_arrive() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, _results) = harness(8, 8);
        streaming.update([0.0; 3], 0, &world);
        let initial: Vec<_> = jobs.try_iter().collect();
        assert_eq!(initial.len(), 6);
        assert_eq!(
            initial
                .iter()
                .map(|job| job.key)
                .collect::<HashSet<_>>()
                .len(),
            6
        );
        streaming.update([0.0; 3], 0, &world);
        assert!(
            jobs.try_recv().is_err(),
            "running jobs must not be resubmitted"
        );
    }

    #[test]
    fn stale_results_cannot_replace_an_edited_mesh_or_clear_the_new_revision_job() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(8, 8);
        streaming.update([0.0; 3], 0, &world);
        let old = jobs.try_recv().unwrap();
        let _queued: Vec<_> = jobs.try_iter().collect();
        streaming.invalidate_edit([0; 3]);
        streaming.update([0.0; 3], 0, &world);
        let fresh: Vec<_> = jobs.try_iter().collect();
        assert!(
            fresh
                .iter()
                .any(|job| job.key == old.key && job.revision != old.revision)
        );

        results.send(result(old.key, old.revision)).unwrap();
        assert!(streaming.take_ready().is_empty());
        assert!(streaming.pending.contains(&old.key));
        assert!(!streaming.loaded.contains(&old.key));

        results.send(result(old.key, streaming.revision)).unwrap();
        let accepted = streaming.take_ready();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].key, old.key);
        assert!(!streaming.pending.contains(&old.key));
    }

    #[test]
    fn moving_away_discards_in_flight_results_and_evicts_delivered_chunks() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let old: Vec<_> = jobs.try_iter().collect();
        results.send(result(old[0].key, old[0].revision)).unwrap();
        assert_eq!(streaming.take_ready().len(), 1);
        let removed = streaming.update([320.0, 0.0, 0.0], 0, &world);
        assert_eq!(removed, vec![old[0].key]);
        results.send(result(old[1].key, old[1].revision)).unwrap();
        assert!(streaming.take_ready().is_empty());
        assert!(!streaming.pending.contains(&old[1].key));
        assert!(!streaming.loaded.contains(&old[1].key));
    }

    #[test]
    fn editing_then_moving_before_remesh_still_evicts_the_old_renderer_mesh() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let first = jobs.try_recv().unwrap();
        let _queued: Vec<_> = jobs.try_iter().collect();
        results.send(result(first.key, first.revision)).unwrap();
        let delivered = streaming.take_ready();
        let mut renderer_residency: HashSet<_> = delivered.iter().map(|mesh| mesh.key).collect();

        streaming.invalidate_edit([0; 3]);
        assert_eq!(
            streaming.progress().0,
            0,
            "invalidated meshes are not current progress"
        );
        assert!(
            streaming.loaded.contains(&first.key),
            "renderer still owns the old mesh"
        );
        for removed in streaming.update([320.0, 0.0, 0.0], 0, &world) {
            renderer_residency.remove(&removed);
        }
        assert!(
            renderer_residency.is_empty(),
            "an edit must not erase eviction bookkeeping"
        );
        assert!(streaming.invalidated.is_empty());
    }

    #[test]
    fn edits_conservatively_invalidate_all_halo_neighbors_including_negative_coordinates() {
        let (mut streaming, _jobs, _results) = harness(1, 1);
        for x in -3..=1 {
            for y in -3..=1 {
                for z in -3..=1 {
                    streaming.loaded.insert((x, y, z));
                }
            }
        }
        streaming.invalidate_edit([-1, -1, -1]);
        assert_eq!(streaming.invalidated.len(), 27);
        assert_eq!(
            streaming.loaded.len(),
            125,
            "invalidation preserves renderer residency"
        );
        for key in [(-2, -2, -2), (-1, -1, -1), (0, 0, 0)] {
            assert!(streaming.invalidated.contains(&key));
        }
        assert!(!streaming.invalidated.contains(&(1, -1, -1)));
    }

    #[test]
    fn accepting_a_replacement_restores_progress_without_duplicate_uploads() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let first = jobs.try_recv().unwrap();
        let _queued: Vec<_> = jobs.try_iter().collect();
        results.send(result(first.key, first.revision)).unwrap();
        streaming.take_ready();
        streaming.invalidate_edit([0; 3]);
        streaming.update([0.0; 3], 0, &world);
        let replacements: Vec<_> = jobs.try_iter().collect();
        assert!(replacements.iter().any(|job| job.key == first.key));
        results.send(result(first.key, streaming.revision)).unwrap();
        assert_eq!(streaming.take_ready().len(), 1);
        assert!(!streaming.invalidated.contains(&first.key));
        assert_eq!(streaming.progress().0, 1);
        streaming.update([0.0; 3], 0, &world);
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn ready_mesh_uploads_are_limited_to_two_per_frame() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(8, 8);
        streaming.update([0.0; 3], 0, &world);
        for job in jobs.try_iter() {
            results.send(result(job.key, job.revision)).unwrap();
        }
        assert_eq!(streaming.take_ready().len(), 2);
        assert_eq!(streaming.take_ready().len(), 2);
        assert_eq!(streaming.take_ready().len(), 2);
        assert!(streaming.take_ready().is_empty());
        assert_eq!(streaming.progress(), (6, 6));
    }

    #[test]
    fn shutdown_disconnects_a_blocked_result_sender_before_joining_the_worker() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        results.send(result((0, 0, 0), 0)).unwrap();
        let (entered, receiving) = mpsc::sync_channel(1);
        let (finished, completion) = mpsc::sync_channel(1);
        streaming.threads.push(thread::spawn(move || {
            entered.send(()).unwrap();
            assert!(results.send(result((1, 0, 0), 0)).is_err());
        }));
        receiving.recv_timeout(Duration::from_secs(2)).unwrap();
        // Run Drop off-thread so a regression fails with a bounded timeout rather
        // than hanging the suite while joining a sender blocked on a full queue.
        let dropper = thread::spawn(move || {
            drop(streaming);
            finished.send(()).unwrap();
        });
        completion
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown releases the full results queue");
        dropper.join().unwrap();
    }
}
