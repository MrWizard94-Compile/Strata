use crate::{
    upload::UploadCommand,
    world::{CHUNK_SIZE, ChunkKey, Mesh, World},
};
use std::{
    collections::HashSet,
    mem::size_of_val,
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
struct CompletedMesh {
    key: ChunkKey,
    mesh: Mesh,
    revision: u64,
    milliseconds: f32,
    queue_wait: Duration,
    ready_at: Instant,
}

struct PendingUpload {
    result: CompletedMesh,
    started: bool,
    cursor: usize,
}

impl PendingUpload {
    fn bytes(&self) -> usize {
        size_of_val(self.result.mesh.vertices.as_slice())
            + size_of_val(self.result.mesh.indices.as_slice())
    }
}

pub const MAX_COPY_SEGMENTS: usize = 16;

/// CPU-side upload admission, not a GPU completion or frame-time guarantee.
/// A callback already in progress cannot be preempted at the time limit.
#[derive(Clone, Copy, Debug)]
pub struct UploadBudget {
    pub max_meshes: usize,
    pub max_bytes: usize,
    pub max_step_bytes: usize,
    pub max_cpu_time: Duration,
}

impl Default for UploadBudget {
    fn default() -> Self {
        Self {
            max_meshes: 2,
            max_bytes: 4 * 1024 * 1024,
            max_step_bytes: 1024 * 1024,
            max_cpu_time: Duration::from_millis(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct UploadStats {
    pub meshes: usize,
    /// Vertex/index payload only; excludes allocator capacity and GPU overhead.
    pub bytes: usize,
    pub started: usize,
    pub segments: usize,
    pub max_segment_bytes: usize,
    pub cancelled: usize,
    pub remaining_bytes: usize,
    pub stale: usize,
    pub discarded: usize,
    pub inspected: usize,
    pub deferred: bool,
    /// Sum of actual upload callback wall-clock durations.
    pub upload_ms: f32,
    pub allocation_ms: f32,
    pub copy_ms: f32,
    pub finalize_ms: f32,
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
    deferred: Option<PendingUpload>,
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

    /// Copies at most max_bytes in aligned segments. A partially uploaded mesh
    /// retains its old visible replacement and pending job until Commit succeeds.
    /// Allocation and individual callbacks are not preemptible; time is soft.
    pub fn upload_ready(
        &mut self,
        budget: UploadBudget,
        upload: impl FnMut(UploadCommand<'_>) -> Result<(), String>,
    ) -> Result<UploadStats, String> {
        self.upload_ready_with_clock(budget, upload, Instant::now)
    }

    fn upload_ready_with_clock(
        &mut self,
        budget: UploadBudget,
        mut upload: impl FnMut(UploadCommand<'_>) -> Result<(), String>,
        mut now: impl FnMut() -> Instant,
    ) -> Result<UploadStats, String> {
        let started = now();
        let mut stats = UploadStats::default();
        let inspection_limit = self.workers.saturating_mul(3).saturating_add(1);
        // Cancel invalid partial work even while upload budgets are paused.
        if let Some(active) = self.deferred.as_ref() {
            stats.inspected += 1;
            let stale = active.result.revision != self.revision;
            let unwanted = !self.desired.contains(&active.result.key);
            if stale || unwanted {
                if active.started {
                    let before = now();
                    upload(UploadCommand::Cancel)?;
                    stats.finalize_ms +=
                        now().saturating_duration_since(before).as_secs_f32() * 1000.0;
                    stats.cancelled += 1;
                }
                let active = self.deferred.take().expect("checked active upload");
                if stale {
                    // A newer revision can already own this key's pending entry.
                    stats.stale += 1;
                } else {
                    self.pending.remove(&active.result.key);
                    stats.discarded += 1;
                }
            }
        }
        let enabled = budget.max_meshes > 0
            && budget.max_bytes >= 4
            && budget.max_step_bytes >= 4
            && !budget.max_cpu_time.is_zero();
        while enabled && stats.meshes < budget.max_meshes {
            if now().saturating_duration_since(started) >= budget.max_cpu_time {
                break;
            }
            if self.deferred.is_none() {
                if stats.inspected >= inspection_limit
                    || budget.max_bytes.saturating_sub(stats.bytes) < 4
                    || stats.segments >= MAX_COPY_SEGMENTS
                {
                    break;
                }
                let Some(result) = self
                    .receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv().ok())
                else {
                    break;
                };
                stats.inspected += 1;
                if result.revision != self.revision {
                    stats.stale += 1;
                    continue;
                }
                if !self.desired.contains(&result.key) {
                    self.pending.remove(&result.key);
                    stats.discarded += 1;
                    continue;
                }
                self.deferred = Some(PendingUpload {
                    result,
                    started: false,
                    cursor: 0,
                });
                // Recheck elapsed time after inspecting/acquiring a result.
                continue;
            }
            let active = self.deferred.as_mut().expect("active upload");
            let before = now();
            if before.saturating_duration_since(started) >= budget.max_cpu_time {
                break;
            }
            if !active.started {
                upload(UploadCommand::Begin {
                    key: active.result.key,
                    vertex_bytes: size_of_val(active.result.mesh.vertices.as_slice()),
                    index_count: active.result.mesh.indices.len(),
                })?;
                stats.allocation_ms +=
                    now().saturating_duration_since(before).as_secs_f32() * 1000.0;
                active.started = true;
                stats.started += 1;
                stats.max_queue_wait_ms = stats
                    .max_queue_wait_ms
                    .max(active.result.queue_wait.as_secs_f32() * 1000.0);
                stats.max_ready_wait_ms = stats.max_ready_wait_ms.max(
                    before
                        .saturating_duration_since(active.result.ready_at)
                        .as_secs_f32()
                        * 1000.0,
                );
                stats.max_mesh_ms = stats.max_mesh_ms.max(active.result.milliseconds);
            } else if active.cursor == active.bytes() {
                upload(UploadCommand::Commit)?;
                stats.finalize_ms += now().saturating_duration_since(before).as_secs_f32() * 1000.0;
                let active = self.deferred.take().expect("committed upload");
                self.pending.remove(&active.result.key);
                self.loaded.insert(active.result.key);
                self.invalidated.remove(&active.result.key);
                self.last_mesh_ms = active.result.milliseconds;
                stats.meshes += 1;
            } else {
                let capacity = budget
                    .max_bytes
                    .saturating_sub(stats.bytes)
                    .min(budget.max_step_bytes)
                    / 4
                    * 4;
                if capacity == 0 || stats.segments >= MAX_COPY_SEGMENTS {
                    break;
                }
                let vertices: &[u8] = bytemuck::cast_slice(&active.result.mesh.vertices);
                let indices: &[u8] = bytemuck::cast_slice(&active.result.mesh.indices);
                let is_vertex = active.cursor < vertices.len();
                let (buffer, offset) = if is_vertex {
                    (vertices, active.cursor)
                } else {
                    (indices, active.cursor - vertices.len())
                };
                let length = capacity.min(buffer.len() - offset);
                let data = &buffer[offset..offset + length];
                upload(if is_vertex {
                    UploadCommand::Vertices { offset, data }
                } else {
                    UploadCommand::Indices { offset, data }
                })?;
                stats.copy_ms += now().saturating_duration_since(before).as_secs_f32() * 1000.0;
                active.cursor += length;
                stats.bytes += length;
                stats.segments += 1;
                stats.max_segment_bytes = stats.max_segment_bytes.max(length);
            }
        }
        stats.deferred = self.deferred.is_some();
        stats.remaining_bytes = self
            .deferred
            .as_ref()
            .map_or(0, |active| active.bytes() - active.cursor);
        stats.upload_ms = stats.allocation_ms + stats.copy_ms + stats.finalize_ms;
        stats.elapsed_ms = now().saturating_duration_since(started).as_secs_f32() * 1000.0;
        Ok(stats)
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
        self.deferred.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::Vertex;
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
        let mut active_key = None;
        streaming
            .upload_ready_with_clock(
                UploadBudget::default(),
                |command| {
                    match command {
                        UploadCommand::Begin { key, .. } => active_key = Some(key),
                        UploadCommand::Commit => keys.push(active_key.take().unwrap()),
                        _ => {}
                    }
                    Ok(())
                },
                || clock,
            )
            .unwrap();
        keys
    }

    fn sized_result(
        key: ChunkKey,
        revision: u64,
        vertices: usize,
        indices: usize,
    ) -> CompletedMesh {
        let mut completed = result(key, revision);
        completed.mesh.vertices = vec![
            Vertex {
                position: [0.0; 3],
                normal: [0.0, 1.0, 0.0],
                color: [1.0; 4],
                texture_layer: 0,
            };
            vertices
        ];
        completed.mesh.indices = vec![0; indices];
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

    type PendingBytes = (ChunkKey, usize, usize, Vec<u8>, Vec<u8>);

    #[derive(Default)]
    struct Sink {
        active: Option<PendingBytes>,
        visible: std::collections::HashMap<ChunkKey, (Vec<u8>, Vec<u8>)>,
    }

    impl Sink {
        fn command(&mut self, command: UploadCommand<'_>) -> Result<(), String> {
            match command {
                UploadCommand::Begin {
                    key,
                    vertex_bytes,
                    index_count,
                } => {
                    assert!(self.active.is_none());
                    self.active =
                        Some((key, vertex_bytes, index_count * 4, Vec::new(), Vec::new()));
                }
                UploadCommand::Vertices { offset, data } => {
                    let (_, _, _, vertices, _) = self.active.as_mut().unwrap();
                    assert_eq!(offset, vertices.len());
                    vertices.extend_from_slice(data);
                }
                UploadCommand::Indices { offset, data } => {
                    let (_, vertex_bytes, _, vertices, indices) = self.active.as_mut().unwrap();
                    assert_eq!(vertices.len(), *vertex_bytes);
                    assert_eq!(offset, indices.len());
                    indices.extend_from_slice(data);
                }
                UploadCommand::Commit => {
                    let (key, vertex_bytes, index_bytes, vertices, indices) =
                        self.active.take().unwrap();
                    assert_eq!(vertices.len(), vertex_bytes);
                    assert_eq!(indices.len(), index_bytes);
                    self.visible.insert(key, (vertices, indices));
                }
                UploadCommand::Cancel => {
                    self.active.take();
                }
            }
            Ok(())
        }
    }

    #[test]
    fn partial_upload_obeys_byte_and_step_caps_and_commits_exact_bytes_atomically() {
        let (mut streaming, _jobs, results) = harness(8, 8);
        let key = (-2, 1, 3);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        streaming.loaded.insert(key);
        streaming.invalidated.insert(key);
        let mut completed = sized_result(key, 0, 3, 3);
        completed.mesh.vertices[1].position = [-63.0, 42.0, 110.0];
        completed.mesh.indices = vec![0, 1, 2];
        let expected = (
            bytemuck::cast_slice::<Vertex, u8>(&completed.mesh.vertices).to_vec(),
            bytemuck::cast_slice::<u32, u8>(&completed.mesh.indices).to_vec(),
        );
        results.send(completed).unwrap();
        let mut sink = Sink::default();
        let old = (vec![7; 44], vec![0; 12]);
        sink.visible.insert(key, old.clone());
        let budget = UploadBudget {
            max_bytes: 99,
            max_step_bytes: 23,
            ..UploadBudget::default()
        };
        let clock = Instant::now();
        let first = streaming
            .upload_ready_with_clock(budget, |c| sink.command(c), || clock)
            .unwrap();
        assert_eq!(
            (first.bytes, first.max_segment_bytes, first.meshes),
            (96, 20, 0)
        );
        assert_eq!(first.remaining_bytes, 48);
        assert!(first.deferred && streaming.pending.contains(&key));
        assert_eq!(streaming.progress().0, 0);
        assert_eq!(sink.visible[&key], old);
        let second = streaming
            .upload_ready_with_clock(budget, |c| sink.command(c), || clock)
            .unwrap();
        assert_eq!((second.bytes, second.meshes), (48, 1));
        assert!(!second.deferred && !streaming.pending.contains(&key));
        assert_eq!(streaming.progress().0, 1);
        assert_eq!(sink.visible[&key], expected);
    }

    #[test]
    fn two_nonempty_meshes_share_the_byte_budget_and_resume_at_exact_offsets() {
        let (mut streaming, _jobs, results) = harness(2, 2);
        let keys = [(0, 0, 0), (0, 1, 0)];
        streaming.desired.extend(keys);
        streaming.pending.extend(keys);
        let mut expected = Vec::new();
        for (index, key) in keys.into_iter().enumerate() {
            let mut completed = sized_result(key, 0, 3, 3);
            completed.mesh.vertices[0].position = [index as f32, 32.0, 7.0];
            completed.mesh.indices = vec![0, 1, 2];
            expected.push((
                bytemuck::cast_slice::<Vertex, u8>(&completed.mesh.vertices).to_vec(),
                bytemuck::cast_slice::<u32, u8>(&completed.mesh.indices).to_vec(),
            ));
            results.send(completed).unwrap();
        }
        let budget = UploadBudget {
            max_bytes: 200,
            max_step_bytes: 100,
            ..UploadBudget::default()
        };
        let clock = Instant::now();
        let mut sink = Sink::default();
        let first = streaming
            .upload_ready_with_clock(budget, |c| sink.command(c), || clock)
            .unwrap();
        assert_eq!((first.started, first.meshes, first.bytes), (2, 1, 200));
        assert_eq!((first.segments, first.remaining_bytes), (4, 88));
        assert!(first.deferred);
        assert_eq!(sink.visible[&keys[0]], expected[0]);
        assert!(!sink.visible.contains_key(&keys[1]));
        let (key, _, _, vertices, indices) = sink.active.as_ref().unwrap();
        assert_eq!(*key, keys[1]);
        assert_eq!(vertices, &expected[1].0[..56]);
        assert!(indices.is_empty());
        assert!(!streaming.pending.contains(&keys[0]));
        assert!(streaming.pending.contains(&keys[1]));

        let mut ranges = Vec::new();
        let second = streaming
            .upload_ready_with_clock(
                budget,
                |c| {
                    match &c {
                        UploadCommand::Vertices { offset, data } => {
                            ranges.push((true, *offset, data.len()));
                        }
                        UploadCommand::Indices { offset, data } => {
                            ranges.push((false, *offset, data.len()));
                        }
                        UploadCommand::Commit => {}
                        _ => panic!("a partial mesh must resume without restarting"),
                    }
                    sink.command(c)
                },
                || clock,
            )
            .unwrap();
        assert_eq!(ranges, [(true, 56, 76), (false, 0, 12)]);
        assert_eq!((second.started, second.meshes, second.bytes), (0, 1, 88));
        assert!(!second.deferred && streaming.pending.is_empty());
        assert!(sink.active.is_none());
        assert_eq!(sink.visible[&keys[1]], expected[1]);
        assert_eq!(streaming.progress(), (2, 2));
    }

    #[test]
    fn pathological_mesh_finishes_across_frames_without_an_oversize_exception() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (2, 4, -2);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        results
            .send(sized_result(key, 0, 393_216, 589_824))
            .unwrap();
        let clock = Instant::now();
        let mut sink = Sink::default();
        let mut bytes = 0;
        for frame in 0..5 {
            let stats = streaming
                .upload_ready_with_clock(UploadBudget::default(), |c| sink.command(c), || clock)
                .unwrap();
            assert!(stats.bytes <= 4 * 1024 * 1024);
            assert!(stats.max_segment_bytes <= 1024 * 1024);
            assert_eq!(stats.meshes, usize::from(frame == 4));
            assert_eq!(streaming.loaded.contains(&key), frame == 4);
            bytes += stats.bytes;
        }
        assert_eq!(bytes, 19_660_800);
        assert_eq!(
            sink.visible[&key].0.len() + sink.visible[&key].1.len(),
            bytes
        );
    }

    #[test]
    fn segment_count_bounds_tiny_steps_even_with_a_frozen_clock() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let clock = Instant::now();
        let stats = streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_step_bytes: 4,
                    ..UploadBudget::default()
                },
                |_| Ok(()),
                || clock,
            )
            .unwrap();
        assert_eq!(stats.segments, MAX_COPY_SEGMENTS);
        assert_eq!(stats.bytes, MAX_COPY_SEGMENTS * 4);
        assert!(stats.deferred);
    }

    #[test]
    fn disabled_or_unaligned_budgets_pause_without_losing_work() {
        for budget in [
            UploadBudget {
                max_meshes: 0,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_bytes: 3,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_step_bytes: 3,
                ..UploadBudget::default()
            },
            UploadBudget {
                max_cpu_time: Duration::ZERO,
                ..UploadBudget::default()
            },
        ] {
            let (mut streaming, _jobs, results) = harness(1, 1);
            let key = (0, 0, 0);
            streaming.desired.push(key);
            streaming.pending.insert(key);
            results.send(result(key, 0)).unwrap();
            let clock = Instant::now();
            let stats = streaming
                .upload_ready_with_clock(budget, |_| panic!("paused"), || clock)
                .unwrap();
            assert_eq!(stats.inspected, 0);
            assert!(streaming.pending.contains(&key));
            assert_eq!(ready(&mut streaming), [key]);
        }
    }

    #[test]
    fn a_slow_allocation_stops_before_copy_and_resumes_on_the_next_redraw() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        streaming.pending.insert(key);
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let base = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let mut sink = Sink::default();
        let stats = streaming
            .upload_ready_with_clock(
                UploadBudget::default(),
                |c| {
                    assert!(matches!(c, UploadCommand::Begin { .. }));
                    sink.command(c)?;
                    elapsed.set(Duration::from_millis(3));
                    Ok(())
                },
                || base + elapsed.get(),
            )
            .unwrap();
        assert_eq!((stats.started, stats.bytes, stats.meshes), (1, 0, 0));
        assert_ms(stats.allocation_ms, 3.0);
        assert_ms(stats.elapsed_ms, 3.0);
        assert!(streaming.pending.contains(&key));
        let next = streaming
            .upload_ready_with_clock(UploadBudget::default(), |c| sink.command(c), || base)
            .unwrap();
        assert_eq!((next.started, next.bytes, next.meshes), (0, 144, 1));
    }

    #[test]
    fn final_copy_deadline_defers_commit_and_rechecks_edits_on_the_next_redraw() {
        for edit_before_commit in [false, true] {
            let world = Arc::new(World::new(1));
            let (mut streaming, jobs, results) = harness(16, 8);
            streaming.update([0.0; 3], 0, &world);
            let queued: Vec<_> = jobs.try_iter().collect();
            let key = queued[0].key;
            streaming.loaded.insert(key);
            streaming.invalidated.insert(key);
            let completed = sized_result(key, 0, 3, 3);
            let expected = (
                bytemuck::cast_slice::<Vertex, u8>(&completed.mesh.vertices).to_vec(),
                bytemuck::cast_slice::<u32, u8>(&completed.mesh.indices).to_vec(),
            );
            results.send(completed).unwrap();
            let old = (vec![7; 44], vec![0; 12]);
            let mut sink = Sink::default();
            sink.visible.insert(key, old.clone());
            let base = Instant::now();
            let elapsed = Cell::new(Duration::ZERO);
            let first = streaming
                .upload_ready_with_clock(
                    UploadBudget::default(),
                    |c| {
                        assert!(!matches!(c, UploadCommand::Commit));
                        let final_copy = matches!(c, UploadCommand::Indices { .. });
                        sink.command(c)?;
                        if final_copy {
                            elapsed.set(Duration::from_millis(3));
                        }
                        Ok(())
                    },
                    || base + elapsed.get(),
                )
                .unwrap();
            assert_eq!(
                (first.bytes, first.meshes, first.remaining_bytes),
                (144, 0, 0)
            );
            assert_ms(first.copy_ms, 3.0);
            assert!(first.deferred && streaming.pending.contains(&key));
            assert!(streaming.invalidated.contains(&key));
            assert_eq!(streaming.progress().0, 0);
            assert_eq!(sink.visible[&key], old);

            if edit_before_commit {
                streaming.invalidate_edit([
                    key.0 * CHUNK_SIZE,
                    key.1 * CHUNK_SIZE,
                    key.2 * CHUNK_SIZE,
                ]);
                streaming.update([0.0; 3], 0, &world);
                assert!(
                    jobs.try_iter()
                        .any(|job| job.key == key && job.revision == 1)
                );
            }
            let second = streaming
                .upload_ready_with_clock(
                    UploadBudget::default(),
                    |c| {
                        assert!(if edit_before_commit {
                            matches!(c, UploadCommand::Cancel)
                        } else {
                            matches!(c, UploadCommand::Commit)
                        });
                        sink.command(c)
                    },
                    || base + elapsed.get(),
                )
                .unwrap();
            assert_eq!(
                (second.started, second.bytes, second.remaining_bytes),
                (0, 0, 0)
            );
            assert!(!second.deferred && sink.active.is_none());
            if edit_before_commit {
                assert_eq!((second.cancelled, second.stale, second.meshes), (1, 1, 0));
                assert!(streaming.pending.contains(&key));
                assert!(streaming.invalidated.contains(&key));
                assert_eq!(sink.visible[&key], old);
            } else {
                assert_eq!((second.cancelled, second.stale, second.meshes), (0, 0, 1));
                assert!(!streaming.pending.contains(&key));
                assert!(!streaming.invalidated.contains(&key));
                assert_eq!(sink.visible[&key], expected);
                assert_eq!(streaming.progress().0, 1);
            }
        }
    }

    #[test]
    fn time_spent_inspecting_retains_the_received_mesh_before_allocation() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        results.send(result(key, 0)).unwrap();
        let base = Instant::now();
        let calls = Cell::new(0);
        let stats = streaming
            .upload_ready_with_clock(
                UploadBudget::default(),
                |_| panic!("time exhausted"),
                || {
                    let call = calls.get();
                    calls.set(call + 1);
                    base + if call >= 2 {
                        Duration::from_millis(2)
                    } else {
                        Duration::ZERO
                    }
                },
            )
            .unwrap();
        assert_eq!((stats.inspected, stats.started), (1, 0));
        assert!(stats.deferred);
        assert_eq!(ready(&mut streaming), [key]);
    }

    #[test]
    fn queue_mesh_and_ready_waits_are_distinct_from_allocation_and_copy_time() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        let base = Instant::now();
        let mut completed = sized_result(key, 0, 3, 3);
        completed.ready_at = base;
        completed.queue_wait = Duration::from_millis(7);
        completed.milliseconds = 13.0;
        results.send(completed).unwrap();
        let elapsed = Cell::new(Duration::from_millis(11));
        let stats = streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_cpu_time: Duration::from_secs(1),
                    ..UploadBudget::default()
                },
                |_| {
                    elapsed.set(elapsed.get() + Duration::from_millis(1));
                    Ok(())
                },
                || base + elapsed.get(),
            )
            .unwrap();
        assert_eq!(stats.meshes, 1);
        assert_ms(stats.max_queue_wait_ms, 7.0);
        assert_ms(stats.max_ready_wait_ms, 11.0);
        assert_ms(stats.max_mesh_ms, 13.0);
        assert_ms(stats.allocation_ms, 1.0);
        assert_ms(stats.copy_ms, 2.0);
        assert_ms(stats.finalize_ms, 1.0);
        assert_ms(stats.upload_ms, 4.0);
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
            let stats = streaming
                .upload_ready_with_clock(
                    UploadBudget::default(),
                    |_| panic!("stale result"),
                    || clock,
                )
                .unwrap();
            assert_eq!((stats.inspected, stats.stale), (expected, expected));
            assert!(streaming.pending.contains(&key));
        }
    }

    #[test]
    fn editing_partial_upload_cancels_it_while_paused_and_preserves_the_new_pending_job() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let queued: Vec<_> = jobs.try_iter().collect();
        let key = queued[0].key;
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let clock = Instant::now();
        let mut sink = Sink::default();
        let old = (vec![7; 44], vec![0; 12]);
        sink.visible.insert(key, old.clone());
        streaming.loaded.insert(key);
        streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_bytes: 4,
                    ..UploadBudget::default()
                },
                |c| sink.command(c),
                || clock,
            )
            .unwrap();
        streaming.invalidate_edit([0; 3]);
        streaming.update([0.0; 3], 0, &world);
        assert!(jobs.try_iter().any(|j| j.key == key && j.revision == 1));
        let stats = streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_bytes: 0,
                    ..UploadBudget::default()
                },
                |c| sink.command(c),
                || clock,
            )
            .unwrap();
        assert_eq!((stats.cancelled, stats.stale, stats.bytes), (1, 1, 0));
        assert!(!stats.deferred && sink.active.is_none());
        assert_eq!(sink.visible[&key], old);
        assert!(streaming.pending.contains(&key));
        assert!(streaming.invalidated.contains(&key));
    }

    #[test]
    fn movement_cancels_partial_work_and_evicts_its_old_visible_mesh() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let key = jobs.try_iter().next().unwrap().key;
        streaming.loaded.insert(key);
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let clock = Instant::now();
        let mut sink = Sink::default();
        sink.visible.insert(key, (vec![7; 44], vec![0; 12]));
        streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_bytes: 4,
                    ..UploadBudget::default()
                },
                |c| sink.command(c),
                || clock,
            )
            .unwrap();
        let removed = streaming.update([320.0, 0.0, 0.0], 0, &world);
        assert_eq!(removed, [key]);
        for key in removed {
            sink.visible.remove(&key);
        }
        let stats = streaming
            .upload_ready_with_clock(UploadBudget::default(), |c| sink.command(c), || clock)
            .unwrap();
        assert_eq!((stats.cancelled, stats.discarded), (1, 1));
        assert!(sink.active.is_none() && sink.visible.is_empty());
        assert!(!streaming.pending.contains(&key));
    }

    #[test]
    fn movement_cancels_a_first_time_partial_upload_even_when_bytes_are_paused() {
        let world = Arc::new(World::new(1));
        let (mut streaming, jobs, results) = harness(16, 8);
        streaming.update([0.0; 3], 0, &world);
        let queued: Vec<_> = jobs.try_iter().collect();
        let key = queued[0].key;
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let clock = Instant::now();
        let mut sink = Sink::default();
        let first = streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_bytes: 4,
                    ..UploadBudget::default()
                },
                |c| sink.command(c),
                || clock,
            )
            .unwrap();
        assert_eq!((first.started, first.bytes, first.meshes), (1, 4, 0));
        assert!(first.deferred && streaming.pending.contains(&key));
        assert!(!streaming.loaded.contains(&key));
        assert!(sink.active.is_some() && sink.visible.is_empty());
        assert!(streaming.update([320.0, 0.0, 0.0], 0, &world).is_empty());
        assert!(!streaming.desired.contains(&key));

        let second = streaming
            .upload_ready_with_clock(
                UploadBudget {
                    max_bytes: 0,
                    ..UploadBudget::default()
                },
                |c| {
                    assert!(matches!(c, UploadCommand::Cancel));
                    sink.command(c)
                },
                || clock,
            )
            .unwrap();
        assert_eq!(
            (second.cancelled, second.discarded, second.bytes),
            (1, 1, 0)
        );
        assert_eq!((second.meshes, second.remaining_bytes), (0, 0));
        assert!(!second.deferred && !streaming.pending.contains(&key));
        assert!(!streaming.loaded.contains(&key));
        assert!(sink.active.is_none() && sink.visible.is_empty());
        streaming.update([0.0; 3], 0, &world);
        assert_eq!(jobs.try_iter().filter(|job| job.key == key).count(), 1);
        assert!(streaming.pending.contains(&key));
    }

    #[test]
    fn a_copy_error_cannot_mark_a_partial_mesh_current_or_replace_visible_data() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        let key = (0, 0, 0);
        streaming.desired.push(key);
        streaming.loaded.insert(key);
        streaming.invalidated.insert(key);
        streaming.pending.insert(key);
        results.send(sized_result(key, 0, 3, 3)).unwrap();
        let clock = Instant::now();
        let mut sink = Sink::default();
        let old = (vec![7; 44], vec![0; 12]);
        sink.visible.insert(key, old.clone());
        let error = streaming
            .upload_ready_with_clock(
                UploadBudget::default(),
                |c| {
                    if matches!(c, UploadCommand::Vertices { .. }) {
                        Err("injected copy failure".into())
                    } else {
                        sink.command(c)
                    }
                },
                || clock,
            )
            .unwrap_err();
        assert_eq!(error, "injected copy failure");
        assert!(streaming.pending.contains(&key) && streaming.invalidated.contains(&key));
        assert_eq!(sink.visible[&key], old);
        assert_eq!(streaming.progress().0, 0);
    }

    #[test]
    fn shutdown_disconnects_a_blocked_result_sender_before_joining_the_worker() {
        let (mut streaming, _jobs, results) = harness(1, 1);
        streaming.deferred = Some(PendingUpload {
            result: result((2, 0, 0), 0),
            started: false,
            cursor: 0,
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
