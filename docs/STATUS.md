# Current checkpoint

2026-10-21. **Goal active.** STRATA has a native editable-world foundation, original 32×32 materials and reproducible CPU meshing evidence. The requested production and studio grade survival/combat/building/exploration game and Microsoft pitch remain incomplete. The 2026-10-20 counts below remain that checkpoint. Day/night evidence is the next section.

## Day/night cycle

Playing advances a 20-minute day on the fixed 60 Hz simulation tick. The title, pause menu, settings, and focus loss all go through the same step function and freeze that clock. Schema 1 generator 2 stores `time_of_day` in `[0, 1)` (midnight is 0). A generator 2 save without the field loads at 08:00. Generator 1 files are rejected and left untouched. Non-finite values, negatives, and exactly 1.0 are rejected and the file is kept. Sun direction, sky, fog, ambient, and sun color are appended camera uniforms. Night clouds and water use the same linear lighting as the terrain. The 44-byte vertex layout is unchanged.

Linux verification for this change, Rust 1.97.1:

- `cargo fmt --check`: passed.
- `cargo test --locked`: **80 passed, 0 failed, 1 ignored** after rebasing onto generator 2 (`8af9548`). The generator 2 checkpoint below remains 65.
- `cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture`: **passed** on Mesa lavapipe (software Vulkan from mesa-vulkan-drivers 25.2.8). That is shader-pipeline and material-upload validation, not a GTX 1660 Ti and not a windowed playthrough.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo build --release --locked`: passed.

Unverified: interactive dawn, noon, dusk, and night appearance, mouse play, and the Windows / NVIDIA GTX 1660 Ti acceptance machine.

## Interaction polish

Holding break or place edits on the press, then again after `EDIT_REPEAT_TICKS` (15 simulation ticks, `EDIT_REPEAT_SECONDS` = 0.25 s). Repeat advances inside the same fixed 60 Hz simulation tick as the day clock and the player. A frame can catch up at most `FRAME_HITCH_CAP_SECONDS` (0.1 s), so a hitch cannot burst edits. A frame that drains zero ticks leaves the hold armed, which is what a monitor above the tick rate does on most frames. The hold is cleared when the mouse button is released and when simulation is suspended: pause, title, settings, focus loss, or the material picker. Resuming does not fire a stale repeat. The outline and both edit actions share `REACH_BLOCKS` (5), including a target whose near face is exactly five blocks along −X. Placement still refuses the player's body. Editable height remains -30 through 158, with the same messages.

The hotbar has eight slots. The default is meadow grass, limestone, timber, canopy, river sand, terracotta, lumen crystal, and basalt. Press E to open a material grid of every placeable block from the registry (`Block::all` filtered by `is_placeable`). Air and water are omitted, so a new solid block shows up without a separate list. Coal, iron, gold, and diamond ore appear in that grid. Ore blocks in the material picker are a deliberate, approved choice. 1–8 and the wheel select a slot; a click fills that slot. Esc or E closes the grid and releases the mouse while it is open. While the picker is open it freezes the day clock and the player. F4 still reaches the game while the picker is open, so the debug hour skip is not swallowed.

Schema 1 is one `PlayerSave` plus one world save (`Snapshot`). `time_of_day`, `hotbar`, and `selected_slot` are optional under `deny_unknown_fields`. A save that omits all three loads 08:00, the default bar, and slot 0. Unknown ids, non-placeable ids, a slot outside 0–7, JSON null for those fields, and a generator newer than `GENERATOR_VERSION` are rejected without rewriting the file. Loadable fixtures are produced by `SaveStore::save` and checked against the live schema and generator constants.

Linux verification for this change, Rust 1.97.1, on day/night `188b322`:

- `cargo fmt --check`: passed.
- `cargo test --locked`: **99 passed, 0 failed, 1 ignored** after rebasing onto CI `4bbd3c6`. Hold-to-repeat and held-arrow turning both go through `advance_play_frame` at 144 Hz, 60 Hz, and a capped hitch.
- `cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture`: **passed** with the Vulkan loader pointed at Mesa lavapipe (`libvulkan_lvp.so`). That is shader-pipeline and material-upload validation, not a GTX 1660 Ti and not a windowed playthrough.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo build --release --locked`: passed.

Unverified: windowed play of hold-to-repeat, the material picker, and the Windows / NVIDIA GTX 1660 Ti acceptance machine. This environment cannot create a game window surface.

