use crate::world::{CHUNK_SIZE, ChunkKey, Mesh, PackedQuad, World};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Job {
    key: ChunkKey,
    revision: u64,
    world: Arc<World>,
    submitted_at: Instant,
}
struct PartialUpload {
    completed: CompletedMesh,
    uploaded_bytes: usize,
}

/// One budgeted write into a mesh that stays hidden until `complete`.
pub struct UploadSlice<'a> {
    pub key: ChunkKey,
    pub revision: u64,
    pub mesh: &'a Mesh,
    pub byte_offset: usize,
    pub byte_len: usize,
    pub complete: bool,
}

/// One admission step. `Reserve` allocates the destination before the timed
/// `Write`. The previous drawable mesh stays visible until the final write.
pub enum UploadAction<'a> {
    Reserve {
        key: ChunkKey,
        revision: u64,
        mesh: &'a Mesh,
    },
    Write(UploadSlice<'a>),
}

struct CompletedMesh {
    key: ChunkKey,
    mesh: Mesh,
    revision: u64,
    milliseconds: f32,
    queue_wait: Duration,
    ready_at: Instant,
}

/// CPU-side upload admission, not a GPU completion or frame-time guarantee.
/// One callback copies at most the remaining byte budget, aligned to a packed
/// quad, and cannot be preempted. A larger mesh continues on later redraws and
/// is not drawable until its last slice. There is no oversized exception.
#[derive(Clone, Copy, Debug)]
pub struct UploadBudget {
    pub max_meshes: usize,
    pub max_bytes: usize,
    pub max_cpu_time: Duration,
}

