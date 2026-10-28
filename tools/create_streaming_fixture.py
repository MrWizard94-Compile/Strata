"""Create a NEW disposable save exercising native segmented mesh uploads."""
import argparse
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", type=Path, help="New save directory; its parent must exist")
args = parser.parse_args()
args.directory.mkdir()

# Match the merged save/world contract; older generator fixtures are preserved
# and intentionally rejected by this client. Explicit 08:00 matches MORNING in
# daylight.rs after JSON deserialization to f32, fixing the initial light state.
SAVE_SCHEMA = 1
GENERATOR_VERSION = 2
TIME_OF_DAY = 1.0 / 3.0

# Chunk (2, 4, -2), with a complete one-cell checkerboard halo. Explicit air
# overrides prevent procedural terrain from changing the analytic workload.
edits = [
    [[x, y, z], 3 if (x + y + z) % 2 == 0 else 0]
    for x in range(63, 97)
    for y in range(127, 161)
    for z in range(-65, -31)
]
snapshot = {
    "schema": SAVE_SCHEMA, "generator": GENERATOR_VERSION, "generation": 1, "seed": 728491,
    "player": {"eye": [83.0, 134.0, -12.0], "yaw": 0.0, "pitch": 0.0, "flying": True},
    "edits": edits,
    "time_of_day": TIME_OF_DAY,
}
encoded = json.dumps(snapshot, separators=(",", ":")).encode("utf-8")
with (args.directory / "world-b.json").open("xb") as handle:
    handle.write(encoded)
manifest = {
    "fixture_version": 2,
    "save_schema": SAVE_SCHEMA, "generator": GENERATOR_VERSION,
    "time_of_day": TIME_OF_DAY,
    "chunk": [2, 4, -2], "seed": 728491,
    "saved_edits": len(edits), "save_sha256": hashlib.sha256(encoded).hexdigest(),
    "expected_center_triangles": 196608,
    "expected_center_mesh_payload_bytes_at_vertex_stride_44": 19660800,
    "title_camera_eye": [98.0, 162.0, 12.0],
    "title_camera_direction_unnormalized": [-0.3, -0.3, -1.0],
    "target_center": [80.0, 144.0, -48.0],
    "scope": "Synthetic rendering/upload diagnostic; no gameplay or performance-target acceptance.",
}
with (args.directory / "fixture.json").open("x", encoding="utf-8", newline="\n") as handle:
    json.dump(manifest, handle, indent=2)
    handle.write("\n")
print(json.dumps(manifest, indent=2))
