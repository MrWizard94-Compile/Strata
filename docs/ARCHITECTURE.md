# Engine architecture and decisions

STRATA is an original Rust voxel engine/game under active development. This document describes the implemented foundation and its extension boundaries. The complete delivery plan is in [PROJECT-TRACKER.md](PROJECT-TRACKER.md).

## Current execution path

`winit ApplicationHandler → input/UI → fixed 60 Hz player tick → bounded CPU meshing workers → main-thread GPU uploads → wgpu world pass → egui overlay → surface presentation`.

The player and world edits are owned by the application thread. Worker jobs share immutable `Arc<World>` snapshots. An edit uses copy-on-write, invalidates a conservative neighbour region and increments a mesh revision. Results from earlier revisions are discarded. Queues are bounded to twice the worker count. Streaming copies at most 4 MiB per redraw in segments up to 1 MiB, with sixteen segments and two mesh completions per call. A 2 ms soft CPU limit is checked between commands. One incomplete mesh retains ownership and is revalidated on retry; an old resident remains visible until all replacement bytes are queued and Commit succeeds. Invalid partial uploads are cancelled, including while budgets are paused. The renderer uses conservative chunk bounds rather than rescanning vertices, and submits queued copies even when surface acquisition is skipped. See STREAMING.md for measured costs and remaining GPU/memory/time limits. Workers use a shared receive lock only to take a job, then mesh independently; this is not described as lock-free.

`World` is a seed plus sparse edits. Terrain is generated from integer coordinates using seeded noise and deterministic tree anchors. A 32³ chunk samples a one-block halo, generates exposed faces and greedily merges compatible material/AO faces. A block ID is one byte in the sampled working buffer; mesh storage, scratch arrays, edits and GPU buffers are additional costs. This is conventional greedy meshing, not the roadmap's binary bitmask mesher.

Rendering uses native wgpu 30, depth-tested mesh buffers, CPU frustum culling, directional lighting, per-vertex ambient occlusion, atmospheric sky/fog, thirteen original 32×32 sRGB material tiles, stylized reflective water and a selected-block outline. A texture array isolates materials; world-axis coordinates repeat one tile per block across merged quads. Face-specific layers distinguish grass and timber ends. Six mips average in linear light, with nearest magnification and filtered minification. Vertex RGB carries lighting variation, not a second albedo palette. Water remains visually opaque and non-solid. Global illumination, true fluid simulation, GPU visibility/indirect submission, LOD and physical construction are future phases.

Save schema 1 / generator 1 stores seed, sorted sparse edits and player state. Two alternating slots retain the preceding committed generation during replacement. A flushed pending file is renamed to the inactive slot; the active slot is retained. Unsupported versions, oversized saves, unknown block IDs, duplicate edits and non-finite/out-of-range player states are rejected. A corrupt slot stops loading with an explicit error, preserving it for recovery. This protects common interrupted writes; it is not a filesystem power-loss durability proof. A later format must migrate or preserve old worlds, not silently regenerate them under changed terrain rules.

`SaveStore` holds an OS-exclusive lock for its lifetime. A second writer is rejected; a leftover empty lock marker is harmless once the process closes. Controlled rendering and event-loop failures attempt a CPU-world save before teardown and retain failure diagnostics. The launcher captures those diagnostics under `logs/`; unexpected process termination/panic is not claimed to invoke this controlled-save path.

New spawn and fall recovery search upward for a nonintersecting full-body AABB, including trees and edits. Search is bounded at eye height 512; if construction fills the recovery column, creative flight provides escape. Turning flight off while intersecting terrain is rejected with feedback.

Presentation telemetry counts only calls that reach `Queue::present`; skipped acquisitions are separately counted. Intervals measure CPU-observed presentation-submission cadence, not GPU execution or physical display timing. The 240-sample window resets on entering/resuming. Skipped/unfocused rendering retries are throttled; minimized windows wait for events.

## Decisions and reversal paths

| Decision | Reason | Consequence / reversal |
|---|---|---|
| Direct wgpu + winit; egui UI | Engine is the director's central goal; direct visibility into GPU resources and platform behavior | More integration work than a complete game engine. Keep world/simulation APIs independent so renderer can evolve. |
| Rust 1.97.1 and exact current primary dependency versions | Compile against one declared, reproducible graph; verify current APIs from installed source | Lockfile must accompany source. Updates need integration, shader and native tests. |
| 32-block chunk edge | Moderate edit/remesh unit and initial memory budget | Benchmark before changing; storage coordinates/save semantics remain world-block based. |
| Conventional greedy meshing first | Establish an independently testable geometry baseline | Binary meshing is admitted only after equivalent material/AO/edge output and measured benefit. |
| Bounded workers, immutable snapshots | Explicit ownership, cancellation by revision and no mutable shared world | Large edit maps can make snapshots expensive; measure copy-on-write and move toward per-chunk versions if needed. |
| Fixed simulation tick, independently rendered frames | Stable collision update order and responsive presentation | Presently player-only; no claim of deterministic whole-world simulation or full interpolation. |
| Original procedural assets | Coherent early visual foundation without copying protected game assets | Art direction, animation, sound and material authored detail require dedicated later phases. |
| Windows first, GTX 1660 Ti measured first | Real available acceptance machine | Vulkan/DX12 backend availability does not establish Xbox compatibility; console evidence is a separate stage. |

## Known architectural limits to address

- World mesh coordinates use f32; far-coordinate precision is not yet premium large-world behavior. Camera-relative geometry and integer spatial authority are a planned S2 contract.
- Vertically streamed chunks currently span -32 through 159. Editing is constrained to -30 through 158. Large or unbounded vertical construction is not yet supported.
- The sparse edit limit is 100,000 and encoded save size is 16 MiB. These are explicit safety bounds for schema 1, not the final creative-world capacity.
- Collision samples procedural terrain directly. Dense entity populations need chunk/collider caching and measured invalidation.
- All edits globally invalidate in-flight revisions. This is correct but can waste work under rapid editing; per-region generations and priority lanes need measurement.
- The terrain and simulation seed is deterministic, but cross-platform bit-for-bit floating-point procedural equivalence has not been demonstrated.
- No ECS, multiplayer, mod sandbox, machines, ecology, progression or authored content loop is currently implemented. Do not present crate names as evidence of these systems.

## Research and acceptance discipline

The supplied roadmap's exact timings are unverified targets, not measurements of this engine. `64³ × 2 bytes = 512 KiB` before overhead, so its 256 KB dense-u16 chunk figure is inconsistent. A radius measured in chunks needs an explicit edge size. A 60 Hz tick alone does not establish deterministic simulation. Wasm fuel, epoch interruption, memory limits, host-call validation and crash recovery are separate mechanisms.

Every material performance experiment must record commit, build profile, seed, region, camera path, edits, resolution, quality, GPU/driver, warm/cold state and percentile frame times. CPU meshing microbenchmarks cannot establish total render latency or Xbox frame rate. Pitch claims require measured operation on the hardware claimed.
