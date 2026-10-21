# STRATA Milestone 1 playtest

For Rob, on a Windows PC with an NVIDIA GTX 1660 Ti. STRATA is the working name. This pass checks the first set of playable controls. It is not the finished game.

## These pull requests have to be merged first

This checklist matches the game only after all three of these are merged into the `development` branch:

- [Pull request #2](https://github.com/MrWizard94-Compile/Strata/pull/2) — day and night (`cursor/day-night-cycle-26da`)
- [Pull request #4](https://github.com/MrWizard94-Compile/Strata/pull/4) — walking, sprint, and sneak (`cursor/player-movement-feel-8b4f`)
- [Pull request #6](https://github.com/MrWizard94-Compile/Strata/pull/6) — breaking, placing, and the block picker (`cursor/interaction-hotbar-picker-3aca`)

`development` already has the title screen, pause screen, breaking and placing, flight, F3, and F5 save. The three pull requests add the rest. If you are not sure they are in the copy you are about to launch, ask Vector or Atlas before you play. An older build will fail the new steps.

## How to start the game

The game on your PC lives in this folder:

`C:\WPAI\Software\Rust Based Voxel`

If that folder is not there, stop and tell Vector or Atlas. Do not download a different copy unless they tell you to.

Open PowerShell from the Start menu. Paste these two lines and press Enter:

```powershell
Set-Location 'C:\WPAI\Software\Rust Based Voxel'
.\run.ps1
```

The script builds the game, then opens it. The first build can take several minutes and needs the internet. Later launches are quicker. Leave the PowerShell window open while you play.

If PowerShell says `cargo` is not recognized, Rust is missing. Close that window and do this:

1. In a browser, open [https://rustup.rs](https://rustup.rs).
2. Download `rustup-init.exe` and run it.
3. Accept the default install. Choose the MSVC toolchain if it asks. That is the Windows one this project needs. If it says the C++ build tools are missing, let it install them.
4. Close PowerShell, open it again, and run the two lines above.

You do not pick the Rust version yourself. The file `rust-toolchain.toml` in the project makes the first build download Rust 1.97.1.

If PowerShell says another STRATA window may already have the world open, close that window and run `.\run.ps1` again. If it says running scripts is disabled, stop and send Vector or Atlas that message.

**1.** Run the two lines above. The window titled **STRATA — The Emerald Reach** should open, and PowerShell should print a line starting with `STRATA logs:`.

- [ ] Pass
- Notes:

## Starting a world

The usual world may already contain a small change from an earlier test. You will make your own marker later. The first time this new build opens an older world, the clock starts at 08:00 in the morning and the bottom row of blocks is the usual eight.

**2.** On the title screen, you should see the button **ENTER THE REACH** and world seed **728491**. A different number means this is not the usual world, so write it down and keep going.

- [ ] Pass
- Notes:

**3.** Click **ENTER THE REACH**, or press Enter. The mouse pointer should hide, a small crosshair should sit in the middle, and any panel that says **THE WORLD IS UNFOLDING** should finish and go away.

- [ ] Pass
- Notes:

## Walking and jumping

W moves you forward, A left, S back, and D right.

**4.** Hold W, then let go. You should ease up to speed, then coast to a stop, and A, S, and D should step you sideways or backward.

- [ ] Pass
- Notes:

**5.** Move the mouse. The view should follow it. Tap the arrow keys, and each tap should turn the view a little. If a note says the mouse cannot be captured, use the arrow keys to look.

- [ ] Pass
- Notes:

**6.** Press Space once. You should jump, and holding Space should hop you again when you land, without a second jump while you are still in the air.

- [ ] Pass
- Notes:

**7.** Walk at a one-block step. It should stop you, and a jump should be what gets you on top.

- [ ] Pass
- Notes:

**8.** Walk off a ledge and tap Space immediately. You should still jump, and a slow tap that misses is fine if you say so in the notes.

- [ ] Pass
- Notes:

## Sprint and sneak

Use the Shift and Ctrl keys on the left side of the keyboard. The ones on the right do nothing.

**9.** Hold left Shift and W. You should go clearly faster than a walk, and left Shift with S, or with only A or D, should stay at a normal walk.

- [ ] Pass
- Notes:

**10.** On the ground, hold left Ctrl and walk toward a drop. You should creep, the top-left word should say **SNEAKING**, and you should stop at the edge. If the creep and the edge-stop work but the word stays **EXPLORING**, mark Pass and write "no SNEAKING label".

- [ ] Pass
- Notes:

**11.** Keep holding left Ctrl and press Space at that edge. The jump should carry you off. Let go of Ctrl and walk off another ledge, and you should fall. Holding left Shift while sneaking should stay slow.

- [ ] Pass
- Notes:

## Flight

**12.** Press F. A note should say flight is on, with Space up and Ctrl down, the top-left should say **FREE FLIGHT**, and you should fly through the ground and trees.

- [ ] Pass
- Notes:

**13.** In the air, Space should rise, left Ctrl should sink, and left Shift should make flight much faster in every direction, including backward.

- [ ] Pass
- Notes:

**14.** In open air, press F again. A note should say you are walking, and you should drop if you are above the ground.

- [ ] Pass
- Notes:

**15.** Fly into a hill and press F while you are inside it. A note should tell you to move clear of the terrain. Fly back out, press F, and finish standing on the ground with the top-left saying **EXPLORING**.

- [ ] Pass
- Notes:

## Looking at and breaking blocks

**16.** Look at a nearby block, then at one much farther away. A pale box should outline only the nearby one, about five blocks away, and a farther outline should be noted as a longer reach.

- [ ] Pass
- Notes:

**17.** Left-click a nearby outlined block, or press X. That block should disappear.

- [ ] Pass
- Notes:

**18.** Hold the left mouse button. Blocks should keep breaking a few times a second until you let go. Hold X, and only one block should break.

- [ ] Pass
- Notes:

## Placing blocks and the hotbar

The row of eight blocks along the bottom is the hotbar. The selected one is highlighted, and its name sits just above the row.

**19.** Select each slot with the number keys. A first visit with this build should show Meadow grass, Limestone, Timber, Canopy, River sand, Terracotta, Lumen crystal, then Basalt. The mouse wheel should move the highlight along the row.

- [ ] Pass
- Notes:

**20.** Select slot 7, Lumen crystal, and right-click or press C on a nearby face. A crystal should appear there. Holding the right mouse button should keep placing, and holding C should place only one.

- [ ] Pass
- Notes:

**21.** Aim at a spot inside your body and try to place. A note should tell you to step back, and you should not be stuck inside a new block.

- [ ] Pass
- Notes:

**22.** Place one Lumen crystal beside a hole you broke, somewhere you will recognize. You will come back to this spot after quitting.

- [ ] Pass
- Notes:

## The block picker

**23.** Press E. The mouse pointer should come back, a panel titled **MATERIALS** should open, and you should stay still, unable to look or break blocks. The top-left should say **CHOOSING A MATERIAL**. If that word stays **EXPLORING**, mark Pass and write "no CHOOSING A MATERIAL label".

- [ ] Pass
- Notes:

**24.** The panel should show these ten names: Meadow grass, Earth, Limestone, River sand, Timber, Canopy, Alpine snow, Terracotta, Lumen crystal, and Basalt. Water is absent, and this build has no ore names. If you see an ore, write its name and check that you can click it. A missing name, or Water in the list, is a fail.

- [ ] Pass
- Notes:

**25.** Press 1, click **Alpine snow**, and a note should say it is in slot 1. Press 4 so the panel shows slot 4 and Canopy, then press Esc once. The panel should close, the mouse should be captured, and the game should stay unpaused, with Alpine snow in slot 1 and Canopy selected.

- [ ] Pass
- Notes:

**26.** Press Esc again. The pause screen should open. Press Enter to return, and slot 4 should still be selected.

- [ ] Pass
- Notes:

## Day and night

A full day lasts 20 real minutes, and the clock moves only while you are playing. It is a 24-hour clock. F3 and F4 are the function keys along the top of the keyboard. The clock also creeps forward while you do these checks, so the hour and the word next to it matter more than an exact `:00`.

**27.** Press F3. **FIELD NOTES** should show a time, `08:00 morning` the first time this build opens an older world, plus the words `F4 +1 hour`, and a second press of F3 should hide the panel.

- [ ] Pass
- Notes:

**28.** With the notes hidden, F4 should leave the time unchanged. Show the notes, press F4 once, and the time should jump one hour, with that same time in a note on screen, such as `08:00 morning` becoming `09:00 morning`.

- [ ] Pass
- Notes:

**29.** With the notes open, tap F4 until it says 18:00 and **dusk**. The sky and the haze should look warm and orange.

- [ ] Pass
- Notes:

**30.** Tap F4 until it says 22:00 and **night**. The world should turn dark blue, and you should still be able to see the ground.

- [ ] Pass
- Notes:

**31.** Tap F4 until it says 06:00 and **dawn**, then until 12:00 and **midday**. Dawn should warm the horizon, and midday should be the brightest. Tap F4 until it says 18:00 and **dusk** again.

- [ ] Pass
- Notes:

**32.** Walk for about one minute without pressing Esc, and the clock should move forward by about an hour. Press Esc for half a minute and that time should hold, then press Enter and leave the clock where it is.

- [ ] Pass
- Notes:

## Saving and loading

The game also saves by itself about every 30 seconds after you enter the world. This section checks a save you chose.

**33.** Stand by your crystal, on foot, with flight off, Alpine snow in slot 1, and Canopy selected in slot 4. Press F3 if the notes are hidden, write down the exact time, and press F5. A note should say **World saved.**

- [ ] Pass
- Notes:

**34.** Press Esc, click **Save and quit**, and run `.\run.ps1` again after the window closes. You should return on foot to the same spot, with the crystal, the hole, Alpine snow in slot 1, and Canopy still selected.

- [ ] Pass
- Notes:

**35.** Press F3. The time should match what you wrote in step 33, and the sky should match that time of day. A few seconds of drift is fine. Waking at the start, in the morning, or with the old row of blocks, is a fail.

- [ ] Pass
- Notes:

## Pause and alt-tab

**36.** While exploring, press Esc. You should see a pause screen that says the world can wait, with **CONTINUE EXPLORING**, **Save world**, **Settings**, and **Save and quit**. Ignore Settings. Click **CONTINUE EXPLORING** or press Enter, and you should be playing again.

- [ ] Pass
- Notes:

**37.** Show FIELD NOTES, write down the time, and press Alt+Tab. The game should pause with the pointer free, and clicking back should leave it paused on that same time until you continue.

- [ ] Pass
- Notes:

## If something's wrong

Tell Vector or Atlas in chat. Give them the step number, what you did, and what happened instead. A picture helps. Press the Windows key, Shift, and S together, drag a box around the game, and paste the picture into the chat.

Each run writes two notes files in:

`C:\WPAI\Software\Rust Based Voxel\logs`

The names look like `20261021-153045-a1b2c3d4-stdout.log` and a matching file ending in `-stderr.log`. PowerShell prints the full path on the line that starts with `STRATA logs:`. Attach both files if you can.

Please leave the world files in `saves\emerald-reach` where they are. `world-a.json` and `world-b.json` are the world you just played.

## Results to paste back

Copy this, fill it in, and paste it to Vector or Atlas.

```
Milestone 1 playtest
Date:
PC: Windows, GTX 1660 Ti
Build confirmed after PRs 2, 4, and 6 were merged: yes / no

Passed steps:
Failed steps:
Skipped steps:

1. Launch:
2. Title screen:
3. Enter the world:
4. Walk:
5. Look:
6. Jump:
7. Step up:
8. Jump just off a ledge:
9. Sprint:
10. Sneak:
11. Leave a sneak edge:
12. Flight on:
13. Fly up, down, and fast:
14. Flight off in the open:
15. Flight off inside the ground:
16. Block outline:
17. Break one block:
18. Hold to keep breaking:
19. Hotbar:
20. Place:
21. Step back to place:
22. Marker:
23. Open the block picker:
24. Every block, including ores if any:
25. Alpine snow in slot 1:
26. Esc closes the picker, then pauses:
27. F3 clock:
28. F4 one hour:
29. Dusk:
30. Night:
31. Dawn and midday:
32. Clock moves, pause holds:
33. F5 save:
34. Quit and reload place, edits, and hotbar:
35. Reload time:
36. Pause screen:
37. Alt-tab:

What went wrong:

Screenshot: yes / no
Log files attached: yes / no
```
