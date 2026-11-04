# Play STRATA

STRATA is a working title. This is an early development build of a voxel game, not a finished product. You do not need Rust or a code editor.

## Windows

On a Windows PC, download this file: **`strata-windows-x86_64.zip`**.

1. Open [the Releases page](https://github.com/MrWizard94-Compile/Strata/releases).
2. Under the latest release, download **`strata-windows-x86_64.zip`**.
3. Unzip it. You should see `strata.exe` and a copy of this page. The game does not need any other files.
4. Double-click `strata.exe`.

Windows may say "Windows protected your PC". This build is not signed. Click **More info**, then **Run anyway**.

The game needs Vulkan or DirectX 12. It does not open a console window, so a startup failure does not appear as a Windows message. If no window appears, open `strata-error.txt` in the same folder as `strata.exe`. The first line starts with `STRATA_START_FAILED`. A missing graphics driver is the usual cause.

`SHA256SUMS.txt` on that page is optional. It only checks that the zip was not corrupted.

If Releases has no Windows zip yet, open [Playable build](https://github.com/MrWizard94-Compile/Strata/actions/workflows/release.yml), click the newest run with a green check, and download **`strata-windows-x86_64.zip`** at the bottom. Those downloads are deleted after 30 days. A Release stays up.

## Linux

Linux also has a zip, `strata-linux-x86_64.zip`, on the same Releases page. There is no Mac download.

Unzip it, open a terminal in that folder, and run:

```sh
chmod +x strata
./strata
```

Use a normal desktop session, not a machine with no screen. You also need a Vulkan graphics driver. The file is a 64-bit program built on Ubuntu 22.04, so it needs glibc 2.35 or newer (Ubuntu 22.04 and most Linux systems from 2022 onward).

## How to play

On the title screen, click **ENTER THE REACH** or press Enter.

| Key | What it does |
|---|---|
| Enter | Enter the world, or resume from the pause menu |
| W A S D | Walk |
| Mouse | Look around |
| Space | Jump. While flying, this rises |
| Left Shift | Sprint |
| F | Turn flight on or off. You must move clear of solid ground before flight can turn off |
| Left Ctrl | Descend while flying |
| Left click, or X | Break the block you are looking at, up to five blocks away. Hold the mouse button to repeat |
| Right click, or C | Place the selected material, up to five blocks away. Hold the mouse button to repeat |
| 1 through 8, or the mouse wheel | Select a hotbar slot |
| E | Open or close the material picker for the selected hotbar slot |
| Arrow keys | Turn without the mouse. Hold an arrow key to keep turning |
| F5 | Save the world |
| F3 | Show or hide frame time, world statistics, and the clock |
| F4 | With the F3 overlay open, skip the clock forward one hour |
| Escape | Close the material picker, or pause. Escape again resumes |

**World settings** on the title screen changes how far you can see and how fast the mouse turns. **Quit** closes the game.

## Saves

The game stores the world in a folder named `saves/emerald-reach`, inside the folder you started it from. Double-clicking `strata.exe` uses the unzipped folder, so the saves appear next to the exe.

A backup should keep both `world-a.json` and `world-b.json`. Do not delete them while the game is open. Opening the same world a second time is refused until the first game closes.

This download does not write a log file. The `logs` folder exists only when someone builds from source and starts the game with `run.ps1`.

## If something goes wrong

Close the game and start it again. The last save should still be in `saves/emerald-reach`.

To report a problem, open a [new issue](https://github.com/MrWizard94-Compile/Strata/issues/new). Say what you clicked, what you expected, and what happened. If no window appears, say that you used `strata-windows-x86_64.zip` and paste the `STRATA_START_FAILED` line from `strata-error.txt`.