impl Default for UploadBudget {
    fn default() -> Self {
        Self {
            max_meshes: 2,
            max_bytes: 4 * 1024 * 1024,
            max_cpu_time: Duration::from_millis(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct UploadStats {
    /// Upload callbacks started this call. A segmented mesh uses one per slice.
    pub meshes: usize,
    /// Packed-quad bytes copied this call. Never exceeds `max_bytes`.
    pub bytes: usize,
    /// Slices whose mesh became fully resident.
    pub completed: usize,
    pub stale: usize,
    pub discarded: usize,
    pub inspected: usize,
    pub deferred: bool,
    /// A mesh still has slices waiting after this call.
    pub partial: bool,
    /// Sum of actual upload callback wall-clock durations.
    pub upload_ms: f32,
    /// Whole admission call wall-clock duration, including inspection and drops.
    pub elapsed_ms: f32,
    pub max_queue_wait_ms: f32,
    /// Completion-to-callback time, including result-channel backpressure.
    pub max_ready_wait_ms: f32,
    pub max_mesh_ms: f32,
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
    deferred: Option<CompletedMesh>,
    partial: Option<PartialUpload>,
    cancelled: Vec<ChunkKey>,
    desired: Vec<ChunkKey>,
    center: Option<(i32, i32, i32)>,
    revision: u64,
    pub workers: usize,
    pub last_mesh_ms: f32,
}

fn quad_aligned(bytes: usize) -> usize {
    let stride = std::mem::size_of::<PackedQuad>();
    bytes - bytes % stride
}

struct SliceWrite<'a> {
    result: &'a CompletedMesh,
    offset: usize,
    len: usize,
    complete: bool,
    callback_started: Instant,
}

fn record_slice(
    slice: SliceWrite<'_>,
    upload: &mut impl FnMut(UploadSlice<'_>),
    now: &mut impl FnMut() -> Instant,
    callback_time: &mut Duration,
    stats: &mut UploadStats,
) {
    upload(UploadSlice {
        key: slice.result.key,
        revision: slice.result.revision,
        mesh: &slice.result.mesh,
        byte_offset: slice.offset,
        byte_len: slice.len,
        complete: slice.complete,
    });
    *callback_time += now().saturating_duration_since(slice.callback_started);
    stats.meshes += 1;
    stats.bytes = stats.bytes.saturating_add(slice.len);
    stats.completed += usize::from(slice.complete);
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
                    let queue_wait = start.saturating_duration_since(job.submitted_at);
                    let mesh = job.world.mesh_chunk(job.key);
                    let ready_at = Instant::now();
                    if results
                        .send(CompletedMesh {
                            key: job.key,
                            mesh,
                            revision: job.revision,
                            milliseconds: ready_at.duration_since(start).as_secs_f32() * 1000.0,
                            queue_wait,
                            ready_at,
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
            deferred: None,
            partial: None,
            cancelled: Vec::new(),
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
            if let Some(partial) = self.partial.take() {
                if desired.contains(&partial.completed.key) {
                    self.partial = Some(partial);
                } else {
                    self.pending.remove(&partial.completed.key);
                    self.cancelled.push(partial.completed.key);
                }
            }
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
                submitted_at: Instant::now(),
            }) {
                Ok(()) => {
                    self.pending.insert(key);
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => break,
            }
        }
        removed
    }

    /// Apply valid results directly so the budget includes the real renderer
    /// callback. One result may wait outside the bounded channel. A mesh that
    /// does not fit is retained in `partial` and resumed before any new result.
    /// A non-empty partial upload emits `UploadAction::Reserve` once, before
    /// the timed `Write`.
    pub fn upload_ready_prepared(
        &mut self,
        budget: UploadBudget,
        step: impl FnMut(UploadAction<'_>),
    ) -> UploadStats {
        self.admit(budget, step, Instant::now)
    }

    pub fn take_cancelled_uploads(&mut self) -> Vec<ChunkKey> {
        std::mem::take(&mut self.cancelled)
    }

    #[cfg(test)]
    fn upload_ready_with_clock(
        &mut self,
        budget: UploadBudget,
        mut upload: impl FnMut(UploadSlice<'_>),
        now: impl FnMut() -> Instant,
    ) -> UploadStats {
        self.admit(
            budget,
            |action| {
                if let UploadAction::Write(slice) = action {
                    upload(slice);
                }
            },
            now,
        )
    }

    fn admit(
        &mut self,
        budget: UploadBudget,
        mut step: impl FnMut(UploadAction<'_>),
        mut now: impl FnMut() -> Instant,
    ) -> UploadStats {
        let started = now();
        let mut stats = UploadStats::default();
        let mut callback_time = Duration::ZERO;
        // Result channel capacity is 2*workers; at most workers results can be
        // waiting to send, plus one previously deferred result. A producer
        // refilling the channel cannot create an unbounded stale-draining loop.
        let inspection_limit = self.workers.saturating_mul(3).saturating_add(1);
        let enabled =
            budget.max_meshes > 0 && budget.max_bytes > 0 && !budget.max_cpu_time.is_zero();
        while enabled && stats.meshes < budget.max_meshes && stats.inspected < inspection_limit {
            if now().saturating_duration_since(started) >= budget.max_cpu_time {
                break;
            }
            if self.partial.is_none() {
                let result = if let Some(result) = self.deferred.take() {
                    result
                } else if let Some(result) = self
                    .receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv().ok())
                {
                    result
                } else {
                    break;
                };
                stats.inspected += 1;
                if result.revision != self.revision {
                    // Do not erase a newer revision's pending job for the same key.
                    stats.stale += 1;
                    continue;
                }
                if !self.desired.contains(&result.key) {
                    self.pending.remove(&result.key);
                    stats.discarded += 1;
                    continue;
                }
                let callback_started = now();
                // Queue inspection itself can consume the remaining time. Retain
                // this already-received result rather than dropping valid work.
                if callback_started.saturating_duration_since(started) >= budget.max_cpu_time {
                    self.deferred = Some(result);
                    break;
                }
                let payload = result.mesh.payload_bytes();
                if payload == 0 {
                    record_slice(
                        SliceWrite {
                            result: &result,
                            offset: 0,
                            len: 0,
                            complete: true,
                            callback_started,
                        },
                        &mut |slice| step(UploadAction::Write(slice)),
                        &mut now,
                        &mut callback_time,
                        &mut stats,
                    );
                    self.note_latency(&result, callback_started, &mut stats);
                    self.finish_mesh(&result);
                    continue;
                }
                let segment =
                    quad_aligned(budget.max_bytes.saturating_sub(stats.bytes).min(payload));
                if segment == 0 {
                    self.deferred = Some(result);
                    break;
                }
                self.partial = Some(PartialUpload {
                    completed: result,
                    uploaded_bytes: 0,
                });
                if let Some(partial) = self.partial.as_ref() {
                    let completed = &partial.completed;
                    step(UploadAction::Reserve {
                        key: completed.key,
                        revision: completed.revision,
                        mesh: &completed.mesh,
                    });
                }
            }
            let (offset, total, queue_wait, ready_at, milliseconds) = {
                let Some(partial) = self.partial.as_ref() else {
                    break;
                };
                (
                    partial.uploaded_bytes,
                    partial.completed.mesh.payload_bytes(),
                    partial.completed.queue_wait,
                    partial.completed.ready_at,
                    partial.completed.milliseconds,
                )
            };
            let remaining = total.saturating_sub(offset);
            let segment = quad_aligned(budget.max_bytes.saturating_sub(stats.bytes).min(remaining));
            if segment == 0 {
                break;
            }
            let callback_started = now();
            if callback_started.saturating_duration_since(started) >= budget.max_cpu_time {
                break;
            }
            let complete = offset + segment == total;
            record_slice(
                SliceWrite {
                    result: &self.partial.as_ref().expect("partial slice").completed,
                    offset,
                    len: segment,
                    complete,
                    callback_started,
                },
                &mut |slice| step(UploadAction::Write(slice)),
                &mut now,
                &mut callback_time,
                &mut stats,
            );
            if offset == 0 {
                stats.max_queue_wait_ms = stats
                    .max_queue_wait_ms
                    .max(queue_wait.as_secs_f32() * 1000.0);
                stats.max_ready_wait_ms = stats.max_ready_wait_ms.max(
                    callback_started
                        .saturating_duration_since(ready_at)
                        .as_secs_f32()
                        * 1000.0,
                );
                stats.max_mesh_ms = stats.max_mesh_ms.max(milliseconds);
                self.last_mesh_ms = milliseconds;
            }
            let partial = self.partial.as_mut().expect("partial slice");
            partial.uploaded_bytes += segment;
            if complete {
                let done = self.partial.take().expect("finished partial");
                self.finish_mesh(&done.completed);
            }
        }
        stats.deferred = self.deferred.is_some();
        stats.partial = self.partial.is_some();
        stats.upload_ms = callback_time.as_secs_f32() * 1000.0;
        stats.elapsed_ms = now().saturating_duration_since(started).as_secs_f32() * 1000.0;
        stats
    }

    fn note_latency(
        &mut self,
        result: &CompletedMesh,
        callback_started: Instant,
        stats: &mut UploadStats,
    ) {
        stats.max_queue_wait_ms = stats
            .max_queue_wait_ms
            .max(result.queue_wait.as_secs_f32() * 1000.0);
        stats.max_ready_wait_ms = stats.max_ready_wait_ms.max(
            callback_started
                .saturating_duration_since(result.ready_at)
                .as_secs_f32()
                * 1000.0,
        );
        stats.max_mesh_ms = stats.max_mesh_ms.max(result.milliseconds);
        self.last_mesh_ms = result.milliseconds;
    }

    fn finish_mesh(&mut self, result: &CompletedMesh) {
        self.pending.remove(&result.key);
        self.loaded.insert(result.key);
        self.invalidated.remove(&result.key);
    }

    pub fn invalidate_edit(&mut self, position: [i32; 3]) {
        self.revision = self.revision.wrapping_add(1);
        self.pending.clear();
        if let Some(partial) = self.partial.take() {
            self.cancelled.push(partial.completed.key);
        }
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
        self.partial.take();
        self.cancelled.clear();
        self.sender.take();
        self.receiver.take();
        self.deferred.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

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
                deferred: None,
                partial: None,
                cancelled: Vec::new(),
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
            queue_wait: Duration::from_millis(4),
            ready_at: Instant::now(),
        }
    }

    fn ready(streaming: &mut Streaming) -> Vec<ChunkKey> {
        let mut keys = Vec::new();
        let clock = Instant::now();
        streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |slice| {
                if slice.complete {
                    keys.push(slice.key);
                }
            },
            || clock,
        );
        keys
    }

    fn sized_result(key: ChunkKey, revision: u64, quads: usize) -> CompletedMesh {
        let mut completed = result(key, revision);
        completed.mesh.quads = vec![PackedQuad::default(); quads];
        completed
    }

    fn assert_ms(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.0001,
            "{actual} ms != {expected} ms"
        );
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
        assert!(ready(&mut streaming).is_empty());
        assert!(streaming.pending.contains(&old.key));
        assert!(!streaming.loaded.contains(&old.key));

        results.send(result(old.key, streaming.revision)).unwrap();
        let accepted = ready(&mut streaming);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0], old.key);
        assert!(!streaming.pending.contains(&old.key));
    }

