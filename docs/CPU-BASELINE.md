# CPU meshing baseline — 2026-10-20

These reports measure generator 1. Generator 2 changes terrain, biomes and ores;
its terrain-route timings and geometry need a separate baseline. Historical
reports and sidecars are preserved rather than relabelled as current evidence.

Three sequential invocations of `benchmark-cpu.ps1 -SkipBuild` exercised the just-built release executable. Each used dataset 1, seed 728491, 32³ chunks, one warmup and five measured runs per scenario. Full raw samples, geometry and source/binary/hardware provenance are retained under `evidence/`. The observed compiler was rustc 1.97.1; `debug_assertions=false`; one benchmark thread.

| Scenario | Chunks/run | Median milliseconds, runs 1 / 2 / 3 | All 15 samples min–max (ms) | Triangles/run | Mesh payload bytes/run |
|---|---:|---|---|---:|---:|
| Seeded terrain route | 12 | 32.041 / 32.327 / 32.123 | 31.963–32.501 | 27,306 | 2,730,600 |
| Empty high chunk | 1 | 1.942 / 1.947 / 1.944 | 1.941–1.966 | 0 | 0 |
| Isolated solid chunk | 1 | 2.587 / 2.617 / 2.573 | 2.550–2.642 | 612 | 61,200 |
| Halo-enclosed solid | 1 | 2.357 / 2.346 / 2.332 | 2.326–2.373 | 0 | 0 |
| Checkerboard | 1 | 18.330 / 18.624 / 18.024 | 17.908–18.969 | 196,608 | 19,660,800 |
| Edited terrain route | 12 | 32.547 / 32.588 / 32.094 | 32.007–32.694 | 27,320 | 2,732,000 |

All geometry digests and counts matched across the three reports. Each individual report's nearest-rank p95 equals its maximum because it has only five samples; these are baseline observations, not a strong tail-latency study. Route timings are totals for twelve synchronous meshing calls, not one frame or one chunk. The isolated solid produces more than twelve triangles because the current mesher retains spatial lighting variation boundaries.

The checkerboard payload is **18.75 MiB for one chunk**, excluding vector capacity, world/edit maps, scratch arrays, worker snapshots and GPU copies. Two uploads per frame therefore do not establish a small byte/time budget. Next experiments should measure upload duration/bytes and transient residency under this workload and evaluate chunk-local edit storage, compact geometry and budgeted upload scheduling.

The empty chunk still costs about 1.94 ms because the production path samples/generates a halo and runs the mesher. Investigate certified empty/solid-region shortcuts only if they preserve procedural trees, caves, edited cells and neighbour visibility. These numbers identify an optimization candidate; they do not prove its eventual benefit.

Machine: i7-9700, 8 cores/8 logical, 51,409,334,272 RAM bytes, Windows 11 Home 10.0.26300, High performance power plan. GPU is GTX 1660 Ti / driver 32.0.16.1742, but this command does not create a GPU device. Background apps were not isolated; no CPU affinity, thermal normalization or memory-peak capture.

Exact release SHA256: `8987752CF362DBC4CB780B6DDC31AD8D78C54DCCE60ADC632E009E9B971DA8AB`. Reports were taken before the initial Git source commit; sidecars disclose the uncommitted tree and SHA256 every source input instead of pretending a commit identified it. Root built and checked the source immediately before these `-SkipBuild` runs. PNG export and texturing were integrated in this measured build; each vertex is 44 bytes, including its texture layer.

This is partial evidence for S2.P1/R01 and PITCH-07. GPU times, frame-time traces during movement/edits, upload/queue latency, peak RAM/VRAM, cold startup distributions, long sessions and equivalent competing-engine comparisons remain open. No full-game performance target is claimed as passed.

The initial source checkpoint removes trailing blank lines from Cargo.toml and rust-toolchain.toml and normalizes Git text line endings. These cosmetic changes occurred after measurement; the retained raw input hashes describe the measured files. Rust and WGSL behavior is unchanged. Evidence files retain their exact original bytes in Git.
