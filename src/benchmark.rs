//! Reproducible, single-threaded CPU measurements of the production world mesher.
//! Dataset creation, mesh inspection, destruction, and report I/O are not timed.

use crate::world::{Block, CHUNK_SIZE, ChunkKey, Mesh, Vertex, World};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    hint::black_box,
    io::Write,
    mem::size_of,
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const SEED: u64 = 728_491;
const RUNS: usize = 5;
const HIGH_CHUNK: ChunkKey = (0, 6, 0);
const ROUTE: [ChunkKey; 12] = [
    (-2, 0, -2),
    (-1, 0, -1),
    (0, 0, -1),
    (1, 0, -2),
    (2, 0, -2),
    (2, 1, -2),
    (3, 1, -2),
    (1, 1, -1),
    (0, 1, 0),
    (-1, 1, 1),
    (2, 2, -2),
    (3, 2, -2),
];

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    dataset_version: u32,
    measured_at_unix_seconds: u64,
    measurement: &'static str,
    package_version: &'static str,
    build: Build,
    seed: u64,
    chunk_size: i32,
    warmup_runs_per_scenario: usize,
    measured_runs_per_scenario: usize,
    digest_algorithm: &'static str,
    scenarios: Vec<ScenarioReport>,
}

#[derive(Serialize)]
struct Build {
    debug_assertions: bool,
    profile_label_external: Option<String>,
    rustc_version_external: Option<String>,
    hardware_label_external: Option<String>,
    source_revision_external: Option<String>,
    target_os: &'static str,
    target_arch: &'static str,
    available_parallelism: Option<usize>,
    benchmark_threads: usize,
}

#[derive(Serialize)]
struct ScenarioReport {
    name: &'static str,
    description: &'static str,
    chunks: Vec<ChunkKey>,
    world_edit_count: usize,
    elapsed_ms: Summary,
    samples_ms: Vec<f64>,
    vertices_per_run: usize,
    triangles_per_run: usize,
    mesh_payload_bytes_per_run: usize,
    surface_area_per_run: f64,
    mesh_digest: String,
    repeatability_verified: bool,
}

#[derive(Debug, Serialize, PartialEq)]
struct Summary {
    min: f64,
    median: f64,
    p95_nearest_rank: f64,
    max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    vertices: usize,
    triangles: usize,
    bytes: usize,
    surface_area: f64,
    digest: u64,
}

struct Scenario {
    name: &'static str,
    description: &'static str,
    world: World,
    chunks: Vec<ChunkKey>,
    expected_surface_area: Option<f64>,
}

