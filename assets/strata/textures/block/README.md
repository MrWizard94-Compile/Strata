# Block tiles

These PNGs are the shipped `strata` tiles. Another namespace uses the same layout at `assets/<namespace>/textures/block/`. The loader does not treat `strata` as a special case.

## Files

- Size: one square power of two from 16 to 512. This pack is 32×32. Every tile in a pack must match.
- Format: sRGB RGBA8. Color type and bit depth are checked. RGB, indexed, and 16-bit files are rejected.
- Names: lowercase letters, digits, hyphen, or underscore, ending in `.png`. No folders inside the name.
- Path: `assets/<namespace>/textures/block/<file>.png`.

## Alpha

Alpha is coverage for the cutout pass. Leaf holes are 0. Opaque texels are 255. The shader discards a cutout fragment when alpha is below 0.5. Water's see-through alpha is still computed in the shader; the water tile's alpha stays 255. Do not invent a second alpha rule.

## Manifest

`assets/block-textures.manifest` is the pack list.

- `layer <namespace> <file> [more files]` adds those images to the texture array, in order. Extra files on that line are variants and must stay a contiguous run of at most 16.
- `bind <namespace:name> <face> <first-file> [rotate]` maps a block face onto that run. Face is `all`, `top`, `bottom`, `side`, `east`, `west`, `north`, or `south`. A specific face wins over `side`, and `side` wins over `all`.
- `rotate` turns on a stable top-face rotation and horizontal flip. The choice comes from the world block position, so a merged face does not flicker and does not depend on chunk load order. Sides ignore the flag. The shipped binds leave it off. Add `rotate` on grass top, sand, or stone when that variation is wanted.

Replacing art is replacing the PNG. Adding a variant is one extra file name on the existing `layer` line. A PNG with no `layer` line fails the build as unmapped. A repeated bind, a missing file, a wrong size, or a non-RGBA8 file fails the build with the path in the message.

The game still generates the same pixels in `src/materials.rs`. The shipped PNGs are an export of that generator, and a test compares them byte for byte. A changed PNG fails that test until the generator changes too.

## Debug override

A debug build reads `STRATA_BLOCK_TEXTURES` as an assets root and, when a matching PNG exists, uses it instead of the embedded file. The replacement must be the pack's size and RGBA8. Release builds ignore the variable and stay self-contained.
