# Upload scheduling and native traces

The production renderer now receives completed meshes through `Streaming::upload_ready`. Its default admission limits are two callbacks, 4 MiB of vertex/index payload and 2 ms of CPU wall time per redraw. A callback already running cannot be interrupted. One mesh exceeding the byte limit may run first and alone so dense construction cannot starve forever. These are admission limits with an explicit oversized exception, not a guaranteed frame ceiling. Splitting large uploads remains necessary to remove that exception.

Results that exceed the remaining byte budget wait in one deferred slot outside the bounded result channel. Their pending ownership remains intact; later admission rechecks revision and desired location. Edits cannot let a stale result erase a replacement job. Movement discards obsolete results and still evicts old visible meshes. Each call inspects at most `3 × workers + 1` results so continuously arriving stale work cannot create an unbounded drain. Zero count, byte or time limits pause admission. Shutdown disconnects the queues and drops deferred work before joining workers.

Eighteen streaming tests cover these contracts, including deterministic simulated time, oversized progress, deferral across edits/movement and blocked-sender shutdown. These source tests are separate from the actual native observations below.

## Reproduce a native trace

From the repository root:

```powershell
.\trace-streaming.ps1 -TracePath evidence/streaming-new.jsonl -SaveDirectory saves/emerald-reach -SmokeSeconds 30
python tools/summarize_streaming_trace.py evidence/streaming-new.jsonl
```

The wrapper builds the locked release, captures source/binary hashes and hardware in a sibling `.provenance.json`, and launches the visible native client through `run.ps1`. Existing destinations are rejected. `-SkipBuild` is only appropriate when binary/source agreement has already been established. The client begins at the title scene; the wrapper does not simulate player input. Trace rows identify screen, camera and focus so a title run cannot be mistaken for traversal. Do not benchmark while compiling or running another client.

Direct native use is `strata.exe --trace-streaming <new-file.jsonl>`. `run.ps1 -TraceStreaming <new-file.jsonl>` also supports interactive capture. These direct paths do not automatically collect the wrapper's external provenance.

The JSONL header records the active surface size, MSAA, presentation mode, seed, adapter, worker count and admission policy. Each non-minimized redraw records its actual camera/screen/radius, admitted payload, CPU callback duration, entire admission duration, maximum observed queue/ready/mesh time, outcome and resident mesh payload. Screen/radius are captured before UI transitions to match the workload measured. Empty meshes count as callbacks but allocate no GPU chunk buffers.

Queue wait runs from submission until a worker begins meshing. Ready wait runs from mesh completion until the main-thread callback, including time blocked on the result channel. Callback duration includes bounds scanning, GPU buffer allocation/mapped copying/unmapping and replacement bookkeeping. Entire admission duration also includes result inspection, bookkeeping and mesh destruction. None waits for GPU completion. Resident payload excludes GPU allocator padding, staging, old in-flight resources, textures and all other RAM/VRAM.

Presentation intervals are CPU-observed submission cadence, not GPU execution or physical display intervals. Trace writes use a 64 KiB buffer; their observer overhead is not isolated. The footer's `application_clean_exit` describes application status before final trace flush/sync. Accept a trace only with a valid complete footer, matching frame counters/provenance, and launcher exit 0. A crash or write failure can leave a partial file; preserve it for diagnosis.

## Native observations — 2026-10-20

Both runs used the freshly checked release binary SHA256 `4E799BA96150765A1A2A7115490C4BCCD3C3A3E7FE8A0BF224BB8412F0ECE90D`, i7-9700, GTX 1660 Ti / Vulkan, 1440×900, 4× MSAA, AutoVsync and four workers. Each ran 15 seconds, produced 900 successful presentation submissions, zero skips, launcher exit 0 and an empty error log. Both traces remained on one title camera. Source/binary hashes identify the dirty tree based on `b06c1da`; subsequent documentation and diagnostic scripts do not change the measured Rust source.

| Observation | Retained normal world | Synthetic checkerboard |
|---|---:|---:|
| Desired chunks delivered, including empty | 294 | 294 |
| Time to deliver all, from Game start | 2,481.22 ms | 2,498.51 ms |
| Admitted payload total | 31,585,200 bytes | 53,557,000 bytes |
| Maximum payload in one redraw | 826,200 bytes | 19,660,800 bytes |
| Oversized admissions | 0 | 1, alone |
| Active-redraw callback p95 / maximum | 0.2531 / 0.4058 ms | 0.3701 / 7.1273 ms |
| Active-redraw admission maximum | 1.0902 ms | 8.1734 ms |
| Maximum queue / ready wait | 99.69 / 147.96 ms | 116.91 / 164.63 ms |
| Final nonempty resident meshes | 100 | 113 |
| Submission interval p95 / p99 after 5 seconds | 16.8904 / 17.9284 ms | 16.8182 / 16.9251 ms |

These are descriptive single-run observations. Game start occurs after world loading and GPU initialization, so the delivery measurement is not cold launch time. The normal and artificial worlds are different workloads; this table does not claim a speedup. Source tests establish deferral behavior; neither native run happened to need a deferred result. The checkerboard callback exceeded the soft time budget, documenting the remaining monolithic allocation/copy cost rather than hiding it.

Reports are `evidence/streaming-title-20261020-01.jsonl` and `evidence/streaming-checkerboard-20261020-01.jsonl`, each with its exact sidecar. Launcher logs are retained locally under `logs/20261020-181854-3f300099-*` and `logs/20261020-181920-aa1fe5c2-*`.

Create the disposable stress save with `python tools/create_streaming_fixture.py <new-directory>`. It emits 39,304 explicit edits over a checkerboard chunk `(2,4,-2)` and its halo, plus a manifest/hash. The title camera is `[98,162,12]`, looking along `[-0.3,-0.3,-1]` toward its center `[80,144,-48]`. The center produces the expected 196,608 triangles and 19,660,800 payload bytes. This is a native renderer stress fixture, not an automated gameplay test. Preserve real saves and select the new directory explicitly.

Open evidence: segmented upload costs, trace-on/off overhead, moving-camera/edit stress, long sessions, peak RAM/VRAM, GPU timestamps, mouse/resize/focus acceptance and full target-budget runs. The current native UI helper fails during initialization; fresh interactive acceptance remains open.