/// Write a new JSON report. Existing reports are never replaced. Optional runtime
/// environment labels are recorded as external assertions, not detected facts.
pub fn run_cpu_benchmark(output: &Path) -> Result<(), String> {
    if output
        .try_exists()
        .map_err(|e| format!("Cannot inspect benchmark output: {e}"))?
    {
        return Err(format!(
            "Benchmark report already exists: {}",
            output.display()
        ));
    }
    let mut reports = Vec::new();
    for scenario in scenarios() {
        let (_, expected) = measure(&scenario)?;
        if let Some(area) = scenario.expected_surface_area
            && expected.surface_area != area
        {
            return Err(format!(
                "{} surface area {} does not match independent fixture expectation {area}",
                scenario.name, expected.surface_area
            ));
        }
        let mut samples = Vec::with_capacity(RUNS);
        for run in 0..RUNS {
            let (elapsed, geometry) = measure(&scenario)?;
            if geometry != expected {
                return Err(format!(
                    "{} changed geometry during measured run {}; no report published",
                    scenario.name,
                    run + 1
                ));
            }
            samples.push(elapsed);
        }
        reports.push(ScenarioReport {
            name: scenario.name,
            description: scenario.description,
            chunks: scenario.chunks,
            world_edit_count: scenario.world.edits().len(),
            elapsed_ms: summarize(&samples)?,
            samples_ms: samples,
            vertices_per_run: expected.vertices,
            triangles_per_run: expected.triangles,
            mesh_payload_bytes_per_run: expected.bytes,
            surface_area_per_run: expected.surface_area,
            mesh_digest: format!("{:016x}", expected.digest),
            repeatability_verified: true,
        });
    }
    let report = Report {
        schema_version: 1,
        dataset_version: 1,
        measured_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| format!("System clock is before Unix epoch: {e}"))?
            .as_secs(),
        measurement: "sum of wall-clock durations of synchronous production World::mesh_chunk calls per scenario run; CPU work only, includes scheduling/preemption",
        package_version: env!("CARGO_PKG_VERSION"),
        build: Build {
            debug_assertions: cfg!(debug_assertions),
            profile_label_external: external_label("STRATA_BUILD_PROFILE"),
            rustc_version_external: external_label("STRATA_RUSTC_VERSION"),
            hardware_label_external: external_label("STRATA_HARDWARE_LABEL"),
            source_revision_external: external_label("STRATA_SOURCE_REVISION"),
            target_os: std::env::consts::OS,
            target_arch: std::env::consts::ARCH,
            available_parallelism: std::thread::available_parallelism().ok().map(usize::from),
            benchmark_threads: 1,
        },
        seed: SEED,
        chunk_size: CHUNK_SIZE,
        warmup_runs_per_scenario: 1,
        measured_runs_per_scenario: RUNS,
        digest_algorithm: "FNV-1a-64 over chunk keys/counts and ordered little-endian vertex f32 bits, texture-layer u32 and index u32 values; noncryptographic",
        scenarios: reports,
    };
    let mut bytes = serde_json::to_vec_pretty(&report)
        .map_err(|e| format!("Cannot serialize CPU benchmark report: {e}"))?;
    bytes.push(b'\n');
    publish_new_report(output, &bytes)
}

