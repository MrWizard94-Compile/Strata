# CPU meshing baseline

The CPU benchmark calls the same `World::mesh_chunk` implementation used by the streaming workers. It is a reproducible baseline for S2.P1/R01. It does not measure frame rate, GPU execution, upload latency, worker scaling, memory peaks, streaming responsiveness, cold application startup, or native playability. No target or competing-engine claim follows from this report alone.

## Run

From `C:\WPAI\Software\Rust Based Voxel`, build the locked release binary and record labels for the exact experiment. These commands run the compiler and Git outside the benchmark executable; labels remain explicitly external assertions in the report.

The recommended Windows entry point is `./benchmark-cpu.ps1 -ReportPath evidence/cpu-baseline-001.json`. It builds the locked release executable, records SHA256 hashes of source inputs and the binary, captures hardware/compiler/power-plan details, runs the headless benchmark and checks its exit status. A sibling `.provenance.json` identifies dirty/uncommitted trees explicitly. `-SkipBuild` requires the caller to establish that the binary matches the current source. The native CLI below is also available; use `Start-Process -Wait -PassThru` when a Windows GUI-subsystem executable needs explicit exit-status capture.

```powershell
cargo build --release --locked
$env:STRATA_BUILD_PROFILE = 'release'
$env:STRATA_RUSTC_VERSION = (rustc --version)
$env:STRATA_SOURCE_REVISION = (git rev-parse HEAD)
$env:STRATA_HARDWARE_LABEL = 'Record the measured CPU, RAM, OS and power mode here'
.\target\release\strata.exe --benchmark-cpu .\evidence\cpu-baseline-001.json
```

Use an accurate hardware label, and record any uncommitted source changes alongside the report; a Git SHA does not identify a dirty tree. Remove unset labels instead of guessing them. The report also records package version, target OS/architecture, available logical parallelism, actual `debug_assertions`, and the benchmark's one-thread execution. Profile names cannot reliably be inferred from debug assertions, so an absent external profile or compiler label is `null`.

Choose a new output name on every invocation. Existing reports are never replaced. Publication writes and syncs a unique temporary file in the destination directory, then creates the final name using a non-overwriting hard link and removes the temporary link. The destination filesystem must support hard links, as local NTFS does. Unsupported filesystems produce an error. This guards against partial visible JSON and replacement races; it does not promise durability after power loss. A crash can leave a uniquely named `.strata-cpu-*.tmp` file.

## Dataset version 1

Every scenario uses seed `728491` and the current `CHUNK_SIZE` of 32. A chunk coordinate `(x,y,z)` has block origin `(32x,32y,32z)`. Artificial fixtures occupy chunk `(0,6,0)`, whose base is y=192, above generated terrain and trees. Fixture construction occurs before timing. Air edits matching generated air are intentionally absent from the sparse edit map.

| Scenario | Exact workload | Independent check |
|---|---|---|
| `seeded_route` | The ordered 12-chunk route below, spanning negative coordinates, biome terrain, cave depths, surface and upper air | Valid finite mesh data; repeatable digest/counts on every run |
| `empty_high_chunk` | Generated air core and one-cell air halo at `(0,6,0)` | Zero surface area |
| `isolated_solid_chunk` | Every core cell edited to Stone; generated air halo | Area `6 × 32² = 6144` square blocks |
| `enclosed_solid_chunk` | Core plus its full one-cell halo edited to Stone | Zero surface area; no boundary faces |
| `checkerboard_high_chunk` | Stone where local `(x+y+z) mod 2 = 0`, Air otherwise, continued across the one-cell halo | `32³/2` isolated solids: area `98304`, triangles `196608` |
| `edited_terrain_route` | Same route after the deterministic excavation/floor/beam below | Geometry differs from the unedited route and remains repeatable |

Ordered route:

```text
(-2,0,-2), (-1,0,-1), (0,0,-1), (1,0,-2), (2,0,-2), (2,1,-2),
(3,1,-2), (1,1,-1), (0,1,0), (-1,1,1), (2,2,-2), (3,2,-2)
```

Let `g = World::surface_height(64,-64)`. The edited route sets Air over inclusive x=48..79, z=-68..-60, y=`g-8`..`g+3`; places a Basalt floor across the same x/z rectangle at `g-9`; and places a Wood beam at x=56..72, y=`g+4`, z=-64. Edits cross chunk boundaries. Reported edit counts reflect sparse overrides after baseline-equal edits are removed.

## Measurement and interpretation

Each scenario receives one unrecorded warmup run followed by five measured runs. A run times each synchronous production meshing call with `Instant`, then sums the call durations for that scenario. These are wall-clock durations of CPU meshing work, including scheduling or preemption; they are not process CPU-time accounting. For the route this is the total of 12 calls; artificial scenarios contain one call. Dataset construction, digest calculation, geometry validation, mesh destruction, and JSON writing are excluded. Calls run sequentially on one thread. No GPU device or game window is required by the benchmark path.

The report preserves all five samples in milliseconds plus minimum, median, nearest-rank p95 and maximum. Median averages the two middle values for an even sample count. Nearest-rank p95 uses `ceil(0.95 × N)` in one-based order; with five runs, p95 equals the maximum. Five samples provide a bounded baseline, not a robust tail-latency study. There are no timing pass/fail thresholds.

Counts and bytes are totals per scenario run. Mesh payload bytes are `vertices × size_of::<Vertex>() + indices × size_of::<u32>()`; they exclude vector spare capacity, allocator overhead, the World/edit map, intermediate mesher storage, copies, GPU buffers and peak resident memory. Do not label this number total RAM or VRAM.

Every run checks finite vertex components, valid triangle indices and nondegenerate triangles. Artificial fixture area is compared with independent analytic expectations. Geometry counts, area and the ordered digest must match the warmup; otherwise the command fails without publishing a report. The digest is FNV-1a-64 over each chunk's coordinates, counts, ordered vertex float bits, texture-layer u32 and indices, all explicitly little-endian. It detects accidental output drift; it is not a cryptographic hash or proof of visual correctness. A different valid triangulation or vertex/material contract can legitimately change it.

For before/after comparisons, retain both reports and source revisions, use the same dataset version, seed, chunk size, build profile, compiler and target machine, and record background load and power settings. Repeat full invocations to assess run-to-run variability. An intended world-generation or meshing-output change needs an explanation before unlike workloads can support a speedup claim. Compare matched scenarios rather than averaging the single-chunk and twelve-chunk cases together.

## Verification

```powershell
cargo test --locked benchmark::tests
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --check
```

Unit checks cover percentile edge cases, analytic artificial-fixture surfaces through the production mesher, changed route geometry after edits, and publication that preserves an existing report. Native rendering, interactive edits, GPU timings and combined process memory remain separate acceptance work. Benchmark results belong in experiment evidence, not as invented numbers in this document.
