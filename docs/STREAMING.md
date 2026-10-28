# Upload scheduling and native traces

The renderer receives meshes through a synchronous Begin / copy / Commit / Cancel protocol. Defaults cap vertex/index copies at **4 MiB per redraw**, **1 MiB per segment**, and sixteen segments. At most two meshes complete in one redraw. The 2 ms CPU limit is checked between commands; buffer allocation, a copy already running, cleanup and driver calls cannot be preempted. The byte cap has no oversized exception, but this is not a guaranteed frame-time or memory ceiling.

One CPU result and its incomplete GPU buffers can remain outside the bounded result channel. The renderer keeps the old mesh visible until all replacement bytes are queued and Commit succeeds. Empty meshes remove an old mesh only at Commit. Four-byte alignment allows a segment to split a vertex safely; the final buffer still uses the 44-byte vertex layout. Complete buffers are allocated up front without mapping, then filled using queue writes. Queue submission orders those copies before drawing. A skipped or failed surface acquisition still submits queued writes so repeated skips cannot accumulate unsubmitted staging indefinitely.

Every resumed upload rechecks the world revision and desired region, including when upload budgets are paused. Edits cancel obsolete partial data without clearing a newer job's pending entry. Movement cancels obsolete partial data and emits eviction for any old visible mesh. Each call inspects at most `3 × workers + 1` results. Zero count/time budgets, or byte/step budgets below four bytes, pause new work; byte limits round down to four-byte alignment. Shutdown disconnects both queues before joining workers.

Twenty-three streaming regressions cover exact byte assembly, atomic progress, a full 18.75 MiB mesh over five redraws, tiny-segment limits, soft-time overruns, paused cancellation, revision ownership, eviction and blocked-sender shutdown. A separate explicit native GPU test reads buffers back byte-for-byte and exercises partial replacement, cancellation before and after submission, invalid ranges, premature commit and empty removal. Generated-mesh tests check conservative chunk bounds, which replace the former render-thread scan of every vertex.

## Reproduce a native trace

From the repository root:

~~~powershell
.\trace-streaming.ps1 -TracePath evidence/streaming-new.jsonl -SaveDirectory saves/emerald-reach -SmokeSeconds 30
python tools/summarize_streaming_trace.py evidence/streaming-new.jsonl
~~~

The wrapper builds the locked release, captures source/binary hashes (including .cargo/config.toml) and hardware in a sibling `.provenance.json`, and launches the visible native client through `run.ps1`. Existing destinations are rejected. `-SkipBuild` requires binary/source agreement established beforehand. The client starts at the title scene; the wrapper does not simulate player input. Screen, camera and focus are recorded. Do not benchmark while compiling or running another client.

Direct native use is `strata.exe --trace-streaming <new-file.jsonl>`. `run.ps1 -TraceStreaming <new-file.jsonl>` supports interactive capture. These direct paths do not automatically collect external provenance.

New merged-build traces also record generator version, initial time of day and time of day per redraw. These additive fields distinguish generator-2/daylight scenes from preserved generator-1 evidence. Schema 2 records completed meshes separately from bytes copied and transactions started. Each redraw reports segment count/largest segment, cancellation, remaining payload and deferred state. CPU callback time is split into allocation, copy and finalize/cancel; the sum excludes result inspection and CPU mesh destruction, which remain included in total admission time. Queue wait runs from job submission to worker start. Ready wait runs from meshing completion to Begin, including result-channel backpressure. Time spent completing a partial upload after Begin is separate from ready wait. The summary tool also validates historical schema 1 traces using their original oversized-admission policy.

These measurements do not wait for GPU completion. Queue writes copy data into staging immediately, with GPU transfer at submission; timings do not capture all driver, transfer or initialization costs. Resident payload counts only visible mesh buffers, excluding the pending replacement, old in-flight buffers, staging, allocator padding, textures and CPU data. Allocating the full pending replacement increases temporary memory requirements. Actual GPU OOM/device failures can still invoke wgpu's default error/panic behavior; protocol/preflight errors use the controlled CPU-save path, but this is not an OOM or device-loss recovery guarantee.

Presentation intervals measure CPU submission cadence, not GPU execution or physical display intervals. Trace writes use a 64 KiB buffer; observer overhead is not isolated. Accept a trace only with a complete footer, matching counters/provenance **and launcher exit 0**: final flush/sync can fail after the footer is written. Preserve partial traces for diagnosis.

## Generator-1 segmented-upload observations — 2026-10-20