fn external_label(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn scenarios() -> Vec<Scenario> {
    let single = vec![HIGH_CHUNK];
    let mut isolated = World::new(SEED);
    let mut enclosed = World::new(SEED);
    let mut checkerboard = World::new(SEED);
    for z in -1..=CHUNK_SIZE {
        for y in -1..=CHUNK_SIZE {
            for x in -1..=CHUNK_SIZE {
                let position = [x, HIGH_CHUNK.1 * CHUNK_SIZE + y, z];
                enclosed.set_block(position, Block::Stone);
                if (x + y + z).rem_euclid(2) == 0 {
                    checkerboard.set_block(position, Block::Stone);
                }
                if [x, y, z]
                    .iter()
                    .all(|&value| (0..CHUNK_SIZE).contains(&value))
                {
                    isolated.set_block(position, Block::Stone);
                }
            }
        }
    }

    let mut edited = World::new(SEED);
    let ground = edited.surface_height(64, -64);
    // A cross-border excavation and raised beam replace real terrain in the
    // route. The cut crosses x=64 and z=-64 chunk boundaries, including AO halos.
    for z in -68..=-60 {
        for x in 48..=79 {
            for y in ground - 8..=ground + 3 {
                edited.set_block([x, y, z], Block::Air);
            }
            edited.set_block([x, ground - 9, z], Block::Basalt);
        }
    }
    for x in 56..=72 {
        edited.set_block([x, ground + 4, -64], Block::Wood);
    }

    vec![
        Scenario {
            name: "seeded_route",
            description: "12 fixed terrain, river, cave-depth and upper-air chunks; no edits",
            world: World::new(SEED),
            chunks: ROUTE.to_vec(),
            expected_surface_area: None,
        },
        Scenario {
            name: "empty_high_chunk",
            description: "32-cubed air chunk at y=192 with a one-cell air halo; matching Air edits elide to zero overrides",
            world: World::new(SEED),
            chunks: single.clone(),
            expected_surface_area: Some(0.0),
        },
        Scenario {
            name: "isolated_solid_chunk",
            description: "32-cubed Stone edit volume at y=192 surrounded by a one-cell generated-air halo",
            world: isolated,
            chunks: single.clone(),
            expected_surface_area: Some(f64::from(6 * CHUNK_SIZE * CHUNK_SIZE)),
        },
        Scenario {
            name: "enclosed_solid_chunk",
            description: "32-cubed Stone volume plus a fully solid one-cell halo; every face occluded",
            world: enclosed,
            chunks: single.clone(),
            expected_surface_area: Some(0.0),
        },
        Scenario {
            name: "checkerboard_high_chunk",
            description: "alternating Stone/Air cells continued through a one-cell halo; worst-case unmergeable exposed faces",
            world: checkerboard,
            chunks: single,
            expected_surface_area: Some(f64::from(CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE / 2 * 6)),
        },
        Scenario {
            name: "edited_terrain_route",
            description: "same 12-chunk route after a cross-border excavation, basalt floor and timber beam around (64,-64)",
            world: edited,
            chunks: ROUTE.to_vec(),
            expected_surface_area: None,
        },
    ]
}

fn measure(scenario: &Scenario) -> Result<(f64, Geometry), String> {
    let mut elapsed_ms = 0.0;
    let mut geometry = Geometry {
        vertices: 0,
        triangles: 0,
        bytes: 0,
        surface_area: 0.0,
        digest: 0xCBF2_9CE4_8422_2325,
    };
    for &key in &scenario.chunks {
        let start = Instant::now();
        let mesh = black_box(scenario.world.mesh_chunk(black_box(key)));
        elapsed_ms += start.elapsed().as_secs_f64() * 1000.0;
        inspect_mesh(&mesh, key, &mut geometry)
            .map_err(|error| format!("{} chunk {key:?}: {error}", scenario.name))?;
    }
    Ok((elapsed_ms, geometry))
}

fn inspect_mesh(mesh: &Mesh, key: ChunkKey, geometry: &mut Geometry) -> Result<(), String> {
    if !mesh.indices.len().is_multiple_of(3) {
        return Err("index count is not divisible by three".into());
    }
    for value in [key.0, key.1, key.2] {
        digest_bytes(&mut geometry.digest, &value.to_le_bytes());
    }
    for count in [mesh.vertices.len(), mesh.indices.len()] {
        digest_bytes(&mut geometry.digest, &(count as u64).to_le_bytes());
    }
    for vertex in &mesh.vertices {
        for value in vertex
            .position
            .iter()
            .chain(&vertex.normal)
            .chain(&vertex.color)
        {
            if !value.is_finite() {
                return Err("non-finite vertex component".into());
            }
            digest_bytes(&mut geometry.digest, &value.to_bits().to_le_bytes());
        }
        digest_bytes(&mut geometry.digest, &vertex.texture_layer.to_le_bytes());
    }
    for &index in &mesh.indices {
        if index as usize >= mesh.vertices.len() {
            return Err("index outside vertex buffer".into());
        }
        digest_bytes(&mut geometry.digest, &index.to_le_bytes());
    }
    for triangle in mesh.indices.chunks_exact(3) {
        let a = mesh.vertices[triangle[0] as usize].position.map(f64::from);
        let b = mesh.vertices[triangle[1] as usize].position.map(f64::from);
        let c = mesh.vertices[triangle[2] as usize].position.map(f64::from);
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let area = (cross[0].powi(2) + cross[1].powi(2) + cross[2].powi(2)).sqrt() * 0.5;
        if area == 0.0 {
            return Err("degenerate triangle".into());
        }
        geometry.surface_area += area;
    }
    geometry.vertices += mesh.vertices.len();
    geometry.triangles += mesh.indices.len() / 3;
    geometry.bytes +=
        mesh.vertices.len() * size_of::<Vertex>() + mesh.indices.len() * size_of::<u32>();
    Ok(())
}

fn digest_bytes(digest: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *digest = (*digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01B3);
    }
}