    #[test]
    fn moving_away_discards_in_flight_results_and_evicts_delivered_chunks() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let old: Vec<_> = jobs.try_iter().collect();
        results.send(result(old[0].key, old[0].revision)).unwrap();
        assert_eq!(ready(&mut streaming).len(), 1);
        let removed = streaming.update([320.0, 0.0, 0.0], 0, &world);
        assert_eq!(removed, vec![old[0].key]);
        results.send(result(old[1].key, old[1].revision)).unwrap();
        assert!(ready(&mut streaming).is_empty());
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
        let delivered = ready(&mut streaming);
        let mut renderer_residency: HashSet<_> = delivered.into_iter().collect();

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
        ready(&mut streaming);
        streaming.invalidate_edit([0; 3]);
        streaming.update([0.0; 3], 0, &world);
        let replacements: Vec<_> = jobs.try_iter().collect();
        assert!(replacements.iter().any(|job| job.key == first.key));
        results.send(result(first.key, streaming.revision)).unwrap();
        assert_eq!(ready(&mut streaming).len(), 1);
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
        assert_eq!(ready(&mut streaming).len(), 2);
        assert_eq!(ready(&mut streaming).len(), 2);
        assert_eq!(ready(&mut streaming).len(), 2);
        assert!(ready(&mut streaming).is_empty());
        assert_eq!(streaming.progress(), (6, 6));
    }

    #[test]
    fn byte_limit_counts_packed_quad_payload_and_defers_without_losing_pending_work() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let keys = [(0, 0, 0), (0, 1, 0), (0, 2, 0)];
        streaming.desired.extend(keys);
        streaming.pending.extend(keys);
        let payload = std::mem::size_of::<PackedQuad>();
        for key in keys {
            results.send(sized_result(key, 0, 1)).unwrap();
        }
        let budget = UploadBudget {
            max_meshes: 3,
            max_bytes: payload * 2,
            ..UploadBudget::default()
        };
        let clock = Instant::now();
        let mut uploaded = Vec::new();
        let first =
            streaming.upload_ready_with_clock(budget, |slice| uploaded.push(slice.key), || clock);
        assert_eq!(uploaded, keys[..2]);
        assert_eq!(first.meshes, 2);
        assert_eq!(first.bytes, payload * 2);
        assert!(first.deferred);
        assert!(streaming.pending.contains(&keys[2]));
        assert!(!streaming.loaded.contains(&keys[2]));
        let second =
            streaming.upload_ready_with_clock(budget, |slice| uploaded.push(slice.key), || clock);
        assert_eq!(second.meshes, 1);
        assert!(!second.deferred);
        assert_eq!(uploaded, keys);
        assert!(streaming.pending.is_empty());
    }

    #[test]
    fn a_mesh_larger_than_the_byte_budget_uploads_in_aligned_slices() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let keys = [(0, 0, 0), (0, 1, 0)];
        streaming.desired.extend(keys);
        streaming.pending.extend(keys);
        results.send(sized_result(keys[0], 0, 5)).unwrap();
        results.send(sized_result(keys[1], 0, 1)).unwrap();
        let stride = std::mem::size_of::<PackedQuad>();
        let budget = UploadBudget {
            max_meshes: 2,
            // Not a multiple of the quad stride, so each slice rounds down.
            max_bytes: stride * 2 + 4,
            ..UploadBudget::default()
        };
        let clock = Instant::now();
        let mut slices = Vec::new();
        let admit = |streaming: &mut Streaming, recorded: &mut Vec<_>| {
            recorded.clear();
            streaming.upload_ready_with_clock(
                budget,
                |slice| {
                    assert!(slice.byte_len <= budget.max_bytes);
                    assert!(slice.byte_len.is_multiple_of(stride) || slice.byte_len == 0);
                    recorded.push((slice.key, slice.byte_offset, slice.byte_len, slice.complete));
                },
                || clock,
            )
        };
        let first = admit(&mut streaming, &mut slices);
        assert_eq!(slices, [(keys[0], 0, stride * 2, false)]);
        assert_eq!(first.meshes, 1);
        assert_eq!(first.completed, 0);
        assert_eq!(first.bytes, stride * 2);
        assert!(first.bytes <= budget.max_bytes);
        assert!(first.partial);
        assert!(!first.deferred);
        assert!(!streaming.loaded.contains(&keys[0]));
        assert!(streaming.pending.contains(&keys[0]));

        let second = admit(&mut streaming, &mut slices);
        assert_eq!(slices, [(keys[0], stride * 2, stride * 2, false)]);
        assert_eq!(second.completed, 0);
        assert!(second.partial);
        assert!(!streaming.loaded.contains(&keys[0]));

        let third = admit(&mut streaming, &mut slices);
        assert_eq!(
            slices,
            [
                (keys[0], stride * 4, stride, true),
                (keys[1], 0, stride, true)
            ]
        );
        assert_eq!(third.meshes, 2);
        assert_eq!(third.completed, 2);
        assert_eq!(third.bytes, stride * 2);
        assert!(!third.partial);
        assert!(!third.deferred);
        assert!(streaming.loaded.contains(&keys[0]));
        assert!(streaming.loaded.contains(&keys[1]));
        assert!(streaming.pending.is_empty());
    }

    #[test]
    fn a_zero_count_byte_or_time_budget_pauses_without_consuming_results() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let keys = [(0, 0, 0), (0, 1, 0)];
        streaming.desired.extend(keys);
        streaming.pending.extend(keys);
        streaming.deferred = Some(sized_result(keys[0], 0, 1));
        results.send(sized_result(keys[1], 0, 1)).unwrap();
        let clock = Instant::now();
        for budget in [
            UploadBudget {
                max_meshes: 0,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_bytes: 0,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_cpu_time: Duration::ZERO,
                ..UploadBudget::default()
            },
        ] {
            let stats = streaming.upload_ready_with_clock(
                budget,
                |_| panic!("disabled admission must not call the renderer"),
                || clock,
            );
            assert_eq!(stats.meshes, 0);
            assert_eq!(stats.inspected, 0);
            assert!(stats.deferred);
            assert_eq!(streaming.pending.len(), 2);
        }
        assert_eq!(ready(&mut streaming), keys);
    }

    #[test]
    fn time_limit_measures_the_real_callback_and_stops_after_an_overrun() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let keys = [(0, 0, 0), (0, 1, 0)];
        streaming.desired.extend(keys);
        streaming.pending.extend(keys);
        for key in keys {
            results.send(result(key, 0)).unwrap();
        }
        let base = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let mut uploaded = Vec::new();
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |slice| {
                uploaded.push(slice.key);
                elapsed.set(elapsed.get() + Duration::from_millis(3));
            },
            || base + elapsed.get(),
        );
        assert_eq!(uploaded, [keys[0]]);
        assert_ms(stats.upload_ms, 3.0);
        assert_ms(stats.elapsed_ms, 3.0);
        assert_eq!(stats.meshes, 1);
        assert!(streaming.pending.contains(&keys[1]));
        assert!(!streaming.loaded.contains(&keys[1]));
    }

    #[test]
    fn time_spent_inspecting_defers_a_received_mesh_before_starting_the_callback() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        results.send(result(key, 0)).unwrap();
        let base = Instant::now();
        let calls = Cell::new(0);
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| panic!("inspection already exhausted the time budget"),
            || {
                let call = calls.get();
                calls.set(call + 1);
                base + if call >= 2 {
                    Duration::from_millis(2)
                } else {
                    Duration::ZERO
                }
            },
        );
        assert_eq!(stats.meshes, 0);
        assert_eq!(stats.inspected, 1);
        assert_eq!(stats.upload_ms, 0.0);
        assert_ms(stats.elapsed_ms, 2.0);
        assert!(stats.deferred);
        assert!(streaming.pending.contains(&key));
        assert_eq!(ready(&mut streaming), [key]);
    }

    #[test]
    fn queue_mesh_and_ready_latency_are_reported_separately_from_upload_time() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        let base = Instant::now();
        let mut completed = result(key, 0);
        completed.ready_at = base;
        completed.queue_wait = Duration::from_millis(7);
        completed.milliseconds = 13.0;
        results.send(completed).unwrap();
        let elapsed = Cell::new(Duration::from_millis(11));
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| elapsed.set(elapsed.get() + Duration::from_millis(5)),
            || base + elapsed.get(),
        );
        assert_eq!(stats.meshes, 1);
        assert_ms(stats.max_queue_wait_ms, 7.0);
        assert_ms(stats.max_mesh_ms, 13.0);
        assert_ms(stats.max_ready_wait_ms, 11.0);
        assert_ms(stats.upload_ms, 5.0);
        assert_ms(stats.elapsed_ms, 5.0);
    }

    #[test]
    fn stale_draining_is_bounded_even_when_more_results_are_available() {
        let (mut streaming, _jobs, results) = harness(16, 16);
        streaming.revision = 1;
        let key = (0, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        for _ in 0..10 {
            results.send(result(key, 0)).unwrap();
        }
        let clock = Instant::now();
        for expected in [4, 4, 2] {
            let stats = streaming.upload_ready_with_clock(
                UploadBudget::default(),
                |_| panic!("stale results must not reach the renderer"),
                || clock,
            );
            assert_eq!(stats.inspected, expected);
            assert_eq!(stats.stale, expected);
            assert!(
                streaming.pending.contains(&key),
                "new revision's job remains pending"
            );
        }
    }

    #[test]
    fn an_edit_invalidates_a_deferred_result_without_erasing_its_replacement_job() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(8, 8);
        streaming.update([0.0; 3], 0, &world);
        let queued: Vec<_> = jobs.try_iter().collect();
        let keys = [queued[0].key, queued[1].key];
        results.send(sized_result(keys[0], 0, 2)).unwrap();
        results.send(sized_result(keys[1], 0, 1)).unwrap();
        let clock = Instant::now();
        let stats = streaming.upload_ready_with_clock(
            UploadBudget {
                max_bytes: std::mem::size_of::<PackedQuad>() * 2,
                ..UploadBudget::default()
            },
            |_| {},
            || clock,
        );
        assert!(stats.deferred);
        streaming.invalidate_edit([0; 3]);
        streaming.update([0.0; 3], 0, &world);
        assert!(
            jobs.try_iter()
                .any(|job| job.key == keys[1] && job.revision == 1)
        );
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| panic!("deferred old revision must be rejected"),
            || clock,
        );
        assert_eq!(stats.stale, 1);
        assert!(!stats.deferred);
        assert!(streaming.pending.contains(&keys[1]));
        assert!(!streaming.loaded.contains(&keys[1]));
    }

    #[test]
    fn movement_discards_a_deferred_mesh_and_releases_its_pending_entry() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let queued: Vec<_> = jobs.try_iter().collect();
        let keys = [queued[0].key, queued[1].key];
        results.send(sized_result(keys[0], 0, 2)).unwrap();
        results.send(sized_result(keys[1], 0, 1)).unwrap();
        let clock = Instant::now();
        assert!(
            streaming
                .upload_ready_with_clock(
                    UploadBudget {
                        max_bytes: std::mem::size_of::<PackedQuad>() * 2,
                        ..UploadBudget::default()
                    },
                    |_| {},
                    || clock,
                )
                .deferred
        );
        assert_eq!(streaming.update([320.0, 0.0, 0.0], 0, &world), [keys[0]]);
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| panic!("deferred mesh left the desired region"),
            || clock,
        );
        assert_eq!(stats.discarded, 1);
        assert!(!stats.deferred);
        assert!(!streaming.pending.contains(&keys[1]));
        assert!(!streaming.loaded.contains(&keys[1]));
    }

    #[test]
    fn destination_allocation_is_outside_the_timed_slice_and_can_defer_the_copy() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let key = (5, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        results.send(sized_result(key, 0, 2)).unwrap();
        let base = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let allocations = Cell::new(0u32);
        let budget = UploadBudget {
            max_bytes: std::mem::size_of::<PackedQuad>(),
            ..UploadBudget::default()
        };
        let blocked = streaming.admit(
            budget,
            |action| match action {
                UploadAction::Reserve { .. } => {
                    allocations.set(allocations.get() + 1);
                    elapsed.set(elapsed.get() + Duration::from_millis(3));
                }
                UploadAction::Write(_) => panic!("the copy must wait until a later frame"),
            },
            || base + elapsed.get(),
        );
        assert_eq!(allocations.get(), 1);
        assert_eq!(blocked.meshes, 0);
        assert_eq!(blocked.upload_ms, 0.0);
        assert!(blocked.partial);
        assert!(streaming.pending.contains(&key));
        assert!(!streaming.loaded.contains(&key));

        let mut copies = 0;
        let copied = streaming.admit(
            budget,
            |action| match action {
                UploadAction::Reserve { .. } => panic!("the destination is allocated once"),
                UploadAction::Write(_) => copies += 1,
            },
            || base + elapsed.get(),
        );
        assert_eq!(copies, 1);
        assert_eq!(copied.meshes, 1);
        assert_eq!(copied.completed, 0);
        assert!(copied.partial);
        assert!(!streaming.loaded.contains(&key));
    }

    #[test]
    fn a_slow_slice_stops_the_frame_before_the_next_slice() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let key = (4, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        results.send(sized_result(key, 0, 2)).unwrap();
        let base = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let mut slices = Vec::new();
        let stats = streaming.upload_ready_with_clock(
            UploadBudget {
                max_bytes: std::mem::size_of::<PackedQuad>(),
                ..UploadBudget::default()
            },
            |slice| {
                slices.push((slice.byte_offset, slice.byte_len, slice.complete));
                elapsed.set(elapsed.get() + Duration::from_millis(3));
            },
            || base + elapsed.get(),
        );
        assert_eq!(slices, [(0, std::mem::size_of::<PackedQuad>(), false)]);
        assert_eq!(stats.meshes, 1);
        assert_eq!(stats.completed, 0);
        assert_ms(stats.upload_ms, 3.0);
        assert!(stats.partial);
        assert!(!streaming.loaded.contains(&key));
        let elapsed = Cell::new(Duration::ZERO);
        let stats = streaming.upload_ready_with_clock(
            UploadBudget {
                max_bytes: std::mem::size_of::<PackedQuad>(),
                ..UploadBudget::default()
            },
            |slice| {
                slices.push((slice.byte_offset, slice.byte_len, slice.complete));
                elapsed.set(elapsed.get() + Duration::from_millis(1));
            },
            || base + elapsed.get(),
        );
        assert_eq!(stats.completed, 1);
        assert!(!stats.partial);
        assert!(streaming.loaded.contains(&key));
        assert_eq!(
            slices.last().copied(),
            Some((
                std::mem::size_of::<PackedQuad>(),
                std::mem::size_of::<PackedQuad>(),
                true
            ))
        );
    }

    #[test]
    fn an_edit_abandons_a_partial_upload_without_clearing_the_replacement_job() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(8, 8);
        streaming.update([0.0; 3], 0, &world);
        let old = jobs.try_recv().unwrap();
        let _queued: Vec<_> = jobs.try_iter().collect();
        streaming.partial = Some(PartialUpload {
            completed: sized_result(old.key, old.revision, 4),
            uploaded_bytes: std::mem::size_of::<PackedQuad>(),
        });
        streaming.invalidate_edit([0; 3]);
        assert_eq!(streaming.take_cancelled_uploads(), vec![old.key]);
        assert!(streaming.partial.is_none());
        streaming.update([0.0; 3], 0, &world);
        assert!(
            jobs.try_iter()
                .any(|job| job.key == old.key && job.revision == streaming.revision)
        );
        results
            .send(sized_result(old.key, old.revision, 4))
            .unwrap();
        let clock = Instant::now();
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| panic!("stale partial bytes must not be published"),
            || clock,
        );
        assert_eq!(stats.stale, 1);
        assert_eq!(stats.completed, 0);
        assert!(!streaming.loaded.contains(&old.key));
        assert!(streaming.pending.contains(&old.key));
    }

    #[test]
    fn leaving_the_region_cancels_a_partial_upload_and_drops_its_pending_entry() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, _results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let key = jobs.try_recv().unwrap().key;
        let _queued: Vec<_> = jobs.try_iter().collect();
        streaming.partial = Some(PartialUpload {
            completed: sized_result(key, streaming.revision, 3),
            uploaded_bytes: std::mem::size_of::<PackedQuad>(),
        });
        let removed = streaming.update([320.0, 0.0, 0.0], 0, &world);
        assert!(
            !removed.contains(&key),
            "an unfinished mesh was never resident"
        );
        assert_eq!(streaming.take_cancelled_uploads(), vec![key]);
        assert!(streaming.partial.is_none());
        assert!(!streaming.pending.contains(&key));
        let clock = Instant::now();
        let stats = streaming.upload_ready_with_clock(
            UploadBudget::default(),
            |_| panic!("unloaded partial must not resume"),
            || clock,
        );
        assert_eq!(stats.meshes, 0);
        assert!(!streaming.loaded.contains(&key));
    }

    #[test]
    fn a_paused_budget_retains_a_partial_upload() {
        let (mut streaming, _jobs, _results) = harness(4, 4);
        let key = (0, 1, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        streaming.partial = Some(PartialUpload {
            completed: sized_result(key, 0, 3),
            uploaded_bytes: std::mem::size_of::<PackedQuad>(),
        });
        let clock = Instant::now();
        for budget in [
            UploadBudget {
                max_meshes: 0,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_bytes: 0,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_cpu_time: Duration::ZERO,
                ..UploadBudget::default()
            },
        ] {
            let stats = streaming.upload_ready_with_clock(
                budget,
                |_| panic!("paused admission must not copy"),
                || clock,
            );
            assert_eq!(stats.meshes, 0);
            assert!(stats.partial);
            assert_eq!(
                streaming.partial.as_ref().unwrap().uploaded_bytes,
                std::mem::size_of::<PackedQuad>()
            );
        }
    }

    #[test]
    fn shutdown_disconnects_a_blocked_result_sender_before_joining_the_worker() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        streaming.deferred = Some(result((2, 0, 0), 0));
        streaming.partial = Some(PartialUpload {
            completed: sized_result((3, 0, 0), 0, 8),
            uploaded_bytes: std::mem::size_of::<PackedQuad>(),
        });
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
