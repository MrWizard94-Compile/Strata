# STRATA delivery tracker

Updated 2026-10-20. Active project: `C:\WPAI\Software\Rust Based Voxel`.

## Director objective

Build an original, premium, production and studio grade Rust voxel engine and survival/combat/building/exploration game that can support a confident pitch to Microsoft. The engine is a central differentiator, alongside a living world and engineering/automation. Original 32×32 material textures are an explicit director requirement. Track each stage and its phases independently; delegate phases to specialized micro-agents. Research uncertain claims and install required dependencies. The working title is STRATA.

This is the full goal. A playable foundation does not complete it. Nothing here asserts Microsoft endorsement, Xbox compatibility, a commercial relationship, or measured superiority over another game.

## How this tracker works

Stage status and phase status are separate. `planned` means unstarted; `active` means work is underway; `partial` means some evidence exists and named gates remain; `verified` requires every listed phase gate; `accepted` additionally requires director acceptance when taste or product judgment is decisive. No elapsed-time or percentage completion estimates are used. A phase's evidence explicitly identifies source, artifact, native runtime, hardware, or human acceptance.

The root integrator owns integration and this tracker. Micro-agents own bounded files or a research/verification responsibility. Their output is reviewed and tested in the assembled application before promotion. An agent stopping or reporting completion does not close a phase.

## Stage overview

| Stage | Outcome | Status | Exit gate |
|---|---|---|---|
| S0 — Product and technical evidence | Original product thesis, corrected source claims, architecture, risks, measurable budgets | active | Research and product brief reviewed; risks have executable experiments and decision gates |
| S1 — Playable engine foundation | Deterministic editable terrain, native rendering, locomotion, bounded streaming, retained saves | active | Actual client enter/explore/edit/save/relaunch flow; clean source checks; recorded hardware evidence |
| S2 — World scale and rendering | Large views and dense worlds within measured CPU/GPU/RAM limits | active | Same-workload comparative benchmarks; seamless edits/LOD; frame-time and memory budgets on target hardware |
| S3 — Living simulation | Persistent ecology, creatures, weather and meaningful world response | planned | Deterministic or explicitly bounded simulation; playable emergent loop; long-run and persistence tests |
| S4 — Engineering and construction | Machines, structures, energy, logistics and physical interactions | planned | Player-built machine solves a world problem; stable overload, unload, reload and multiplayer behavior |
| S5 — Premium game experience | Cohesive survival/combat/exploration/progression, original 32×32 art, audio, interaction and accessibility | active | External playtest of complete representative loop; documented feedback closure |
| S6 — Extensibility and shared worlds | Safe mods and robust multiplayer tied to the same simulation | planned | Adversarial mod tests, versioned contracts, authoritative networking, reconnect/rollback and save compatibility |
| S7 — Production readiness | Reproducible releases, compatibility, security, performance, recovery and support | planned | Release acceptance matrix passes; install/update/data-retention and prolonged runtime evidence |
| S8 — Microsoft pitch and platform evidence | Honest, compelling demo, business case, technical dossier and platform path | planned | Review-ready deck/build/capture; claims supported; director approves pitch; platform operation demonstrated where claimed |

## S0 — Product and technical evidence

| Phase | Specialized owner | State | Evidence / closure requirement |
|---|---|---|---|
| S0.P1 — Scope and success criteria | Director + root product integrator | partial | Director confirmed engine, survival/combat/building/exploration and 32×32 textures. Product brief retains living-world/engineering direction; creative thesis and proposed budgets need director review. |
| S0.P2 — Primary-source audit | Research micro-agent (`evolution`, second bounded assignment) | verified | Integrator reviewed RESEARCH.md and PRODUCT-BRIEF.md: arithmetic/API/sandbox/console corrections, primary-source links and R01–R08 experiments. Research artifact verified; experiments and game remain open. |
| S0.P3 — Architecture and risk experiments | Root engine architect | active | Architecture recorded; hardest risks need executable experiments before final budgets or platform promises. |
| S0.P4 — Reproducible toolchain and tracking | Root build integrator | partial | Rust 1.97.1, locked dependencies, integrated release build and strict checks pass; ledger exists. Pull requests into development run pinned Windows and Linux source checks, including lavapipe shader validation. The Windows job packages `strata-windows-x86_64.zip` and launches that exe. A separate workflow publishes that zip first, then the Linux zip, with SHA-256 checksums. Further operating systems are extra matrix rows and are not enabled. Branch protection and Actions release permission remain owner settings; a workflow file does not itself block a merge or prove a second build hashes the same. |

## S1 — Playable engine foundation