fn summarize(samples: &[f64]) -> Result<Summary, String> {
    if samples.is_empty()
        || samples
            .iter()
            .any(|sample| !sample.is_finite() || *sample < 0.0)
    {
        return Err("Benchmark samples must be nonempty, finite and nonnegative".into());
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    let midpoint = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        sorted[midpoint - 1] + (sorted[midpoint] - sorted[midpoint - 1]) / 2.0
    } else {
        sorted[midpoint]
    };
    // ceil(0.95 * N) = N - floor(N / 20), without an overflowing 95*N.
    let p95_index = sorted.len() - sorted.len() / 20 - 1;
    Ok(Summary {
        min: sorted[0],
        median,
        p95_nearest_rank: sorted[p95_index],
        max: sorted[sorted.len() - 1],
    })
}

fn publish_new_report(output: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|e| format!("Cannot create benchmark output directory: {e}"))?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("Cannot create temporary report name: {e}"))?
        .as_nanos();
    let temporary = parent.join(format!(".strata-cpu-{}-{nonce}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| format!("Cannot create temporary benchmark report: {e}"))?;
    let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("Cannot finish benchmark report: {error}"));
    }
    // Unlike rename on Unix, a hard link never replaces an existing destination.
    // Both names are in one directory; readers see the complete, synced file.
    let publish_result = fs::hard_link(&temporary, output);
    let cleanup_result = fs::remove_file(&temporary);
    publish_result.map_err(|e| format!("Cannot publish new benchmark report {} (destination must not exist and filesystem must support hard links): {e}", output.display()))?;
    cleanup_result.map_err(|e| {
        format!(
            "Report published at {}, but temporary link cleanup failed at {}: {e}",
            output.display(),
            temporary.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_summary_handles_one_even_odd_and_rank_boundaries() {
        assert_eq!(
            summarize(&[7.0]).unwrap(),
            Summary {
                min: 7.0,
                median: 7.0,
                p95_nearest_rank: 7.0,
                max: 7.0
            }
        );
        assert_eq!(summarize(&[9.0, 1.0, 5.0, 3.0]).unwrap().median, 4.0);
        assert_eq!(summarize(&[9.0, 1.0, 5.0]).unwrap().median, 5.0);
        assert_eq!(summarize(&[f64::MAX, f64::MAX]).unwrap().median, f64::MAX);
        let twenty: Vec<_> = (1..=20).map(f64::from).collect();
        assert_eq!(summarize(&twenty).unwrap().p95_nearest_rank, 19.0);
        assert!(summarize(&[]).is_err());
        assert!(summarize(&[f64::NAN]).is_err());
        assert!(summarize(&[f64::INFINITY]).is_err());
        assert!(summarize(&[-1.0]).is_err());
    }

    #[test]
    fn artificial_datasets_match_analytic_surfaces_and_real_edits_change_the_route() {
        let scenarios = scenarios();
        let mut route = None;
        let mut edited_route = None;
        for scenario in &scenarios {
            let (_, geometry) = measure(scenario).unwrap();
            if let Some(area) = scenario.expected_surface_area {
                assert_eq!(geometry.surface_area, area, "{}", scenario.name);
            }
            if scenario.name == "seeded_route" {
                route = Some(geometry);
            }
            if scenario.name == "edited_terrain_route" {
                edited_route = Some(geometry);
            }
        }
        assert_ne!(route.unwrap().digest, edited_route.unwrap().digest);
        assert!(route.unwrap().triangles > 0);
        let checkerboard = scenarios
            .iter()
            .find(|scenario| scenario.name == "checkerboard_high_chunk")
            .unwrap();
        let (_, geometry) = measure(checkerboard).unwrap();
        assert_eq!(geometry.triangles, (CHUNK_SIZE as usize).pow(3) / 2 * 12);
    }

    #[test]
    fn report_publication_preserves_an_existing_result() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "strata-benchmark-test-{}-{nonce}",
            std::process::id()
        ));
        let path = directory.join("report.json");
        publish_new_report(&path, b"original").unwrap();
        assert!(publish_new_report(&path, b"replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
