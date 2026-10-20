# Current checkpoint

2026-10-20. **Goal active.** STRATA has a native editable-world foundation, original 32×32 materials and reproducible CPU meshing evidence. The requested production and studio grade survival/combat/building/exploration game and Microsoft pitch remain incomplete.

The canonical local project is `C:\WPAI\Software\Rust Based Voxel`, an independent Git root. `origin` now points to `https://github.com/MrWizard94-Compile/Strata.git`; fetch succeeded. GitHub reports that repository PUBLIC and empty. No source or artifacts have been pushed or published.

## Source and artifact verification

- Deterministic terrain, rivers/caves/trees, sparse edits, greedy meshing, DDA targeting, walking/jump/flight, body collision, bounded streaming and versioned two-slot saves are integrated.
- Thirteen original 32×32 sRGB tiles, face-specific grass/timber layers, repeating merged faces, six linear-light mip levels and PNG export are integrated. The exported contact sheet was inspected. Tests decode all thirteen PNGs and compare them byte-for-byte with runtime pixels.
- Nine streaming regressions cover stale revisions, bounded queues/uploads, duplicate work, halo invalidation, unload and shutdown. A dirty-resident eviction leak was corrected.
- Spawn/fall recovery checks trees and construction. Seed 396 reproduces the former tree-spawn intersection; its regression passes. Unsafe flight-to-walk transitions are rejected with player feedback.
- Presentation counters now exclude skipped acquisitions. Skipped/unfocused rendering is throttled. Controlled renderer failures attempt to save the CPU world before exit; this is not panic/power-loss recovery.
- `cargo fmt --check`: passed.
- `cargo test --locked`: **45 passed, 0 failed, 1 explicit GPU skip**.
- `cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture`: **1 passed**, including material mip upload validation on the native GPU.
- `cargo clippy --locked --all-targets -- -D warnings`: passed without warnings.
- `cargo build --release --locked`: passed.
- Both PowerShell entry points parsed and ran. The launcher retains diagnostics; benchmark reports and exports preserve existing destinations.

The original 40-byte vertex assertion failed when adding the texture-layer field intentionally changed the contract to 44 bytes. The test now verifies size, normal/color/layer offsets and GPU attribute format. Geometry-area and intended-behavior assertions were not weakened.

## Native evidence and limits

Earlier actual-window testing entered the world, visibly broke `[63,46,-67]`, placed grass there, saved and closed. A subsequent font-enabled build showed readable menu text; missing menu-arrow glyphs were then removed. The retained test save records seed 728491, that edit, and player eye `[64.5,46.60417,-63.5]`.

The current textured executable loaded this world with `saved_world=true`, submitted **1,800 presentations in 30 seconds**, reported zero skipped acquisitions and exited 0 with an empty error log. Its last rolling sample was approximately 60 submissions/s and p95=16.91ms. This is a vsync-paced title scene at the default 1440×900 window size, not the proposed 1080p traversal/build benchmark, GPU execution time or physical display timing.

While that process was alive, a second executable using the same save directory exited 1 with an explicit world-lock diagnostic. This verifies the runtime writer guard and launcher error path. Both save slots remain retained.

Raw logs: `logs/20261020-080831-e81d7824-stdout.log` and empty stderr; second-writer error in `logs/20261020-080848-5a1f524e-stderr.log`. Earlier untextured 12-second startup submitted 720 presentations. Logs remain local.

Fresh visual retained-block/position re-entry, mouse editing, resize, long traversal, all textured face orientations and distance transitions, and controlled surface-loss/fatal-save injection remain **unverified**. The native computer-use helper fails before initialization with `failed to write kernel assets: The system cannot find the path specified. (os error 3)`, including after a reset/retry. No successful current screenshot or interactive acceptance is claimed.

## Measurements and provenance

[CPU-BASELINE.md](CPU-BASELINE.md) summarizes three sequential release invocations. Each warms six scenarios once and measures five runs per scenario. All six geometry digests match across all three invocations. The 12-chunk terrain route takes about 32 ms total on one thread; the checkerboard chunk takes about 18 ms and produces 18.75 MiB of mesh payload. CPU work duration, GPU time, game FPS and total memory remain separate claims.