| Phase | Specialized owner | State | Evidence / closure requirement |
|---|---|---|---|
| S1.P1 — Terrain, materials, meshing and queries | `world` micro-agent; root integration | partial | Geometry/determinism/halo/DDA tests pass. Earlier actual window showed terrain and visible keyboard edits. 32×32 material integration is tracked in S5.P3; extended visual coverage remains open. |
| S1.P2 — GPU presentation | `renderer` micro-agent; root integration | partial | Native pipelines validate. Two current textured runs each submitted 900 presentations in 15 seconds, including an oversized checkerboard fixture. Presentation/skip accounting and CPU traces verified. Resize and surface-loss injection remain open. |
| S1.P3 — Movement and interaction | Root gameplay integrator; `renderer` review | partial | Actual keyboard break/place/save observed. Regression tests cover a planted trunk at seed 396 column `(64, -64)`, a natural trunk at seed `20261021` column `(-321, -354)`, and unsafe flight exit. Mouse editing, extended traversal and fresh fixes in-world remain open. |
| S1.P4 — Bounded streaming and scheduling | Root + `world` concurrency work | partial | Streaming tests cover stale results, byte/time/count admission, segmented uploads, unload during a partial upload, bounded inspection/queues, duplicate work, halo invalidation and shutdown. The 2026-10-20 native stress run admitted one 18.75 MiB mesh alone; that exception is gone in source. A Windows trace of the packed format remains open. |
| S1.P5 — Save integrity and re-entry | Root persistence integrator | partial | Two-slot saves, rejection and exclusive-writer tests pass. Actual edited world saved and subsequent boot reported saved_world=true; visual retained-block/position re-entry remains open. No power-loss guarantee. |
| S1.P6 — Native acceptance and baseline | Root QA integrator / native computer-use skill | partial | Source checks and explicit GPU validation pass; release startup/close succeeds with retained save. Native UI helper currently fails before initialization, so current interactive/resize acceptance is incomplete. See STATUS.md. |

## S2 — World scale and rendering

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S2.P1 — Benchmark harness and budgets | `world` CPU specialist + root measurement | partial | Six-scenario production CPU mesher harness and three release reports verified; raw samples/source/binary provenance retained. CPU-BASELINE.md summarizes results. Two native title/stress traces now measure upload/queue costs. Full R01 movement/GPU/resource traces and accepted budgets remain open. |
| S2.P2 — Storage and scheduling | `world` scheduling specialist + root integration | active | Packed 8-byte quads and slice admission are integrated. One callback still cannot be preempted and is capped at 4 MiB, not by a measured 2 ms copy. The 2026-10-20 oversized callback was 7.13 ms on a 19,660,800-byte mesh. Palette/compression, lifetime/resource traces, edit priority and large-world precision remain open. |
| S2.P3 — Mesh and GPU submission | Renderer optimization engineer | planned | Compare current greedy meshing with binary algorithm; capability-gated indirect draw/culling; no false visibility loss |
| S2.P4 — Distance and lighting | LOD/lighting researcher | planned | LOD seam and edit propagation tests; measured skylight/shadows/GI quality-cost tradeoff; graceful quality tiers |

## S3 — Living simulation

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S3.P1 — Authoritative world tick | Simulation architect | planned | Explicit state ownership, time, event order, save contracts and deterministic replay where required |
| S3.P2 — Ecology and resources | Ecology systems engineer | planned | Growth/renewal/resource competition changes player decisions; bounded work in loaded/unloaded regions |
| S3.P3 — Creatures and navigation | AI/navigation engineer | planned | Creatures navigate edited terrain, react meaningfully and persist; bounded pathfinding and failure handling |
| S3.P4 — Weather and environmental systems | Environmental simulation engineer | planned | Weather/water/heat scope chosen through measured experiments; stable interactions and long-duration acceptance |

## S4 — Engineering and construction

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S4.P1 — Build grammar and structural state | Construction designer/engineer | planned | Original building affordances, stable IDs and editable assemblies; clear feedback for validity |
| S4.P2 — Physics and moving mechanisms | Physics engineer | planned | Rapier/custom alternatives benchmarked; CCD, collider regeneration, seams and budgeted destruction verified |
| S4.P3 — Power and logistics | Network simulation engineer | planned | Deterministic power/transport networks; explicit capacity, backpressure and topology-edit semantics |
| S4.P4 — Automation gameplay | Systems game designer | planned | A player constructs and operates a useful production chain with diagnostics and progression |

