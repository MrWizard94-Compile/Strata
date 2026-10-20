# STRATA product brief

Phase: **S0.P2**. Date: **2026-10-18**. STRATA is a working title, not a cleared brand. This brief preserves the director's objective: build a premium Rust voxel engine and game with a living world, meaningful engineering and physics, and compelling exploration, strong enough to pitch credibly to Microsoft. The engine is core intellectual property and a deliberate part of the product.

The full game remains the goal. A playable prototype, successful benchmark, or polished demonstration is an intermediate gate. The design below is an implementation direction with measurable review criteria; it is not a statement that these systems already exist.

## The experience

Build a place that changes because the player lives there. Cross a mountain range for something the valley cannot provide. Construct a machine that makes the journey possible. Return to discover that water, wildlife and other inhabitants responded to the changes left behind. The world should reward understanding its rules as much as acquiring equipment.

The central loop is **observe → explore → understand → build → change the world → adapt**. Player creations need purpose beyond decoration, and exploration needs discoveries beyond a differently coloured landscape. Physical and ecological consequences must be legible, reversible where the design allows it, and enjoyable to work with.

Rust supports the chosen implementation strategy. The pitch rests on the game, the engine's demonstrated behavior and the team's ability to deliver; it does not depend on claiming that Java made better games impossible.

## Four connected pillars

| Pillar | Intended player value | Concrete proof to build |
|---|---|---|
| A living world | The world remembers interventions and continues beyond the player's immediate view | Rerouting water changes downstream habitat/resources; an inhabitant or wildlife population reacts; leaving and reloading retains coherent consequences |
| Engineering with physical meaning | Players build tools, transport and structures that solve real spatial problems | A powered lift or bridge obeys load, connection and power rules; the player can diagnose and repair a visible failure |
| Exploration through knowledge | Going farther changes what the player knows and can do | A remote formation, ruin or ecosystem reveals a usable principle/material and a new route or construction opportunity |
| A premium engine | Scale, responsiveness and reliability make those systems feel like one place | World edits, simulation, long views and save/restart coexist in the actual game under a published hardware/workload profile |

The proposed visual identity is geological: readable strata, mineral colour, atmospheric depth, deliberate silhouettes, tactile tools and machines. The director requires original 32×32 material textures. Resolution is a minimum asset contract; material readability, tiling, distance filtering and coherent art direction also need rendered inspection. Audio should explain motion, load, weather and life as well as establish mood. Art direction remains subject to director review; all shipped assets need recorded provenance and usage rights.

## Signature demonstration: the living watershed

A proposed integrated showcase begins above an inhabited valley. The player identifies a damaged crossing and a water-powered mechanism, explores a cavern for a material or construction insight, then builds a working route through the terrain. Diverting part of the stream powers the mechanism but changes a downstream resource or habitat. The player sees the consequence, adjusts the design, and returns after a save/restart to a world that retained the result.

This scenario provides a demanding shared test for terrain edits, water rules, construction physics, creatures/inhabitants, exploration, lighting, interaction and persistence. It needs real systemic cause and effect. Scripted presentation may guide the introduction but must not impersonate simulated behavior in the technical demonstration. It is one representative slice of the full game, not a reduction of the final scope to one valley or puzzle.

Longer-term scope includes a varied world, progression, durable player settlements and machines, modding through a versioned capability API, and cooperative creation/exploration with an explicit multiplayer authority model. The current build has no network gameplay or mod runtime. These remain individually tracked development and acceptance phases, not implied features of the prototype.

## Current baseline and next decisions

Source inspection finds a native Windows foundation with procedural terrain, editable blocks, CPU greedy meshing, asynchronous chunk work, a custom player controller, GPU terrain presentation and versioned local saves. It does not establish the four-pillar experience. [Research](RESEARCH.md) records the precise source boundaries, roadmap corrections and experiments; the central stage tracker owns implementation status and specialist assignments.

Prioritize the shared world authority and persistent event contracts before independent systems begin writing competing state. Prototype a small ecological consequence and one useful physical construction early enough to discover whether the central loop is fun. Rendering advances should make that experience possible and attractive while preserving interaction and reliability. Do not postpone all game design until after an elaborate renderer.

Survival, combat, building and exploration are required. Open product choices include their balance, the combat model, inhabitant complexity, fluid realism, construction failure severity, cooperative player count, content-production tools and the business model. The current authorized work can prototype these without announcing commitments. Decisions that materially change player experience or production scope must return to the director with a concrete comparison.