Raw reports: `evidence/cpu-20261020-01.json`, `-02.json`, `-03.json`, each with a `.provenance.json` sidecar containing source/binary SHA256 hashes, dirty-tree disclosure, compiler and hardware. The source had no commit at measurement time; hashes identify it without inventing one.

Verified hardware: i7-9700, 8 cores/8 logical; 51,409,334,272 RAM bytes; GTX 1660 Ti / driver 32.0.16.1742; Windows 11 Home 10.0.26300; High performance power plan. About 18.5 GB free on C: was observed. Background apps, thermals and CPU affinity were not controlled.

Rust/Cargo 1.97.1, MSVC target; wgpu 30.0.1, winit 0.30.13, egui 0.37.0 with default fonts, glam 0.34.1, bytemuck 1.25.2, pollster 1.0.1 and png 0.18.1; full graph in Cargo.lock.

Measured executable SHA256: `8987752CF362DBC4CB780B6DDC31AD8D78C54DCCE60ADC632E009E9B971DA8AB`.

## Ownership and next work

The [stage/phase tracker](PROJECT-TRACKER.md) preserves S0–S8 and the full objective. `world` contributed terrain, streaming regressions, CPU measurements tooling and materials; `renderer` contributed presentation and independent player/lifecycle review; `evolution` contributed primary research/product criteria and the separate governance candidate. Root reviewed, integrated and tested their output. Intermittent agent usage limits did not prevent local progress.

Next: finish native interactive acceptance; measure upload bytes/latency, streaming, GPU time and memory; establish persistent simulation authority and an early meaningful survival/combat/building/exploration loop. Ecology, creatures, machines, progression, audio, controller/accessibility, mods, multiplayer, world scale/LOD, production releases, external playtests and pitch remain open. Public GDK research does not establish console support.

## Section 0 completion audit

This is scored against the full requested result. FAIL rows prevent a production/completion claim, while the checkpoint evidence above remains valid.

| # | Score | Evidence or gap |
|---|---|---|
| 1 | FAIL | Full production game and pitch incomplete |
| 2 | PASS | Current implemented dependencies resolved and exercised |
| 3 | PASS | Current format/tests/lint/GPU validation/release build clean |
| 4 | FAIL | Regressions pass; full interactive/adversarial/lifecycle matrix incomplete |
| 5 | PASS | README, architecture, measurement/material contracts and tracker synchronized |
| 6 | PASS | Current local boundaries, save validation/locking and no-overwrite behavior reviewed |
| 7 | FAIL | CPU baseline measured; full frame/GPU/streaming/resource budgets open |
| 8 | PASS | Declared toolchain and library graph respected |
| 9 | FAIL | Coherent source checkpoint; production installation/update/recovery package incomplete |
| 10 | PASS | Hardware/storage considered; authority and user data preserved |
| 11 | FAIL | Locked local build/hashes; independent clean build/release reproducibility open |
| 12 | FAIL | Original material provenance; full dependency/license/advisory/name review open |
| 13 | PASS | Bounded specialist ownership and root integration recorded |
| 14 | PASS | Commands, provenance, review artifacts, limits and next actions recorded |
| 15 | PASS | Self-audit below separates evidence classes |

Source tests and native GPU validation passed for the textured implementation.
The executable loaded the retained save, presented the scene, closed and rejected a concurrent writer.
Exported materials were decoded against runtime bytes and the contact sheet was inspected.
Three CPU runs retained matching geometry with exact source/binary provenance.
Interactive re-entry and complete presentation quality remain unverified because current UI control failed.
Production gameplay, integrated budgets, release/license gates, external playtests and pitch readiness remain incomplete.

Frozen law remains `MrWizard94-Compile/Agents-Constitution@0010a8353b00754fad31a30ed24327d3b6d05425` (6.16.4) for this continuing invocation. Byte-exact load/source and accepted workflow were verified earlier.

**Skill evolution:** evidence-budget/platform-claim clarification, candidate 6.17.0, validated apply-ready patch **staged, not propagated**. It does not govern this invocation retroactively; the continuation does not create another obligation.