## S5 — Premium game experience

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S5.P1 — Discovery and progression | World/game designer | planned | Distinct exploration rewards, landmarks and progression integrated with simulation and engineering |
| S5.P2 — Survival and combat | Gameplay engineer | planned | Resource/crafting/survival/combat loop balanced through playtest, with meaningful failure, recovery, responsive attacks, enemies and readable feedback |
| S5.P3 — Art, audio and identity | `world` material specialist + root renderer; later art/audio specialists | partial | Original 32×32 tiles, per-face selection, tiled merged geometry, mip filtering and PNG export integrated and checked. Contact sheet inspected. Native visual acceptance, authored content, soundscape, animation and complete asset/license manifest remain open. |
| S5.P4 — UX and accessibility | Interaction/accessibility engineer | planned | Controller, remapping, scalable UI, readable feedback, reduced-motion and accessibility review; onboarding validated |

## S6 — Extensibility and shared worlds

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S6.P1 — Content and mod contracts | Mod API architect | planned | Versioned block/item/entity definitions and batched event API; save migration and compatibility policies |
| S6.P2 — Sandbox and capability limits | Wasm security engineer | planned | Wasmtime component prototype with explicit capabilities, fuel/epoch distinction, memory limits and trapped-mod recovery |
| S6.P3 — Authoritative multiplayer | Network engineer | planned | Server-owned state, replication/prediction/interpolation, chunk streaming and adversarial input validation |
| S6.P4 — Shared-world durability | Reliability engineer | planned | Disconnect/reconnect, duplicate/reordered messages, mod mismatch, server save/restart and concurrent edit tests |

## S7 — Production readiness

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S7.P1 — Performance and hardware matrix | Performance QA engineer | planned | Agreed budgets on low/target/high hardware; long sessions and peak memory; no borrowed benchmark claims |
| S7.P2 — Security and supply chain | Security/release auditor | planned | Dependency advisories, licenses, hostile saves/mods/network input and crash-diagnostic privacy reviewed |
| S7.P3 — Packaging and upgrades | Release engineer | planned | Reproducible clean build, checksums, install/update/rollback preserving real player worlds |
| S7.P4 — External playtests and defect closure | QA lead + director | planned | Representative external feedback, severity triage and closure; no unresolved release blockers |

## S8 — Microsoft pitch and platform evidence

| Phase | Specialized micro-agent role | State | Evidence / closure requirement |
|---|---|---|---|
| S8.P1 — Platform feasibility | Platform engineer | planned | GDK/console program access confirmed where needed; exact runtime/backend/SDK path and real target test |
| S8.P2 — Demonstration slice | Demo director + engine/game agents | planned | Cohesive real build demonstrates engine, living world, engineering and exploration without staged fake behavior |
| S8.P3 — Commercial and technical dossier | Product/research writer | planned | Clear audience, original differentiation, production plan, risk register, measured technical evidence and defensible claims |
| S8.P4 — Pitch review and delivery | Director + release/presentation agents | planned | Deck, captured footage, build and presenter notes reviewed; external contact/publication requires director instruction |

## Current blockers and next integration order

Two specialized source agents stopped due to an account usage limit after writing files. Their files are present and root integration continues. This limits further parallel capacity, not local progress. The director subsequently authorized committing and pushing all project changes to Strata. This does not extend to commercial release, the separate Constitution amendment, or external outreach.

1. Re-measure packed-quad upload duration, including first-slice buffer allocation, on the GTX 1660 Ti. The Linux VM has no GPU adapter. The 7.13 ms callback is the 2026-10-20 observation, before this format.
2. Recover native UI tooling and finish rendered re-entry, mouse input, resize and traversal acceptance.
3. Measure moving-world streaming, GPU time and peak memory; decide further optimization from evidence.
4. Implement persistent world authority and an early meaningful survival/combat/building loop alongside rendering/scale work.
5. Preserve all later simulation, engineering, shared-world, production and pitch gates.

Research-to-delivery links: R01 → S1.P4/S2.P1/PITCH-03/04; R02 → S3/PITCH-02; R03 → S4/PITCH-02; R04/R05 → S2.P3/P4; R06 → S6.P2; R07 → S1.P5/S6.P4/S7.P3; R08 → S8.P1. PITCH-01/02 require integrated gameplay, PITCH-05/06 require reliability and human playtests, PITCH-07/08 require S7/S8 evidence. None is closed by the CPU baseline alone.

## Invocation provenance

Frozen law: `MrWizard94-Compile/Agents-Constitution@0010a8353b00754fad31a30ed24327d3b6d05425`, version 6.16.4. Byte-exact 19-file check and source validator passed; accepted commit's `Validate WPAI Constitution Source` workflow reports success. One evidence-budget/platform-claim amendment is staged separately, not propagated, and does not govern this invocation.
