# Original 32×32 material set

The material generator in `src/materials.rs` authors seventeen 32×32 pixel albedo tiles for STRATA. Every pixel comes from project-owned geometric rules and fixed palettes. There are no downloaded images, sampled photographs, copied game textures, bundled art dependencies, or claims of legal clearance for the working title.

This is the source material set for the director's explicit 32×32 texture requirement. Source tests establish dimensions, mapping, deterministic output, visible variation and mip behavior. They do not establish approval of its appearance in the rendered game.

## Array contract

`TILE_SIZE = 32`, `LAYER_COUNT = 17`. `rgba_layers()` returns 69,632 bytes, packed in layer → row → column → RGBA order. Each layer is 4,096 bytes. RGB values are full sRGB albedo. Every layer except leaves stores alpha 255. Each staggered 8×8 leaf cell punches alpha 0 where `local_x.abs() + local_y.abs() >= 6`: 208 of 1,024 leaf texels, coverage 0.796875. Upload with an sRGB RGBA8 texture-array format, and preserve lighting/AO as independent factors rather than multiplying albedo by the existing block palette a second time.

`layer_for(Block, [nx,ny,nz])` accepts the axis-aligned integer face normals used by the world mesher. Mapping is independent of world seed, load order and chunk coordinates.

| Layer | Block / face | Authored pixel structure |
|---|---|---|
| 0 | Grass, +Y | Olive meadow bed, sixteen clustered three-blade tufts, dark roots and warm blade tips |
| 1 | Grass, side | Irregular sod thickness, hanging root strokes and exposed earth; row zero is the top of the block |
| 2 | Dirt; Grass, −Y | Offset soil clods, shaded creases, warm raised edges and occasional pale pebbles |
| 3 | Stone | Limestone sediment bands, short joints and a pale curved mineral inclusion |
| 4 | Sand | Wind-ripple ridges with alternating lit/dark edges and sparse pale grains |
| 5 | Water | Low-contrast teal wavelets and crossing darker striae; animation/reflection remain renderer effects |
| 6 | Wood, side | Vertical bark grooves, raised ridges and an elongated knot |
| 7 | Wood, ±Y | Offset concentric growth rings with a radial split |
| 8 | Leaves | Staggered pointed leaf clusters, individual veins and dark interstices |
| 9 | Snow | Broad pale facets, cool shallow depressions and isolated highlights |
| 10 | Clay | Terracotta sediment layers, fine slanted drying cracks and ochre rims |
| 11 | Crystal | Cyan mineral seams, pale facet edges and blue-grey host rock |
| 12 | Basalt | Offset angular column joints, cool raised edges and sparse mineral inclusions |
| 13 | Coal ore | Limestone host with a wrapped cluster of dark coal specks |
| 14 | Iron ore | Limestone host with a wrapped cluster of rust-colored specks |
| 15 | Gold ore | Limestone host with a wrapped cluster of warm gold specks |
| 16 | Diamond ore | Limestone host with a wrapped cluster of pale cyan specks |

Air is never rendered; its defensive mapping is layer 3. Grass uses the soil layer on its underside, and both timber ends use the same endgrain layer. Ore tiles reuse the limestone host and add one mineral cluster each, so they stay distinct from stone and from each other without a second albedo path.

## Rendering conventions

Each block face repeats one 32-pixel tile. Greedy quads must derive repeating UVs from world coordinates or explicit face dimensions, so a merged face does not stretch a single tile across multiple blocks. Grass-side v increases downward from the top edge. Side timber grain runs along v. Use repeat address modes and a texture array to prevent filtering across material layers.

Periodic sampling wraps coordinates modulo 32. Features that cross a boundary use wrapped coordinates; the top and bottom of the directional grass-side tile deliberately represent different materials and are not intended to look continuous with each other. Fine deterministic grain changes channels by at most three values around the authored patterns. It supplies surface detail; it is not the primary design of a tile.

The palette uses restrained greens, warm earth, cool limestone/basalt and brighter cyan crystal seams. Leaf corners are open so the cutout pass can discard them. Water's albedo tile stays opaque; the translucent pass supplies the blended alpha. Normal maps, roughness, emissive maps and biome-dependent texture sets are not implemented by this generator.

## Mip chain

`mip_chain()` returns six packed texture-array levels at 32, 16, 8, 4, 2 and 1 pixels per side. Every level contains all seventeen layers in the same order. Each 2×2 group is decoded using the sRGB transfer function. Opaque texels average evenly in linear light and keep alpha 255, so the black/white sRGB check stays 188. Leaf mips weight that average by coverage and store the coverage in alpha. The 1-pixel leaf alpha is 203, above the shader's 0.5 discard, so a distant canopy does not vanish or turn solid. Filtering never mixes layers. Total chain payload is 92,820 bytes; this is pixel payload, not total GPU allocation.

The renderer uses nearest magnification to retain readable pixels and linear minification/mipmap filtering to reduce distant shimmer. Renderer integration and visual acceptance are separate checks: inspect all six face orientations, grass-side direction, repeated large greedy faces, shared chunk boundaries, and distance transitions in the actual client.

`strata.exe --export-materials <new-directory>` writes seventeen exact 32×32 PNG tiles, a 6× nearest-neighbour contact sheet and a layer/provenance manifest. The parent directory must exist. Existing export directories are rejected, preserving artist edits. This is an export path; file edits do not yet hot-reload into the engine. A consumer test decodes every PNG and compares it byte-for-byte with the runtime source.

## Verification

```powershell
cargo test --locked materials::tests
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Tests cover exact dimensions; 208 leaf holes and opaque alpha on every other layer; every block/face layer bound; directional grass and timber mapping; deterministic and distinct material bytes; contrast greater than the fine-grain variation; periodic sampling; complete mip dimensions; opaque mips staying at alpha 255; the 1-pixel leaf alpha of 203; constant-color preservation across layer boundaries; and the linear-light average of black and white (188 in sRGB, rather than 128).

The content is authored procedurally in this project, with no third-party asset attribution required for the source patterns. Original authorship is a provenance statement, not a trademark, copyright-registration or other legal opinion. Human art-direction and native visual acceptance remain required before calling the material set premium or final.
