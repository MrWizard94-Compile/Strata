# Strata pull request update

2026-11-03. Development tip used for every merge: `22f0ee6` (`Restore snow, clay, crystal, and deep basalt`, pull request #8). That commit is generator 3: snow caps, clay, crystal clusters, and deep basalt. Generator 1 and 2 saves stay rejected and are not rewritten.

#4 was left alone. Its CI was already re-running.

No pull request was merged, approved, tagged, or released. GitHub settings were not changed. History was not rewritten. Every push was a normal fast-forward.

The local integration branch `grok/local-combined-do-not-push` (`d65381f`) was not pushed. It lives in `C:\Users\Bulkl\.grok\worktrees\software-rust-based-voxel\combined-local`.

## Pull request #5 — packed quads

**What it does.** Greedy chunk faces are packed into 8-byte quads. GPU uploads are sliced to the remaining byte budget, aligned to one quad. The destination buffer is allocated before the timed copy. The logical 44-byte corner used by digests is not uploaded.

**Branch.** `cursor/packed-quad-upload-budget-f0c1`
**Tip.** `2d44c052d90ddb7567b63c4935aefb4f135700e9`
The tip is the merge of `origin/development`. Remote `git ls-remote` matches. There were no follow-up commits.

**Conflicts with development.** Git auto-merged. `src/world.rs` still has generator 3, the #8 golden chunk hashes, and snow, clay, crystal, and basalt. The packed-quad mesher is still the branch's own change. `golden_chunks_match_platform_independent_hashes` passed, so chunk block bytes did not move.

**What I changed.** Nothing after the merge commit.

**Checks**, in that worktree, `CARGO_BUILD_JOBS=2`:

- `cargo fmt --check`: passed
- `cargo test --locked`: 114 passed, 0 failed, 1 ignored (`native_gpu_validates_all_world_shader_pipelines`)
- `cargo clippy --locked --all-targets -- -D warnings`: passed
- `cargo build --release --locked`: passed

**Review.** No correctness bug was fixed. Left as notes:

- `src/world.rs` is about 2,499 lines and `src/streaming.rs` about 1,253. Meshing stays in `world.rs` because that module owns it. The files were not split.
- `docs/STATUS.md` still has an older sentence that the 44-byte layout is unchanged, and a "next" line that still says to split oversized uploads. The later packed-quad section is the current GPU contract.
- `Renderer::upload_slice` cancels a slice that is not a multiple of 8 bytes. Streaming still advances `uploaded_bytes` after the callback returns. Production slices are produced with `quad_aligned`, so this path is not hit by the current uploader.
- `DecodedQuad::normal` indexes the axis directly. The mesher only emits axes 0, 1, and 2. The shader treats anything else as Z.
- Chunk positions are still `origin as f32 + local as f32`. That can diverge from one integer-to-float cast past 2^24. That limit is already documented.

**Test pins.** None.

**Ready to merge.** Yes, onto current `development`. This is source QA. The ignored GPU pipeline test was not run, so the packed shader was not device-validated here. This was not a windowed playtest, and the new commit's CI was not waited on. The committed Linux CPU evidence file is from before generator 3, so its route mesh digests are historical.

## Pull request #9 — transparent water and leaves

**What it does.** Leaves are drawn in a depth-writing cutout pass (texel alpha below 0.5 is discarded, culling off). Water is drawn after that, alpha blended (`mix(0.55, 0.9, fresnel)`), depth tested, and without a depth write. Chunks are sorted back to front by center for the water pass. One upload buffer is ordered opaque, then leaf cutout, then water, and nothing in it is drawn until the last slice. Water stays non-solid. This branch also carries its own copy of the packed-quad commits (different hashes from #5).

**Branch.** `cursor/transparent-water-leaves`
**Merge.** `a1543f51b222834c7d60c06b36db3b5b869bfe25`
**Tip.** `10715ac13569421cd787ea2c19e10c60d8942c9a`
Remote matches.

**Conflicts with development.** Only `README.md` conflicted. The resolved text keeps sliced 8-byte uploads and development's source-check and playable-build workflow text. Generator 3 terrain and save rules stayed. The architecture text no longer says water is visually opaque.

**What I changed.** Follow-up `10715ac`, "Retarget mesh pins for generator 3 terrain", in `docs/BENCHMARKING.md`, `docs/STATUS.md`, `src/benchmark.rs`, and `src/world.rs`.

**The two generator failures.** Both were mesh pins from generator 2. Face rules did not change. Golden chunk hashes passed, so block bytes did not move. The new numbers are the same generator 3 terrain with the leaf/water face rule.

1. `an_ocean_chunk_keeps_water_out_of_the_opaque_and_cutout_ranges`, seed `728491`, chunk `(-8, 0, -8)`. Old `(opaque, cutout, water, bytes)`: `(1485, 0, 46, 12248)`. New: `(1488, 0, 46, 12272)`. The chunk has no clay, crystal, snow, or leaves. The y=0 basalt floor was already present. Generator 3 adds 56 basalt intrusions above that floor. Those split stone merges by 3 quads. The 46 water quads stay in the blended range.

2. `artificial_datasets_match_analytic_surfaces_and_real_edits_change_the_route`. Seeded route, old: 13,551 quads, 108,408 bytes, 27,102 triangles, digest `d732c9d403bf9a68` (opaque 10,792, cutout 2,759, water 0). New: 13,416 quads, 107,328 bytes, 26,832 triangles, digest `eea13108cbbc62ee` (opaque 10,713, cutout 2,703, water 0). Edited route, old: 13,589 quads, 108,712 bytes, digest `553186af4f85dbdf`. New: 13,462 quads, 107,696 bytes, digest `ce065bae1978a3cb` (opaque 10,759, cutout 2,703, water 0). The unedited route has 5,088 snow blocks, 87 crystal, basalt floor plus intrusions, no clay, and no water. Snow covers grass and removes 56 leaf faces that now touch snow. Crystal and basalt seams also change opaque merges. Checkerboard digest `9b90a6ba4714f004` and isolated-solid digest `41be41bea5c6c402` stayed put.

**Checks** after the pin commit:

- `cargo fmt --check`: passed
- `cargo test --locked`: 117 passed, 0 failed, 1 ignored
- `cargo clippy --locked --all-targets -- -D warnings`: passed
- `cargo build --release --locked`: passed

The run before the pin commit was 115 passed, 2 failed, 1 ignored.

**Review.** No logic bug was fixed. Water stays non-solid. Opaque, then depth-writing leaf cutout, then blended water. A chunk is not drawn until the last upload slice.

Left as notes:

- `src/renderer.rs` is about 1,071 lines, `src/streaming.rs` about 1,254, `src/world.rs` about 2,763. Not split.
- Triangle patterns are duplicated in Rust and `world.wgsl`.
- Water is sorted by chunk center, so water inside one chunk blends in mesh order.
- Fog changes water RGB. Leaf alpha is the cutout test, and water alpha is the fresnel mix.
- Water and leaf back faces are drawn. The normal is not flipped.

**Ready to merge.** The branch itself is healthy against current `development`. Merge it after #5. It carries an older copy of the packed-quad work, and combining it with #5 conflicts in the renderer, shader, streaming, world, and several docs. The resolution that keeps both behaviors is #9's three-pass drawing on top of the 8-byte quad format. See the combined section. Same limits as #5: no GPU pipeline run here, no windowed playtest, CI on the new commits not waited on.

## Pull request #10 — PNG block textures

**What it does.** Block tiles load from `assets/<namespace>/textures/block/*.png` through `assets/block-textures.manifest`, `build.rs`, `src/texture_pack.rs`, and `src/block_textures.rs`. The shipped `strata` PNGs match the procedural tiles, including leaf alpha. The quad stays 8 bytes. The layer field is 10 bits. This branch's history also contains the leaf/water commit and the older packed-quad commits.

**Branch.** `cursor/png-block-textures`
**Merge.** `ddd350627f0797a2fe95892d0e8168d0e10b41ee`
**Tip.** `19e85c9b9e0ff5219c51aa60db6b9116df2c3d65`
Remote matches.

**Conflicts with development.** Only `README.md`. Resolved the same way as #9: sliced uploads plus development's CI and package text. Generator 3 placement stayed. Snow, clay, crystal, and basalt still bind to the pack PNGs that were already on the branch. Water is documented as the blended pass.

**What I changed.** Follow-up `19e85c9`, "Pin generator 3 route and ocean mesh counts", in `docs/BENCHMARKING.md`, `docs/STATUS.md`, `src/benchmark.rs`, and `src/world.rs`. Golden chunk hashes already matched generator 3 and were not edited. The route and ocean numbers are the same ones as #9, for the same reason: generator 3 snow, crystal, and basalt change meshes. The texture pack does not. Ocean chunk `(-8, 0, -8)` is 1,488 opaque, 0 cutout, 46 water, 12,272 bytes. The three extra opaque quads are basalt. Checkerboard and isolated-solid digests are unchanged.

**Checks** after the pin commit:

- `cargo fmt --check`: passed
- `cargo test --locked`: 121 passed, 0 failed, 2 ignored (the PNG rewrite test, and the native GPU pipeline test)
- `cargo clippy --locked --all-targets -- -D warnings`: passed
- `cargo build --release --locked`: passed

Before the pin commit: 119 passed, 2 failed, 2 ignored.

**Review.** No missing texture, dropped generator rule, or dropped save rejection was found. The two failures were the generator 2 mesh pins above.

Left as notes:

- `world.rs` is about 2,961 lines, `main.rs` about 1,929, `streaming.rs` about 1,311, `renderer.rs` about 1,121. Not split.
- `quad_material` can fall back to layer 0 outside debug if a bind is missing. The shipped blocks are covered by a test.
- Water sorting is still by chunk center.

**Ready to merge.** Healthy against current `development`. Merge it after #9 (and therefore after #5). It already contains the leaf/water behavior plus the pack. Merging it before #9 makes #9 a second copy of the same pass split. Same evidence limits: no GPU run, no windowed playtest, CI not waited on.

## Pull request #11 — reproducible player zips

**What it does.** Player zips get commit-time timestamps, fixed modes, and no extra fields. Release builds set `SOURCE_DATE_EPOCH`, remap workspace, Cargo home, and rustup paths, and pass `/Brepro` on MSVC. The playable-build workflow builds and zips twice after deleting `target` and fails if the hashes differ. The publish job does not check out the repo; it passes `GH_REPO` and `--repo` to `gh`. A tag name containing `-` is a prerelease. The zip writer test does not depend on Windows newline translation.

**Branch.** `cursor/reproducible-player-zips-5031`
**Tip.** `16ef6c9e359ff1fe3d1b98b94da8e77e7bf03384`
The tip is the merge commit. Remote matches. No follow-up commits.

**Conflicts with development.** Clean merge. Git auto-merged `README.md` and `docs/STATUS.md`. Generator 3 behavior stayed. The zip workflow stayed.

**Checks:**

- `cargo fmt --check`: passed
- `cargo test --locked`: 103 passed, 0 failed, 1 ignored
- `cargo clippy --locked --all-targets -- -D warnings`: passed
- `cargo build --release --locked`: passed
- `python -m unittest tools/test_player_zip.py` with Python 3.10.11: 4 tests, OK. `py -3` (Python 3.14.3) also passed those 4 tests.

**Review.** No definite nondeterminism, path traversal, checkout leak, newline bug, or dropped CI check was patched. I re-read `tools/repro_build.py`: remap pairs are sorted by prefix length after deduplication, so `PYTHONHASHSEED` does not reorder `--remap-path-prefix`. Windows `tar` can extract a zip from this writer. Slash and backslash entry names are rejected.

Left as notes:

- Checksum assembly is copied between the `checksums` and `publish` jobs.
- The path scan is case-sensitive and looks for exact workspace, Cargo, and rustup prefixes. A same-runner hash compare does not prove another machine would match.
- Package jobs still call `git`. The publish job is the checkout-free one.
- `write_zip` does not reject `.` or `..`. The package CLI only writes `strata.exe`, `strata`, and `PLAYING.md`.
- README still describes the Windows release step as plain `cargo build --release --locked`. CI runs `python tools/repro_build.py`.

**Ready to merge.** Yes, onto current `development`. `release.yml` was not executed. Not verified here: the double build after deleting `target`, `/Brepro` plus remap making Windows and Linux zips match across machines, `package-windows.ps1` on a runner, or `gh release create` without a checkout. An earlier dry run is not this commit.

## Pull request #7 — draft playtest checklist

Listed with the open pull requests. It is still a draft. The branch was 3 commits behind `development` and the merge was clean.

**Branch.** `cursor/playtest-m1-checklist-642e`
**Tip.** `a679a167daf0bc2fadfc2c01a8dc6aae033189c6`
Remote matches. The diff against `origin/development` is only `docs/PLAYTEST-M1.md` (394 lines).

Cargo was not re-run on this branch. The code is `development`. The checklist steps were not reviewed line by line. It already names Alpine snow, Lumen crystal, and Basalt. Leave it draft until someone walks the list.

## Combined suite

Local branch `grok/local-combined-do-not-push` at `d65381f`. Not pushed. Order used:

1. `origin/development`
2. #11 — clean
3. #5 — conflict in `docs/STATUS.md` only. The audit rows now include both the player-zip workflow note and the packed-quad GPU/CPU limits.
4. #9 — conflicts in `README.md`, `docs/ARCHITECTURE.md`, `docs/BENCHMARKING.md`, `docs/STATUS.md`, `docs/STREAMING.md`, `src/renderer.rs`, `src/shaders/world.wgsl`, `src/streaming.rs`, and `src/world.rs`.
5. #10 — conflicts in `docs/BENCHMARKING.md`, `docs/STATUS.md`, `src/benchmark.rs`, and `src/world.rs`.

How #5 and #9 were kept together: #9's renderer, shader, streaming, and world files are the 8-byte quad format plus the cutout and water passes. A direct diff of the two tips shows #5's copy of those files is that same quad format without the passes. The combined tree uses #9's code for those four files. Docs keep #5's newer Linux VM test note (110 passed on `4afa708`), #9's pass description and generator 3 mesh figures, and #11's zip audit row.

How #10 was kept: the pack, `build.rs`, and the 10-bit layer field came in with the merge. The shader on the combined tree has both `(input.word1 >> 8u) & 1023u` and a cutout bit at bit 22. The benchmark and ocean-chunk conflicts were the same integers with different thousands separators and comments. Both comments were kept: 56 basalt intrusions, three extra opaque quads, 46 water quads still blended.

**Combined checks**, same worktree:

- `cargo fmt --check`: passed
- `python -m unittest tools/test_player_zip.py`: 4 tests, OK
- `cargo test --locked`: **121 passed, 0 failed, 2 ignored**
- `cargo clippy --locked --all-targets -- -D warnings`: passed

Ignored tests: `materials::tests::write_shipped_block_pngs` and `renderer::tests::native_gpu_validates_all_world_shader_pipelines`.

`cargo build --release --locked` was not run on the combined tree. Each of #5, #9, #10, and #11 reported a passing release build on its own updated branch.

No test failed in the combination. The conflicts were real, and they were in docs and in the shared mesh/render files. They do not show up when each pull request is compared with `development` on its own.

## Recommended merge order

1. **#11.** Independent of the mesh. Clean on today's `development`.
2. **#5.** Also clean on today's `development`. If #11 lands first, the only conflict is the `docs/STATUS.md` audit rows. Keep both the zip note and the packed-quad evidence limits.
3. **#9.** After #5 is on `development`, merge `development` into #9 again (merge commit, no rebase) and resolve the renderer, shader, streaming, world, and doc conflicts the way the local combination did: three draw passes, 8-byte quads, generator 3 pins. Then merge #9.
4. **#10.** After #9. The remaining conflicts seen here were the texture-pack section, the same generator 3 numbers, and comment wording. The 10-bit layer and the cutout bit both have to survive.
5. **#7** whenever the draft checklist is accepted. It does not block the four above. It is already even with `development` plus `docs/PLAYTEST-M1.md`.

Do not merge #10 before #9. #10 contains the leaf/water work, and #9 would then be a second copy of that pass split.

Merging #5 or #11 will put #9 and #10 behind `development` again. Refresh them with a merge commit before merging them. Do not rebase and do not force-push.

## Evidence limits

Source checks above are not CI on the new commits, not a GTX 1660 Ti playtest, and not a published zip. The main checkout at `C:\WPAI\Software\Rust Based Voxel` is still local `development` at `4afa708`, one commit behind `origin/development`. This report is not committed.

Frozen constitution for this call: `MrWizard94-Compile/Agents-Constitution` `63d99cdc72a1c18b5aa91b9336d02fd30126407b` (6.18.0). Loader version matches. Byte-exact freeze check passed.

Skill evolution: `VCS-INTEG-001` (a hosting provider's clean flag is not a current base, and pairwise pull requests need an unpublished combined check) is in pull request https://github.com/MrWizard94-Compile/Agents-Constitution/pull/42. It is not on canonical `main` and does not govern this call.