The final measured release SHA256 is `20D39A912941C2B3AA7E836E96AFA95A8E8DBDB98F4487D94C4669203263C7F2`. These three runs used the same binary and source inputs, verified against their provenance sidecars. Hardware/settings match the historical baseline: i7-9700, GTX 1660 Ti / Vulkan, 1440×900, 4× MSAA, AutoVsync, four workers. Each ran 15 seconds, submitted 899 presentations, had zero skips, exited 0 and left an empty error log. All used a stationary title camera.

| Observation | Normal world (-03) | Checkerboard (-03) | Checkerboard (-04) |
|---|---:|---:|---:|
| Chunks completed, including empty | 294 | 294 | 294 |
| All delivered after Game start | 2491.77 ms | 2559.01 ms | 2558.13 ms |
| Copied payload total | 31,585,200 bytes | 53,557,000 bytes | 53,557,000 bytes |
| Maximum copied per redraw | 826,200 bytes | 4,194,304 bytes | 4,194,304 bytes |
| Largest copy segment | 727,056 bytes | 1,048,576 bytes | 1,048,576 bytes |
| Redraws retaining incomplete work | 0 | 4 | 4 |
| Callback maximum per active redraw | 0.1893 ms | 0.7045 ms | 0.7140 ms |
| Total admission maximum | 0.3739 ms | 1.6257 ms | 1.5612 ms |
| Allocation / copy maximum | 0.0334 / 0.1676 ms | 0.0340 / 0.7045 ms | 0.0338 / 0.6923 ms |
| Queue / ready wait maximum | 99.67 / 147.85 ms | 166.30 / 214.31 ms | 166.53 / 214.69 ms |
| Final resident meshes / triangles | 100 / 315,852 | 113 / 535,570 | 113 / 535,570 |
| Submission interval p95 / p99 after 5 s | 16.8740 / 17.1453 ms | 17.0857 / 17.1786 ms | 16.9574 / 17.2717 ms |

The 18.75 MiB stress mesh completes across five redraws, with four retaining partial work. Both final stress runs obeyed the 4 MiB call cap and 1 MiB segment cap. Their final 53,557,000 bytes and 535,570 triangles match the historical stress scene exactly. Native cancellation/edit behavior is covered by source and GPU tests, not by these stationary runs.

The historical monolithic callback peaked at 7.1273 ms; the final segmented runs peaked at 0.7045 and 0.7140 ms per active redraw. Full desired-world delivery took about 60 ms longer (2.558–2.559 s versus 2.499 s), consistent with distributing the large transfer across frames and retaining bounded backpressure. This is a same-scene observation, not an FPS uplift, hard time guarantee or statistical performance study. The change also removes a vertex-bounds scan and replaces mapped initialization with unmapped buffers plus queued writes. The difference cannot be attributed solely to segment size; GPU transfer/initialization and staging-memory costs remain unmeasured.

Reports are `evidence/streaming-title-20261020-03.jsonl` and `evidence/streaming-checkerboard-20261020-03.jsonl` / `-04.jsonl`, with exact sidecars. Local logs are `logs/20261020-234112-b56cf6e8-*`, `logs/20261020-233947-de37a88e-*` and `logs/20261020-234021-ae3c060c-*`. Sidecars record the dirty candidate based on `a80c17f`; source hashes, not that base commit alone, identify the measured implementation.

Earlier segmented `-02` reports are preserved. They used binary SHA256 `1AED63B82158216E31AC2E05903F20617CB9CF99F9F6657218830E5CDB78E07C`, before three additional test-only regressions. Their production Rust code is equivalent; the final reports above bind the complete current source and executable separately. The earlier stress run also obeyed the cap and recorded a 0.6697 ms callback maximum. No historical sidecars were rewritten.

## Historical monolithic-upload observations — 2026-10-20

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

Current fixture generation emits schema 1 / generator 2 at 08:00; it does not convert existing generator-1 saves. Create a new disposable stress save with `python tools/create_streaming_fixture.py <new-directory>`. It emits 39,304 explicit edits over a checkerboard chunk `(2,4,-2)` and its halo, plus a manifest/hash. The title camera is `[98,162,12]`, looking along `[-0.3,-0.3,-1]` toward its center `[80,144,-48]`. The center produces the expected 196,608 triangles and 19,660,800 payload bytes. This is a native renderer stress fixture, not an automated gameplay test. Preserve real saves and select the new directory explicitly.

Open evidence: GPU transfer/initialization and allocation recovery, trace-on/off overhead, moving-camera/edit stress, long sessions, peak RAM/VRAM, GPU timestamps, mouse/resize/focus acceptance and full target-budget runs. The current native UI helper fails during initialization; fresh interactive acceptance remains open.