This note does not replace the native evidence below. Windowed play of the new controls was not run on the Windows acceptance machine.

The canonical local project is `C:\WPAI\Software\Rust Based Voxel`, an independent Git root. `origin` now points to `https://github.com/MrWizard94-Compile/Strata.git`; fetch succeeded. The repository is public. The director subsequently authorized committing and pushing all project changes. Source checkpoints and a Windows CI workflow are being delivered on development; this is not a production release.

## Source and artifact verification

- Deterministic generator 2 terrain is integrated: plains, forest, desert, ocean, and beach, with sea-level water, caves, grid-anchored trees, and coal, iron, gold, and diamond. Sparse edits, greedy meshing, DDA targeting, walking/jump/flight, body collision, bounded streaming and versioned two-slot saves are integrated. Generator 1 saves are rejected and left untouched.
- Seventeen original 32×32 sRGB tiles, face-specific grass/timber layers, repeating merged faces, six linear-light mip levels and PNG export are integrated. The exported contact sheet was inspected. Tests decode all seventeen PNGs and compare them byte-for-byte with runtime pixels.
- Eighteen streaming regressions cover stale revisions, bounded queues, byte/time/count admission, deferred work, duplicate work, halo invalidation, unload and shutdown. A dirty-resident eviction leak was corrected.
- Spawn/fall recovery checks a planted trunk and player construction. Seed 396 at column `(64, -64)` is not a generator 2 trunk site, so that regression plants the wood. Seed `20261021` grows a natural trunk at `(-321, -354)`, ground 35, five blocks tall; that test uses no edits. Unsafe flight-to-walk transitions are rejected with player feedback.
- Presentation counters now exclude skipped acquisitions. Skipped/unfocused rendering is throttled. Controlled renderer failures attempt to save the CPU world before exit; this is not panic/power-loss recovery.
- `cargo fmt --check`: passed.
- `cargo test --locked`: **65 passed, 0 failed, 1 explicit GPU skip** on 2026-10-21 after generator 2. The ignored test is the native GPU pipeline check.
- `cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture`: **1 passed**, including material mip upload validation on the native GPU.
- `cargo clippy --locked --all-targets -- -D warnings`: passed without warnings.
- `cargo build --release --locked`: passed.
- All three PowerShell entry points parsed; CPU benchmark, native launcher and streaming-trace wrapper ran. The launcher retains diagnostics; benchmark reports and exports preserve existing destinations.

The original 40-byte vertex assertion failed when adding the texture-layer field intentionally changed the contract to 44 bytes. The test now verifies size, normal/color/layer offsets and GPU attribute format. Geometry-area and intended-behavior assertions were not weakened.

## Native evidence and limits

The runs in this section are the 2026-10-20 checkpoint. They predate generator 2. The retained seed 728491 save is a generator 1 file: current code rejects it and leaves the bytes in place. Those runs are not evidence that the current generator loads that world.

Earlier actual-window testing entered the world, visibly broke `[63,46,-67]`, placed grass there, saved and closed. A subsequent font-enabled build showed readable menu text; missing menu-arrow glyphs were then removed. The retained test save records seed 728491, that edit, and player eye `[64.5,46.60417,-63.5]`.

The earlier textured executable loaded this world with `saved_world=true`, submitted **1,800 presentations in 30 seconds**, reported zero skipped acquisitions and exited 0 with an empty error log. Its last rolling sample was approximately 60 submissions/s and p95=16.91ms. This is a vsync-paced title scene at the default 1440×900 window size, not the proposed 1080p traversal/build benchmark, GPU execution time or physical display timing.

While that process was alive, a second executable using the same save directory exited 1 with an explicit world-lock diagnostic. This verifies the runtime writer guard and launcher error path. Both save slots remain retained.

Raw logs: `logs/20261020-080831-e81d7824-stdout.log` and empty stderr; second-writer error in `logs/20261020-080848-5a1f524e-stderr.log`. Earlier untextured 12-second startup submitted 720 presentations. Logs remain local.

Fresh visual retained-block/position re-entry, mouse editing, resize, long traversal, all textured face orientations and distance transitions, and controlled surface-loss/fatal-save injection remain **unverified**. The native computer-use helper fails before initialization with `failed to write kernel assets: The system cannot find the path specified. (os error 3)`, including after a reset/retry. No successful current screenshot or interactive acceptance is claimed.

