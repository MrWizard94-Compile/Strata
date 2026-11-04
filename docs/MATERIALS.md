# Original 32×32 material set

The material generator in `src/materials.rs` authors seventeen opaque 32×32 pixel albedo tiles for STRATA, plus an eighteenth magenta/black checker for `strata:missing`. Every authored pixel comes from project-owned geometric rules and fixed palettes. There are no downloaded images, sampled photographs, copied game textures, bundled art dependencies, or claims of legal clearance for the working title.

This is the source material set for the director's explicit 32×32 texture requirement. Source tests establish dimensions, mapping, deterministic output, visible variation and mip behavior. They do not establish approval of its appearance in the rendered game.

## Array contract

`TILE_SIZE = 32`. `layer_count()` is the number of texture ids in the frozen block registry (18 for the builtin set). `rgba_layers()` packs layer → row → column → RGBA. Each layer is 4,096 bytes. RGB values are full sRGB albedo; alpha is always 255. Upload with an sRGB RGBA8 texture-array format. Vertex RGB remains lighting. Linear block tint is data for a later shader multiply and is not baked into these tiles.

`layer_for(BlockId, [nx,ny,nz])` reads the registry face list. The first variant is the layer the mesher uses. Layer numbers are assigned at freeze in sorted texture-id order. They are not written in the block TOML. The grain salt for the seventeen authored tiles still uses the historical authoring index, so those pixels are unchanged.

| Layer | Texture id | Authored pixel structure |
|---|---|---|
| 0 | `strata:bark` | Vertical bark grooves, raised ridges and an elongated knot |
| 1 | `strata:basalt` | Offset angular column joints, cool raised edges and sparse mineral inclusions |
| 2 | `strata:clay` | Terracotta sediment layers, fine slanted drying cracks and ochre rims |
| 3 | `strata:coal_ore` | Limestone host with a wrapped cluster of dark coal specks |
| 4 | `strata:crystal` | Cyan mineral seams, pale facet edges and blue-grey host rock |
| 5 | `strata:diamond_ore` | Limestone host with a wrapped cluster of pale cyan specks |
| 6 | `strata:endgrain` | Offset concentric growth rings with a radial split |
| 7 | `strata:gold_ore` | Limestone host with a wrapped cluster of warm gold specks |
| 8 | `strata:grass_side` | Irregular sod thickness, hanging root strokes and exposed earth; row zero is the top of the block |
| 9 | `strata:grass_top` | Olive meadow bed, sixteen clustered three-blade tufts, dark roots and warm blade tips |
| 10 | `strata:iron_ore` | Limestone host with a wrapped cluster of rust-colored specks |
| 11 | `strata:leaves` | Staggered pointed leaf clusters, individual veins and dark interstices |
| 12 | `strata:limestone` | Limestone sediment bands, short joints and a pale curved mineral inclusion |
| 13 | `strata:missing` | 4-pixel magenta (`#FF00FF`) and black checker. Not a limestone stand-in |
| 14 | `strata:sand` | Wind-ripple ridges with alternating lit/dark edges and sparse pale grains |
| 15 | `strata:snow` | Broad pale facets, cool shallow depressions and isolated highlights |
| 16 | `strata:soil` | Offset soil clods, shaded creases, warm raised edges and occasional pale pebbles |
| 17 | `strata:water` | Low-contrast teal wavelets and crossing darker striae; animation/reflection remain renderer effects |

Air is never rendered; its defensive mapping is `strata:limestone` (layer 12). Grass uses the soil layer on its underside, and both timber ends use the same endgrain layer. Ore tiles reuse the limestone host and add one mineral cluster each, so they stay distinct from stone and from each other without a second albedo path.

## Rendering conventions

Each block face repeats one 32-pixel tile. Greedy quads must derive repeating UVs from world coordinates or explicit face dimensions, so a merged face does not stretch a single tile across multiple blocks. Grass-side v increases downward from the top edge. Side timber grain runs along v. Use repeat address modes and a texture array to prevent filtering across material layers.

Periodic sampling wraps coordinates modulo 32. Features that cross a boundary use wrapped coordinates; the top and bottom of the directional grass-side tile deliberately represent different materials and are not intended to look continuous with each other. Fine deterministic grain changes channels by at most three values around the authored patterns. It supplies surface detail; it is not the primary design of a tile.

The palette uses restrained greens, warm earth, cool limestone/basalt and brighter cyan crystal seams. Leaf and water textures remain opaque in this stage. Leaf cutouts, fluid transparency, normal maps, roughness, emissive maps and biome-dependent texture sets are not implemented by this generator.

## Mip chain

`mip_chain()` returns six packed texture-array levels at 32, 16, 8, 4, 2 and 1 pixels per side. Every level contains every registry layer in the same order (18 in the builtin set). Each 2×2 group is decoded using the sRGB transfer function, averaged in linear light, and encoded back to sRGB. Alpha remains opaque and filtering never mixes layers. The builtin chain payload is 98,280 bytes; this is pixel payload, not total GPU allocation.

The renderer uses nearest magnification to retain readable pixels and linear minification/mipmap filtering to reduce distant shimmer. Renderer integration and visual acceptance are separate checks: inspect all six face orientations, grass-side direction, repeated large greedy faces, shared chunk boundaries, and distance transitions in the actual client.

`strata.exe --export-materials <new-directory>` writes one exact 32×32 PNG per registry layer, a 6× nearest-neighbour contact sheet and a layer/provenance manifest. The builtin export is 18 tiles. The parent directory must exist. Existing export directories are rejected, preserving artist edits. This is an export path; file edits do not yet hot-reload into the engine. A consumer test decodes every PNG and compares it byte-for-byte with the runtime source.

## Verification

```powershell
cargo test --locked materials::tests
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover exact dimensions and alpha; every block/face layer bound; directional grass and timber mapping; deterministic and distinct material bytes; contrast greater than the fine-grain variation; periodic sampling; complete mip dimensions; constant-color preservation across layer boundaries; and the linear-light average of black and white (188 in sRGB, rather than 128).

The content is authored procedurally in this project, with no third-party asset attribution required for the source patterns. Original authorship is a provenance statement, not a trademark, copyright-registration or other legal opinion. Human art-direction and native visual acceptance remain required before calling the material set premium or final.
