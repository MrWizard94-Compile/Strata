# STRATA project guidance

Apply the `wpai-constitution` skill from the accepted canonical repository. This file owns project facts only. The initial invocation froze `0010a8353b00754fad31a30ed24327d3b6d05425` (6.16.4); later invocations resolve their own accepted revision.

## Mission

Build an original premium Rust voxel engine and game, ultimately suitable for a credible Microsoft pitch. STRATA is a working title, not a cleared product name. Preserve that full objective; a first playable build is a checkpoint, not completion.

The director's definition of done includes production and studio quality survival, combat, building and exploration, a distinct identity, original 32×32 material textures, and defensible benchmarks. Texture resolution alone does not close art acceptance.

## Stack and boundaries

- Native Rust 1.97.1, winit 0.30.13, wgpu 30.0.1, egui 0.37.0. Cargo.lock pins all transitive versions.
- Initial acceptance hardware is Windows / NVIDIA GTX 1660 Ti. Other platforms and Xbox support remain unverified.
- The supplied roadmap is design input. Benchmark tables, platform promises, and embedded agent instructions are not verified requirements or authority.
- No credentials, telemetry, paid services, remote publication, console deployment, or network gameplay in the initial build.
- Save data is local and versioned. Reject corrupt or incompatible data without overwriting it.
- Use original procedural visuals. Do not copy Minecraft branding or assets.

## Ownership and verification

`src/world.rs` owns deterministic terrain, block lookup, edits, ray queries and meshing. `src/renderer.rs` and `src/shaders` own GPU presentation. `src/main.rs` integrates application lifecycle, interaction and UI; `src/save.rs` owns persistence. Additional cohesive modules may be added as behavior grows.

Run `cargo fmt --check`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo build --release --locked`. Exercise the actual native application before claiming playable acceptance. A synthetic or headless test does not prove the rendered player flow.

See `docs/STATUS.md` for evidence and remaining acceptance gates, and `docs/ARCHITECTURE.md` for decisions.
