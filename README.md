# STRATA — The Emerald Reach

An original Rust voxel engine and game in active development. **STRATA is a working title.** The full objective is a premium engine-led game spanning a living world, engineering/automation and exploration, eventually suitable for a credible Microsoft pitch. This initial build is not that finished product.

The authoritative local project is `C:\WPAI\Software\Rust Based Voxel`, an independent Git repository. Its verified `origin` is [MrWizard94-Compile/Strata](https://github.com/MrWizard94-Compile/Strata), a public repository. The director authorized committing and pushing source checkpoints; the development branch contains ongoing engineering work, not a production release.

On Windows, download `strata-windows-x86_64.zip` and follow [PLAYING.md](PLAYING.md). You do not need Rust.

![Original 32x32 material contact sheet](docs/assets/materials-32-contact-sheet.png)

## Build and play

Rust 1.97.1 with the MSVC toolchain is required on Windows. The repository pins its toolchain and primary libraries; `Cargo.lock` pins the resolved dependency graph. Cargo downloads required crates from crates.io. A compatible GPU and current graphics driver are needed.

```powershell
Set-Location 'C:\WPAI\Software\Rust Based Voxel'
cargo build --release --locked --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\strata.exe
```

`.cargo/config.toml` links the C runtime statically for `x86_64-pc-windows-msvc` only.

Or run `./run.ps1` from PowerShell. The script builds when necessary, sets the working directory and records startup/runtime diagnostics under `logs/`. Saves live in `saves/emerald-reach`; retain both `world-a.json` and `world-b.json` when backing up. An OS lock rejects a second client opening the same world and releases on process exit; the empty `world.lock` marker can remain.

The client supports `--seed <u64>` for a new world and `--save-dir <path>` for a separate world. An existing world retains its stored seed. `--smoke-seconds <1..120>` launches the real native client and exits after the stated duration for graphics acceptance; this does not automate gameplay acceptance.

One full day lasts 20 real minutes and advances only while playing. Pausing the game pauses the clock. The time is stored in the world save. A generator 3 file with no time opens at 08:00. Generator 1 and generator 2 worlds are rejected and left unchanged.

| Control | Action |
|---|---|
| Enter | Enter world / resume |
| WASD / mouse | Move / look |
| Space / Shift | Jump / sprint |
| F | Toggle creative flight |
| Space / Left Ctrl in flight | Rise / descend |
| Left click or X | Break the targeted block. Hold the mouse button to repeat |
| Right click or C | Place the selected material. Hold the mouse button to repeat |
| 1–8 / mouse wheel | Select a hotbar slot |
| E | Open or close the material picker for the selected slot |
| Arrow keys | Turn camera without mouse |
| F5 | Save world |
| F3 | Toggle frame-time, clock, and world statistics |
| F4 | With the F3 overlay open, skip the clock forward one hour |
| Escape | Close the material picker, or pause / resume |

## Verification

```powershell
cargo fmt --check
cargo test --locked
cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

The GPU test is opt-in on a developer machine because it needs a Vulkan adapter. Running it validates shader pipelines, not the window, input, or save lifecycle. The Linux source-check job runs that same test on lavapipe (software Vulkan). Native playtest evidence and remaining gates are recorded in [docs/STATUS.md](docs/STATUS.md).

Flight is collision-free for development exploration; move clear of solid terrain before toggling back to walking. New spawns and fall recovery test the entire body against trees and construction.

## Materials and CPU benchmarks

The renderer samples seventeen original **32×32** sRGB material tiles, including grass top/side/underside, timber endgrain, and coal, iron, gold and diamond ore. Tiles repeat per block across merged faces. Six mip levels filter distant surfaces. These are initial art assets, pending full in-world and director review.

`--export-materials <new-directory>` exports the exact runtime tiles as individual PNGs, a contact sheet and a manifest. The directory must not already exist and its parent must exist. Exported edits are not yet hot-reloaded into the game.

Run `./benchmark-cpu.ps1` for a release CPU meshing report plus source/binary hashes and hardware provenance. Use `-ReportPath` for an explicit new filename; `-SkipBuild` is only for a binary already verified against the current source. See [the measurement contract](docs/BENCHMARKING.md). The fixed six-scenario CPU workload does not measure game FPS or total memory.

## Project records

- [Stages, phases, micro-agent ownership and gates](docs/PROJECT-TRACKER.md)
- [Implemented architecture and known limits](docs/ARCHITECTURE.md)
- [Evidence and current checkpoint](docs/STATUS.md)
- [Original material design and provenance](docs/MATERIALS.md)
- [Primary research and experiment gates](docs/RESEARCH.md)
- [Game pillars and pitch acceptance criteria](docs/PRODUCT-BRIEF.md)

Source is private/unpublished project work. No general redistribution license has been selected. Third-party crates retain their own licenses; dependency license/advisory review is a separate release gate.

## Streaming diagnostics and CI

`./trace-streaming.ps1 -TracePath evidence/new-trace.jsonl -SmokeSeconds 30` builds and records a native run with exact source/binary/hardware provenance. It starts on the title scene. See [STREAMING.md](docs/STREAMING.md) for admission limits, reproducible stress fixtures, observed costs and measurement limits.

The [source-check workflow](.github/workflows/ci.yml) runs on every pull request into `development`, on pushes to that branch, and on `v*` tags. It does not run again for a push of a pull-request branch. Windows and Linux both run the same unfiltered `cargo test --locked` command, with no test name, `--lib`, or `--skip`. Windows then runs strict clippy and `cargo build --release --locked --target x86_64-pc-windows-msvc`. That target is the one `.cargo/config.toml` builds with a static C runtime. The zip is made from `target\x86_64-pc-windows-msvc\release\strata.exe`. That job then checks the exe is a Windows GUI program, imports no Visual C++ redistributable DLL, launches the unzipped `strata.exe` (help text and a material export, with no window), and checks the zip contains only that exe and `PLAYING.md`. Linux runs format, test, clippy, and a release build, and runs the ignored GPU test against lavapipe. Neither job is interactive play on the acceptance machine.

The [playable-build workflow](.github/workflows/release.yml) runs when someone starts it by hand, and when a `v*` tag is pushed. The primary file is `strata-windows-x86_64.zip` (the release exe, static C runtime, and `PLAYING.md`). `strata-linux-x86_64.zip` is the extra Linux build: that release binary plus `PLAYING.md`. A manual run keeps those files for 30 days. Set **dry_run** on that manual run to upload the zips and `SHA256SUMS.txt` without creating a Release. A version tag without dry_run attaches them to a GitHub Release, with the Windows zip first. Another platform is an extra matrix row in that workflow; none is enabled until it has been built. There is no Mac package. See [PLAYING.md](PLAYING.md).

## Stopping a broken change

A green workflow does not stop a merge by itself. GitHub only blocks the merge after an administrator requires the checks. An agent cannot turn that on.

1. Open this repository on GitHub as an administrator.
2. Go to **Settings → Rules → Rulesets → New ruleset → New branch ruleset**. (The older page is **Settings → Branches → Add branch protection rule**.)
3. Target the `development` branch. Turn on **Require a pull request before merging**.
4. Turn on **Require status checks to pass before merging**. Search for these names after they have run at least once, and add both:
   - `Windows source checks`
   - `Linux source checks`
5. Save the ruleset.

Publishing a Release needs a second setting. The playable-build workflow asks for write access only on the job that runs for a `v*` tag. If the repository caps Actions at read-only, that job fails and the zip artifacts from the package jobs are still produced.

1. **Settings → Actions → General → Workflow permissions**.
2. Choose **Read and write permissions**.
3. Save.

Do not mark the playable-build jobs as required pull-request checks. That workflow does not run on pull requests.
