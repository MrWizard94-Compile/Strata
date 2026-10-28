"""Validate a finished STRATA trace and report descriptive CPU measurements."""
import argparse
import hashlib
import json
import math
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("trace", type=Path)
args = parser.parse_args()
rows = [json.loads(line) for line in args.trace.read_text(encoding="utf-8").splitlines()]
header, end = rows[0], rows[-1]
assert header["kind"] == "header" and header["schema_version"] in (1, 2)
segmented = header["schema_version"] == 2
frames = rows[1:-1]
assert frames and all(row["kind"] == "frame" for row in frames)
assert end["kind"] == "end" and end["application_clean_exit"] is True
assert end["frames"] == len(frames)
provenance_path = Path(str(args.trace) + ".provenance.json")
assert hashlib.sha256(provenance_path.read_bytes()).hexdigest().upper() in header["source_label_external"]
previous_presented = previous_skipped = 0
for frame in frames:
    assert frame["outcome"] in ("presented", "skipped")
    previous_presented += frame["outcome"] == "presented"
    previous_skipped += frame["outcome"] == "skipped"
    assert frame["presented"] == previous_presented
    assert frame["skipped"] == previous_skipped
    stats = frame["uploads"]
    if "initial_time_of_day" in header:
        assert 0 <= header["initial_time_of_day"] < 1
        assert 0 <= frame["time_of_day"] < 1
    assert stats["meshes"] <= header["upload_max_meshes"]
    assert stats["inspected"] <= 3 * header["workers"] + 1
    if segmented:
        assert 0 <= stats["bytes"] <= header["upload_max_bytes"]
        assert 0 <= stats["segments"] <= header["upload_max_segments"]
        assert 0 <= stats["max_segment_bytes"] <= header["upload_max_step_bytes"]
        assert stats["bytes"] % 4 == 0 and stats["max_segment_bytes"] % 4 == 0
        assert (stats["bytes"] > 0) == (stats["segments"] > 0)
        assert stats["bytes"] <= stats["segments"] * stats["max_segment_bytes"]
        assert stats["remaining_bytes"] >= 0
        assert stats["deferred"] or stats["remaining_bytes"] == 0
    elif stats["bytes"] > header["upload_max_bytes"]:
        assert stats["meshes"] == 1 and stats["oversized"] == 1
    else:
        assert stats["oversized"] == 0

def distribution(samples):
    ordered = sorted(samples)
    if not ordered:
        return None
    return {"samples": len(ordered), "min": ordered[0],
            **{f"p{p}": ordered[math.ceil(p / 100 * len(ordered)) - 1] for p in (50, 95, 99)},
            "max": ordered[-1]}

active = [f for f in frames if f["uploads"]["meshes"] or f["uploads"]["bytes"]
          or f["uploads"].get("started", 0) or f["uploads"].get("cancelled", 0)]
full = next((f["elapsed_ms"] for f in frames if f["desired_chunks"] > 0
             and f["current_chunks"] == f["desired_chunks"]), None)
result = {
    "trace": args.trace.name,
    "schema_version": header["schema_version"],
    "generator_version": header.get("generator_version"),
    "initial_time_of_day": header.get("initial_time_of_day"),
    "trace_sha256": hashlib.sha256(args.trace.read_bytes()).hexdigest(),
    "binary_sha256": json.loads(provenance_path.read_bytes())["binary_sha256"],
    "frames": len(frames), "presented": previous_presented, "skipped": previous_skipped,
    "screens": sorted({f["screen"] for f in frames}),
    "distinct_camera_positions": len({tuple(f["eye"]) for f in frames}),
    "first_all_desired_chunks_delivered_ms_after_game_start": full,
    "completed_meshes_including_empty": sum(f["uploads"]["meshes"] for f in frames),
    "copied_payload_bytes": sum(f["uploads"]["bytes"] for f in frames),
    "max_payload_bytes_per_redraw": max(f["uploads"]["bytes"] for f in frames),
    "oversized_admissions": sum(f["uploads"].get("oversized", 0) for f in frames),
    "deferred_redraws": sum(f["uploads"]["deferred"] for f in frames),
    "upload_callback_ms_on_active_redraws": distribution([f["uploads"]["upload_ms"] for f in active]),
    "admission_ms_on_active_redraws": distribution([f["uploads"]["elapsed_ms"] for f in active]),
    "max_queue_wait_ms": max(f["uploads"]["max_queue_wait_ms"] for f in frames),
    "max_ready_wait_ms": max(f["uploads"]["max_ready_wait_ms"] for f in frames),
    "present_submission_interval_ms_all": distribution([f["present_interval_ms"] for f in frames if f["present_interval_ms"] is not None]),
    "present_submission_interval_ms_after_5s": distribution([f["present_interval_ms"] for f in frames if f["elapsed_ms"] >= 5000 and f["present_interval_ms"] is not None]),
    "final_resident_meshes": frames[-1]["resident_meshes"],
    "final_resident_payload_bytes": frames[-1]["resident_mesh_payload_bytes"],
    "final_resident_triangles": frames[-1]["resident_triangles"],
    "limits": "Descriptive single-run CPU observations. Require launcher exit 0 as well as this trace validation. No GPU time, display timing, total RAM/VRAM, traversal or playability proof. Tracing overhead is not isolated.",
}
if segmented:
    result.update({
        "max_segment_bytes": max(f["uploads"]["max_segment_bytes"] for f in frames),
        "segments": sum(f["uploads"]["segments"] for f in frames),
        "cancelled_uploads": sum(f["uploads"]["cancelled"] for f in frames),
        **{f"{name}_ms_on_active_redraws": distribution([f["uploads"][f"{name}_ms"] for f in active])
           for name in ("allocation", "copy", "finalize")},
    })
print(json.dumps(result, indent=2))