## Measurable pitch-readiness gates

The following are proposed internal targets for a credible demonstration package. They are neither current results nor Microsoft certification criteria. Changes to a target require a recorded reason and director visibility; silently lowering it after a failed measurement is not acceptance.

| ID | Gate | Evidence required |
|---|---|---|
| PITCH-01 | Distinct game identity | An unedited 10–15 minute playable watershed sequence demonstrates every pillar and two consequential player choices. Three independent first-time players can explain the loop and one systemic consequence in their own words; record friction and dissent, not just positive quotes |
| PITCH-02 | Integrated systems | The same world supports construction, terrain change, a persistent living-world reaction, discovery and save/restart. Test normal use, misuse and region unload/reload; no manually repaired state between demonstration steps |
| PITCH-03 | Responsive baseline PC experience | Proposed 1920×1080 native-resolution GTX 1660 Ti profile, with exact CPU, RAM, driver, build, scene, settings and capture method fixed before acceptance. Over three equivalent 10-minute traversal/build runs: frame-interval p95 ≤16.67 ms and p99 ≤33.33 ms, no unaccounted stall over 100 ms, no increasing simulation backlog. Report startup separately |
| PITCH-04 | Honest scale and resource use | Proposed full-detail interaction radius ≥256 m and a visual horizon ≥2048 m, with LOD boundaries and visible terrain extent disclosed. Proposed process RAM ceiling 8 GiB and game GPU allocation/residency budget 4.5 GiB on the 6 GiB card. Record actual peak, transient copies, OS/driver measurement limits and long-session growth; these budgets still need feasibility testing |
| PITCH-05 | Reliable player ownership | Repeated save/load and interrupted-write tests preserve accepted world changes; one-hour representative sessions complete without crashes or corrupted saves across five runs. Demonstrate migration or an explicit supported-version boundary, actionable recovery and repeatable installation |
| PITCH-06 | Premium usability and presentation | Complete controller and keyboard/mouse path through the showcase; remapping, readable UI scale, essential contrast, subtitles/captions for meaning carried by audio, adjustable motion effects and a coherent original art/audio direction. Director and independent playtest review remains separate from source QA |
| PITCH-07 | Defensible engine case | Reproducible benchmark scenes, pinned source/build, architecture/tradeoff notes, licenses and asset provenance, known issues, and a demonstrated safe mod boundary if modding is advertised. Any comparison uses equivalent workloads and discloses differences |
| PITCH-08 | Reviewable business and platform package | Playable PC build, short honest gameplay capture, concise game/engine presentation, realistic remaining-scope and staffing/cost model, and a specific partnership ask. Document Windows evidence, Xbox access/porting gaps, and proposed release path separately. No invented market validation, revenue projection or Microsoft endorsement |

Population, machine complexity, edit throughput and multiplayer player count must be selected from the intended experience and measured experiments before their phases close. They cannot remain unspecified in a final pitch. The physics target is a 60 Hz simulation with measured headroom and an explicit overload policy; a fixed timestep in source alone does not meet that gate. High-end PC modes, advanced illumination and console profiles each need their own workload and evidence.

## Microsoft-facing claim boundary

The intended message is an original game with distinctive systemic play, powered by a Rust engine whose scale, stability and extensibility have been demonstrated. Show the actual build and the actual limits. A language choice, attractive landscape or borrowed timing table cannot carry the pitch.

Microsoft currently provides public GDK/documentation research entry points, while partner access, console resources and service permissions have distinct gates. The published funding-pitch route requires ID@Xbox registration and an approved game concept. Preparing this package neither submits it nor establishes eligibility, certification, a commercial agreement or funding. [Microsoft's 2026 development overview](https://developer.microsoft.com/en-us/games/articles/2026/03/gdc-2026-press-start-get-your-pc-game-ready-for-xbox-in-one-day/), [resource access](https://learn.microsoft.com/en-us/xbox/gdk/docs/gdk-dev/development-downloads/access-resources?view=gdk-2604), [pitch process](https://learn.microsoft.com/en-us/gaming/game-publishing/onboarding/onboarding-pitch-your-game)

## Acceptance record for this brief

S0.P2 produces a source-grounded direction, explicit claim boundaries and falsifiable pitch criteria. Its completion does not complete the game or the experiments. The integrator owns stage IDs, dependencies, status updates and assignment of a specialized micro-agent to each implementation/verification phase. Every later gate needs its own artifact and evidence, with the director retaining creative and architectural veto.