## Current native streaming checkpoint

The current release SHA256 is `4E799BA96150765A1A2A7115490C4BCCD3C3A3E7FE8A0BF224BB8412F0ECE90D`. Two 15-second native runs each submitted 900 presentations with zero skips, exited 0 and produced empty error logs. Both remained on a stationary title camera. The normal world delivered 294 chunks, including empty meshes, in 2.48 seconds from Game start; the checkerboard fixture delivered 294 in 2.50 seconds. These exclude world loading and graphics initialization.

The stress fixture admitted its 19,660,800-byte center mesh alone, with a 7.1273 ms upload callback and 8.1734 ms full admission call. This verifies oversized progress and exposes the soft time limit. A 4 MiB / 2 ms budget does not interrupt one running callback. The source suite covers deferred edits/movement; neither native run needed deferral. Existing trace paths were rejected with their hashes preserved. See [STREAMING.md](STREAMING.md) for raw paths, metrics, fixture reproduction and exact limitations.

The original foundation is local commit `b06c1da`; the current trace sidecars identify subsequent dirty source with exact hashes. The supplied stage/phase scope remains intact.

## Measurements and provenance

[CPU-BASELINE.md](CPU-BASELINE.md) summarizes three sequential release invocations. Each warms six scenarios once and measures five runs per scenario. All six geometry digests match across all three invocations. The 12-chunk terrain route takes about 32 ms total on one thread; the checkerboard chunk takes about 18 ms and produces 18.75 MiB of mesh payload. CPU work duration, GPU time, game FPS and total memory remain separate claims.

Raw reports: `evidence/cpu-20261020-01.json`, `-02.json`, `-03.json`, each with a `.provenance.json` sidecar containing source/binary SHA256 hashes, dirty-tree disclosure, compiler and hardware. The source had no commit at measurement time; hashes identify it without inventing one.

Verified hardware: i7-9700, 8 cores/8 logical; 51,409,334,272 RAM bytes; GTX 1660 Ti / driver 32.0.16.1742; Windows 11 Home 10.0.26300; High performance power plan. About 18.5 GB free on C: was observed. Background apps, thermals and CPU affinity were not controlled.

Rust/Cargo 1.97.1, MSVC target; wgpu 30.0.1, winit 0.30.13, egui 0.37.0 with default fonts, glam 0.34.1, bytemuck 1.25.2, pollster 1.0.1 and png 0.18.1; full graph in Cargo.lock.

Historical CPU-baseline executable SHA256: `8987752CF362DBC4CB780B6DDC31AD8D78C54DCCE60ADC632E009E9B971DA8AB`.

## Pull-request checks and playable packages

2026-10-21. Pull requests into `development` run Windows and Linux source checks. Both jobs run unfiltered `cargo test --locked` (no test-name filter and no OS gate). The Linux job also runs `native_gpu_validates_all_world_shader_pipelines` on lavapipe; that is the only `#[ignore]`d test, and it is the only test the Windows job does not run. The Windows job builds `x86_64-pc-windows-msvc` with the static C runtime from `.cargo/config.toml`, then `package-windows.ps1` checks subsystem 2 (no console window), rejects Visual C++ redistributable imports, unzips a package that must contain only `strata.exe` and `PLAYING.md`, and launches that exe for `--help` and `--export-materials`. A startup failure writes `strata-error.txt` beside the exe. A separate workflow uploads that Windows zip first and a Linux zip as well. The Linux package is built on Ubuntu 22.04, must contain only `strata` and `PLAYING.md` with the executable bit stored in the zip, and that binary is launched for the same two commands. Neither launch opens a window. macOS is not a matrix row. Branch protection and the Actions write permission for releases are owner settings; the workflow files do not apply them.

