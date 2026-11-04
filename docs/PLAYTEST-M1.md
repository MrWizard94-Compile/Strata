# STRATA Milestone 1 playtest

For Rob, on a Windows PC with an NVIDIA GTX 1660 Ti. STRATA is the working name. This pass checks the graphics card first, then the playable controls, in one sitting. It is not the finished game.

## This applies after these land, in this order

#3 and #2 are merged. The rest of the order is #5, #4, #6, then this pull request:

1. [Pull request #3](https://github.com/MrWizard94-Compile/Strata/pull/3) — plains, forests, deserts, oceans, caves, and ores. Merged.
2. [Pull request #2](https://github.com/MrWizard94-Compile/Strata/pull/2) — day and night. Merged.
3. [Pull request #5](https://github.com/MrWizard94-Compile/Strata/pull/5) — smaller terrain uploads (`cursor/packed-quad-upload-budget-f0c1`)
4. [Pull request #4](https://github.com/MrWizard94-Compile/Strata/pull/4) — walking, sprint, and sneak (`cursor/player-movement-feel-8b4f`)
5. [Pull request #6](https://github.com/MrWizard94-Compile/Strata/pull/6) — breaking, placing, and the block picker (`cursor/interaction-hotbar-picker-3aca`)
6. This checklist, [pull request #7](https://github.com/MrWizard94-Compile/Strata/pull/7), last

`development` already has the title screen, pause screen, breaking and placing, flight, F3, F5 save, the ore blocks from #3, and the day/night clock from #2. Still to come are the upload check, the walking feel, and the block picker. Do this whole list in one sitting, after someone tells you those remaining pull requests are merged. An older build will fail the new steps.

The combined game you are checking works like this: you can change a block about five blocks away, F4 changes the clock even while a menu is open, the save keeps both the clock and the hotbar, left Ctrl sneaks on the ground and sinks while flying, and the corner word changes among **SNEAKING**, **FREE FLIGHT**, **CHOOSING A MATERIAL**, and **EXPLORING**.

## How to get ready

The game on your PC lives in this folder:

`C:\WPAI\Software\Rust Based Voxel`

If that folder is not there, stop and tell Vector or Atlas. Do not download a different copy unless they tell you to.

Open PowerShell from the Start menu. If PowerShell says `cargo` is not recognized, Rust is missing. Close that window and do this:

1. In a browser, open [https://rustup.rs](https://rustup.rs).
2. Download `rustup-init.exe` and run it.
3. Accept the default install. Choose the MSVC toolchain if it asks. That is the Windows one this project needs. If it says the C++ build tools are missing, let it install them.
4. Close PowerShell and open it again.

You do not pick the Rust version yourself. The file `rust-toolchain.toml` in the project makes the first build download Rust 1.97.1.

If PowerShell says running scripts is disabled, stop and send Vector or Atlas that message. If it says `python` is not recognized, or Windows opens the Store, stop and tell them. Do not install a different Python unless they say so.

## Graphics check

Do this before you play, in the same PowerShell window. Close STRATA if a window is already open. These checks use a throwaway world, not your usual one. The first build can take several minutes and needs the internet. Write the three numbers down before you start playing, so later text does not push them off the screen.

**1.** Paste this and press Enter. You should only get the prompt back. No game window yet.

```powershell
Set-Location 'C:\WPAI\Software\Rust Based Voxel'
```

- [ ] Pass
- Notes:

**2.** Paste this and wait. Building can take several minutes. No game window should open.

```powershell
cargo test --locked native_gpu_validates_all_world_shader_pipelines -- --ignored --nocapture
```

- [ ] Pass
- Notes:

**3.** Near the end, a line should name `native_gpu_validates_all_world_shader_pipelines` and end in `ok`, and the next summary should say `test result: ok`. Copy both lines into the results sheet. A line that says `FAILED`, `native GPU adapter`, or `lacks per-draw immediates` is a fail. Paste that text instead.

- [ ] Pass
- Notes:

**4.** Paste this as one line. A STRATA window should open by itself, stay about 15 seconds, and close. Do not click it or press keys. If you see `fixture failed` or `Choose a new trace path`, stop and send Vector or Atlas that message. Do not delete any folders.

```powershell
python tools/create_streaming_fixture.py saves/checkerboard-upload-check; if ($LASTEXITCODE -ne 0) { throw 'fixture failed' }; .\trace-streaming.ps1 -TracePath evidence/streaming-packed-quad.jsonl -SaveDirectory saves/checkerboard-upload-check -SmokeSeconds 15; python tools/summarize_streaming_trace.py evidence/streaming-packed-quad.jsonl
```

- [ ] Pass
- Notes:

**5.** When that command finishes, PowerShell prints a block of quoted labels. In that block, copy three numbers and ignore `p50`, `p95`, and `p99`:

- `"max_payload_bytes_per_redraw"` — the number on that same line. This is the largest amount of world data sent in one redraw.
- `"upload_callback_ms_on_active_redraws"`, then the `"max"` inside it. This is the worst upload time, in milliseconds.
- `"admission_ms_on_active_redraws"`, then the `"max"` inside it. This is the worst admission time, in milliseconds.

If either `"max"` says `null`, or those labels are missing, that is a fail. Paste the whole block. The raw recording is `evidence\streaming-packed-quad.jsonl` if they ask for the file. The three numbers come from the printed block, not from that file.

- [ ] Pass
- Notes:

## Starting the game

**6.** In the same PowerShell window, paste `.\run.ps1` and press Enter. The window titled **STRATA — The Emerald Reach** should open, and PowerShell should print a line starting with `STRATA logs:`. Leave PowerShell open while you play.

The first build can take several minutes. If PowerShell says another STRATA window may already have the world open, close that window and run `.\run.ps1` again.

- [ ] Pass
- Notes:

## Starting a world

The usual world may already contain a small change from an earlier test. You will make your own marker later. The first time this new build opens an older world, the clock starts at 08:00 in the morning and the bottom row of blocks is the usual eight.

**7.** On the title screen, you should see the button **ENTER THE REACH** and world seed **728491**. A different number means this is not the usual world, so write it down and keep going.

- [ ] Pass
- Notes:

**8.** Click **ENTER THE REACH**, or press Enter. The mouse pointer should hide, a small crosshair should sit in the middle, and any panel that says **THE WORLD IS UNFOLDING** should finish and go away.

- [ ] Pass
- Notes:

## Walking and jumping

W moves you forward, A left, S back, and D right.

**9.** Hold W, then let go. You should ease up to speed, then coast to a stop, and A, S, and D should step you sideways or backward.

- [ ] Pass
- Notes:

**10.** Move the mouse. The view should follow it. Tap the arrow keys, and each tap should turn the view a little. If a note says the mouse cannot be captured, use the arrow keys to look.

- [ ] Pass
- Notes:

**11.** Press Space once. You should jump, and holding Space should hop you again when you land, without a second jump while you are still in the air.

- [ ] Pass
- Notes:

**12.** Walk at a one-block step. It should stop you, and a jump should be what gets you on top.

- [ ] Pass
- Notes:

**13.** Walk off a ledge and tap Space immediately. You should still jump, and a slow tap that misses is fine if you say so in the notes.

- [ ] Pass
- Notes:

## Sprint and sneak

Use the Shift and Ctrl keys on the left side of the keyboard. The ones on the right do nothing. On the ground, left Ctrl sneaks. In the air, the same key sinks.

**14.** Hold left Shift and W. You should go clearly faster than a walk, and left Shift with S, or with only A or D, should stay at a normal walk.

- [ ] Pass
- Notes:

**15.** On the ground, hold left Ctrl and walk toward a drop. You should creep, the top-left word should say **SNEAKING**, and you should stop at the edge.

- [ ] Pass
- Notes:

**16.** Keep holding left Ctrl and press Space at that edge. The jump should carry you off. Let go of Ctrl and walk off another ledge, and you should fall. Holding left Shift while sneaking should stay slow.

- [ ] Pass
- Notes:

## Flight

**17.** Press F. A note should say flight is on, with Space up and Ctrl down, the top-left should say **FREE FLIGHT**, and you should fly through the ground and trees.

- [ ] Pass
- Notes:

**18.** In the air, Space should rise, left Ctrl should sink, and left Shift should make flight much faster in every direction, including backward.

- [ ] Pass
- Notes:

**19.** In open air, press F again. A note should say you are walking, and you should drop if you are above the ground.

- [ ] Pass
- Notes:

**20.** Fly into a hill and press F while you are inside it. A note should tell you to move clear of the terrain. Fly back out, press F, and finish standing on the ground with the top-left saying **EXPLORING**.

- [ ] Pass
- Notes:

## Looking at and breaking blocks

Reach is five blocks. Farther than that is a fail.

**21.** Look at a block a few steps away, then at one across a gap. A pale box should outline only the near block.

- [ ] Pass
- Notes:

**22.** Left-click a nearby outlined block, or press X. That block should disappear.

- [ ] Pass
- Notes:

**23.** Hold the left mouse button. Blocks should keep breaking a few times a second until you let go. Hold X, and only one block should break.

- [ ] Pass
- Notes:

## Placing blocks and the hotbar

The row of eight blocks along the bottom is the hotbar. The selected one is highlighted, and its name sits just above the row. The four ores are not on this row. They are in the block list you open with E.

**24.** Select each slot with the number keys. A first visit with this build should show Meadow grass, Limestone, Timber, Canopy, River sand, Terracotta, Lumen crystal, then Basalt. The mouse wheel should move the highlight along the row.

- [ ] Pass
- Notes:

**25.** Select slot 7, Lumen crystal, and right-click or press C on a nearby face. A crystal should appear there. Holding the right mouse button should keep placing, and holding C should place only one.

- [ ] Pass
- Notes:

**26.** Aim at a spot inside your body and try to place. A note should tell you to step back, and you should not be stuck inside a new block.

- [ ] Pass
- Notes:

**27.** Place one Lumen crystal beside a hole you broke, somewhere you will recognize. You will come back to this spot after quitting.

- [ ] Pass
- Notes:

## The block picker

**28.** Press E. The mouse pointer should come back, a panel titled **MATERIALS** should open, and you should stay still, unable to look or break blocks. The top-left should say **CHOOSING A MATERIAL**.

- [ ] Pass
- Notes:

**29.** The panel should show these fourteen names, including the four ores: Meadow grass, Earth, Limestone, River sand, Timber, Canopy, Alpine snow, Terracotta, Lumen crystal, Basalt, Coal ore, Iron ore, Gold ore, and Diamond ore. Water is not in the list. A missing name is a fail.

- [ ] Pass
- Notes:

**30.** Press 1, click **Alpine snow**, and a note should say it is in slot 1. Press 4 so the panel shows slot 4 and Canopy, then press Esc once. The panel should close, the mouse should be captured, and the game should stay unpaused, with Alpine snow in slot 1 and Canopy selected.

- [ ] Pass
- Notes:

**31.** Press Esc again. The pause screen should open. Press Enter to return, and slot 4 should still be selected.

- [ ] Pass
- Notes:

## Day and night

A full day lasts 20 real minutes, and the clock moves only while you are playing. It is a 24-hour clock. F3 and F4 are the function keys along the top of the keyboard. F4 still reaches the game while a menu is open. The clock also creeps forward while you do these checks, so the hour and the word next to it matter more than an exact `:00`.

**32.** Press F3. **FIELD NOTES** should show a time, `08:00 morning` the first time this build opens an older world, plus the words `F4 +1 hour`, and a second press of F3 should hide the panel.

- [ ] Pass
- Notes:

**33.** With the notes hidden, F4 should leave the time unchanged. Show the notes, press F4 once, and the time should jump one hour, with that same time in a note on screen, such as `08:00 morning` becoming `09:00 morning`.

- [ ] Pass
- Notes:

**34.** Press Esc so the pause screen is up, with **FIELD NOTES** still open, and press F4. The time should jump another hour while that screen is open. Press Enter to go back to the world.

- [ ] Pass
- Notes:

**35.** With the notes open, tap F4 until it says 18:00 and **dusk**. The sky and the haze should look warm and orange.

- [ ] Pass
- Notes:

**36.** Tap F4 until it says 22:00 and **night**. The world should turn dark blue, and you should still be able to see the ground.

- [ ] Pass
- Notes:

**37.** Tap F4 until it says 06:00 and **dawn**, then until 12:00 and **midday**. Dawn should warm the horizon, and midday should be the brightest. Tap F4 until it says 18:00 and **dusk** again.

- [ ] Pass
- Notes:

**38.** Walk for about one minute without pressing Esc, and the clock should move forward by about an hour. Press Esc for half a minute and that time should hold, then press Enter and leave the clock where it is.

- [ ] Pass
- Notes:

## Saving and loading

The save keeps both the clock and the hotbar. The game also saves by itself about every 30 seconds after you enter the world. This section checks a save you chose.

**39.** Stand by your crystal, on foot, with flight off, Alpine snow in slot 1, and Canopy selected in slot 4. Press F3 if the notes are hidden, write down the exact time, and press F5. A note should say **World saved.**

- [ ] Pass
- Notes:

**40.** Press Esc, click **Save and quit**, and run `.\run.ps1` again after the window closes. You should return on foot to the same spot, with the crystal, the hole, Alpine snow in slot 1, and Canopy still selected.

- [ ] Pass
- Notes:

**41.** Press F3. The time should match what you wrote in step 39, and the sky should match that time of day. A few seconds of drift is fine. Waking at the start, in the morning, or with the old row of blocks, is a fail.

- [ ] Pass
- Notes:

## Pause and alt-tab

**42.** While exploring, press Esc. You should see a pause screen that says the world can wait, with **CONTINUE EXPLORING**, **Save world**, **Settings**, and **Save and quit**. Ignore Settings. Click **CONTINUE EXPLORING** or press Enter, and you should be playing again.

- [ ] Pass
- Notes:

**43.** Show FIELD NOTES, write down the time, and press Alt+Tab. The game should pause with the pointer free, and clicking back should leave it paused on that same time until you continue.

- [ ] Pass
- Notes:

## If something's wrong

Tell Vector or Atlas in chat. Give them the step number, what you did, and what happened instead. A picture helps. Press the Windows key, Shift, and S together, drag a box around the game, and paste the picture into the chat.

Each play run writes two notes files in:

`C:\WPAI\Software\Rust Based Voxel\logs`

The names look like `20261021-153045-a1b2c3d4-stdout.log` and a matching file ending in `-stderr.log`. PowerShell prints the full path on the line that starts with `STRATA logs:`. Attach both files if you can. The graphics numbers are in the PowerShell text from step 5. The recording behind them is `evidence\streaming-packed-quad.jsonl`.

Please leave the world files in `saves\emerald-reach` where they are. `world-a.json` and `world-b.json` are the world you just played. Leave `saves\checkerboard-upload-check` alone too.

## Results to paste back

Copy this, fill it in, and paste it to Vector or Atlas.

```
Milestone 1 playtest
Date:
PC: Windows, GTX 1660 Ti
Merge order confirmed (#3 and #2 merged, then #5, #4, #6, then this checklist): yes / no

GPU test line:

Largest payload (max_payload_bytes_per_redraw):
Worst upload time, milliseconds (upload_callback_ms_on_active_redraws max):
Worst admission time, milliseconds (admission_ms_on_active_redraws max):

Passed steps:
Failed steps:
Skipped steps:

1. In the project folder:
2. GPU test command:
3. GPU test result:
4. Upload timing command:
5. The three numbers:
6. Launch:
7. Title screen:
8. Enter the world:
9. Walk:
10. Look:
11. Jump:
12. Step up:
13. Jump just off a ledge:
14. Sprint:
15. Sneak:
16. Leave a sneak edge:
17. Flight on:
18. Fly up, down, and fast:
19. Flight off in the open:
20. Flight off inside the ground:
21. Block outline, five-block reach:
22. Break one block:
23. Hold to keep breaking:
24. Hotbar:
25. Place:
26. Step back to place:
27. Marker:
28. Open the block picker:
29. Fourteen blocks, including the four ores:
30. Alpine snow in slot 1:
31. Esc closes the picker, then pauses:
32. F3 clock:
33. F4 one hour:
34. F4 while the pause screen is open:
35. Dusk:
36. Night:
37. Dawn and midday:
38. Clock moves, pause holds:
39. F5 save:
40. Quit and reload place, edits, and hotbar:
41. Reload time:
42. Pause screen:
43. Alt-tab:

What went wrong:

Screenshot: yes / no
Log files attached: yes / no
```
