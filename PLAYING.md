# Play STRATA

STRATA is a working title. This is an early development build of a voxel game, not a finished product. You do not need Rust or a code editor.

## Download

1. Open [the Releases page](https://github.com/MrWizard94-Compile/Strata/releases).
2. On the latest release, download one zip:
   - Windows: `strata-windows-x86_64.zip`
   - Linux: `strata-linux-x86_64.zip`
3. There is no Mac download.
4. Unzip it. You should see the game (`strata.exe` on Windows, `strata` on Linux) and a copy of this page.
5. The release also has `SHA256SUMS.txt`. You can ignore that file. It only checks that the zip was not corrupted.

If Releases has no game yet, use a manual build instead. Open [Playable build](https://github.com/MrWizard94-Compile/Strata/actions/workflows/release.yml), click the newest run with a green check, and download the zip at the bottom of that page. Those downloads are deleted after 30 days. A Release stays up.

## Windows

Unzip the download if you have not already, then double-click `strata.exe`.

Windows may say "Windows protected your PC". This build is not signed. Click **More info**, then **Run anyway**.

The game does not open a black console window. If Windows says a file such as a DLL is missing, install the latest driver for your graphics card and try again.

## Linux

Unzip the download, open a terminal in that folder, and run:

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
| Left click, or X | Break the block you are looking at |
| Right click, or C | Place the selected material |
| 1 through 8, or the mouse wheel | Choose a material |
| Arrow keys | Turn without the mouse |
| F5 | Save the world |
| F3 | Show or hide frame time and world statistics |
| Escape | Pause. Escape again resumes |

**World settings** on the title screen changes how far you can see and how fast the mouse turns. **Quit** closes the game.

## Saves

The game stores the world in a folder named `saves/emerald-reach`, inside the folder you started it from. Double-clicking `strata.exe` on Windows uses the unzipped folder, so the saves appear next to the exe.

A backup should keep both `world-a.json` and `world-b.json`. Do not delete them while the game is open. Opening the same world a second time is refused until the first game closes.

This download does not write a log file. The `logs` folder exists only when someone builds from source and starts the game with `run.ps1`.

## If something goes wrong

Close the game and start it again. The last save should still be in `saves/emerald-reach`.

To report a problem, open a [new issue](https://github.com/MrWizard94-Compile/Strata/issues/new). Say what you clicked, what you expected, and what happened. If no window appears, say which zip you downloaded and the exact Windows or Linux message, if there was one.