GitHub run [39545873483](https://github.com/MrWizard94-Compile/Strata/actions/runs/39545873483) on `860c4e8` completed with both jobs successful. Windows `cargo test --locked` reported `55 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out`. The ignored test was `native_gpu_validates_all_world_shader_pipelines`. Linux's unfiltered suite reported the same 55 passed, 1 ignored, and 0 filtered out, then passed that GPU test on lavapipe. `package-windows.ps1` on that runner found no Visual C++ redistributable import (dumpbin listed kernel32, user32, gdi32, ntdll, shlwapi, oleaut32, ole32, shell32, dwmapi, advapi32, api-ms-win-core-synch-l1-2-0, opengl32, dxgi, setupapi, bcryptprimitives, imm32, and uxtheme), launched the unzipped exe, and hashed `strata-windows-x86_64.zip` as `e91ad1e7b912a6d0a14dd92d816fdd5373612a1001ee66029160ece01b23847a`. That launch was `--help` and `--export-materials`, not a displayed window and not play on the GTX 1660 Ti. That count and zip hash are from before `development` `188b322` (day/night and generator 2).

Playable build run [39550868602](https://github.com/MrWizard94-Compile/Strata/actions/runs/39550868602) on `12e81fe` is the dry-run after that rebase. It was a branch push, not a tag, so it built and uploaded artifacts and did not create a Release. Package windows-x86_64 and Package linux-x86_64 succeeded. Windows `cargo test --locked` reported `82 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out`, including `golden_chunks_match_platform_independent_hashes`, `held_arrow_turns_on_each_simulation_tick`, and `startup_failure_is_written_beside_the_chosen_directory`. Linux's unfiltered suite reported the same 82 passed, 1 ignored, and 0 filtered out, then passed the ignored GPU test on lavapipe (`1 passed; 82 filtered out`). `package-windows.ps1` found dumpbin, rejected Visual C++ redistributable imports, required subsystem 2, launched the unzipped exe, and hashed `strata-windows-x86_64.zip` as `c4d87423e1cea773262a481fa2051e482317b46d09f829924ee4a2096c698a77`. `strata-linux-x86_64.zip` hashed as `60756fe6c62eaf516c9589fb629e4836086c2de0081e0d64ff79741a9feafd0c`. Write checksums kept both zip files intact (`skip-decompress: true`), `sha256sum -c` printed OK for each, and uploaded `SHA256SUMS.txt`. Publish GitHub Release was skipped. Artifact names from that run: `strata-windows-x86_64.zip`, `strata-windows-x86_64.zip.sha256`, `strata-linux-x86_64.zip`, `strata-linux-x86_64.zip.sha256`, and `SHA256SUMS.txt`. The Windows launch was still `--help` and `--export-materials`, not a displayed window and not play on the GTX 1660 Ti. Source checks on the same commit are run [39550872382](https://github.com/MrWizard94-Compile/Strata/actions/runs/39550872382): Windows source checks and Linux source checks both succeeded. The branch-only push trigger used for that proof was removed afterward; `workflow_dispatch` input `dry_run` remains for a later manual run from `development`.

Local Linux verification before rebasing onto `188b322`: with `VK_DRIVER_FILES` pointed only at `lvp_icd.json`, the ignored GPU test passed. `cargo fmt --check`, `cargo test --locked` (55 passed, 1 ignored), `cargo clippy --locked --all-targets -- -D warnings`, and `cargo build --release --locked` passed. After the review fixes on the rebased tree, those commands passed again and `cargo test --locked` reported 82 passed, 1 ignored. That local release binary links `libc`, `libm`, and `libgcc_s` and records `GLIBC_2.39` because this machine is Ubuntu 24.04. The Linux zip above is built on the Ubuntu 22.04 runner instead, so its glibc floor is that image's. The GitHub runs are separate results from these local checks.

## Ownership and next work

The [stage/phase tracker](PROJECT-TRACKER.md) preserves S0–S8 and the full objective. `world` contributed terrain, streaming regressions, CPU measurements tooling and materials; `renderer` contributed presentation and independent player/lifecycle review; `evolution` contributed primary research/product criteria and the separate governance candidate. Root reviewed, integrated and tested their output. Intermittent agent usage limits did not prevent local progress.

Next: split oversized GPU uploads, measure observer overhead and moving-world/GPU/memory costs; finish native interactive acceptance; establish persistent simulation authority and an early meaningful survival/combat/building/exploration loop. Ecology, creatures, machines, progression, audio, controller/accessibility, mods, multiplayer, world scale/LOD, production releases, external playtests and pitch remain open. Public GDK research does not establish console support.

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
| 7 | FAIL | CPU baseline and native upload/queue traces measured; full frame/GPU/movement/resource budgets open |
| 8 | PASS | Declared toolchain and library graph respected |
| 9 | FAIL | Coherent source checkpoint; production installation/update/recovery package incomplete |
| 10 | PASS | Hardware/storage considered; authority and user data preserved |
| 11 | FAIL | Locked local build/hashes; Windows CI added, independent result and release reproducibility remain separate gates |
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
