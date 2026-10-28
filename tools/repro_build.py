#!/usr/bin/env python3
"""Release build for player zips.

Cargo treats CARGO_ENCODED_RUSTFLAGS as the only rustflags source, so this
script repeats the Windows +crt-static flag from .cargo/config.toml instead of
inheriting it. Path prefixes are remapped to stable labels. On MSVC the linker
also gets /Brepro so the PE timestamp and CodeView id come from the inputs.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import player_zip  # noqa: E402

WORKSPACE_LABEL = "strata-workspace"
CARGO_LABEL = "strata-cargo"
RUSTUP_LABEL = "strata-rustup"
UNIT = "\x1f"


def _forms(prefix: str) -> set[str]:
    text = prefix.strip().rstrip("\\/")
    if len(text) < 3:
        return set()
    forms = {text}
    # Windows paths show up with both separators. A Unix prefix must not grow
    # a backslash twin; that twin is not a path rustc emits.
    if "\\" in text or (len(text) > 2 and text[1] == ":"):
        forms.add(text.replace("\\", "/"))
        forms.add(text.replace("/", "\\"))
    return forms


def _rustup_roots() -> list[str]:
    roots: list[str] = []
    if os.environ.get("RUSTUP_HOME"):
        roots.append(os.environ["RUSTUP_HOME"])
    roots.append(str(Path.home() / ".rustup"))
    try:
        sysroot = subprocess.check_output(
            ["rustc", "--print", "sysroot"], text=True
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return roots
    if sysroot:
        path = Path(sysroot)
        if path.parent.name == "toolchains":
            roots.append(str(path.parent.parent))
        roots.append(sysroot)
    return roots


def remap_pairs() -> list[tuple[str, str]]:
    workspace = subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], text=True
    ).strip()
    cargo_home = os.environ.get("CARGO_HOME") or str(Path.home() / ".cargo")
    labeled = (
        (workspace, WORKSPACE_LABEL),
        (cargo_home, CARGO_LABEL),
        *[(root, RUSTUP_LABEL) for root in _rustup_roots()],
    )
    pairs: list[tuple[str, str]] = []
    seen: set[str] = set()
    for prefix, label in labeled:
        for form in _forms(prefix):
            if form in seen:
                continue
            seen.add(form)
            pairs.append((form, label))
    # rustc applies the last matching prefix. Longer paths must win.
    pairs.sort(key=lambda item: len(item[0]))
    return pairs


def rustflags(target: str | None) -> list[str]:
    flags: list[str] = []
    if target == "x86_64-pc-windows-msvc":
        flags.extend(["-C", "target-feature=+crt-static", "-C", "link-arg=/Brepro"])
    for prefix, label in remap_pairs():
        flags.extend(["--remap-path-prefix", f"{prefix}={label}"])
    return flags


def encoded_rustflags(flags: list[str]) -> str:
    return UNIT.join(flags)


def scan_binary(path: Path, pairs: list[tuple[str, str]]) -> int:
    data = path.read_bytes()
    hits: list[str] = []
    for prefix, _label in pairs:
        for encoding, blob in (
            ("utf-8", prefix.encode("utf-8")),
            ("utf-16le", prefix.encode("utf-16le")),
        ):
            index = data.find(blob)
            if index == -1:
                continue
            excerpt = data[max(0, index - 12) : index + len(blob) + 24]
            shown = "".join(chr(byte) if 32 <= byte < 127 else "." for byte in excerpt)
            hits.append(f"{encoding} {prefix!r} at {index:#x}: {shown}")
    if hits:
        print(f"{path} still contains absolute build paths:", file=sys.stderr)
        for hit in hits[:12]:
            print(f"  {hit}", file=sys.stderr)
        return 1
    print(f"no absolute workspace, cargo, or rustup paths in {path}")
    return 0


def output_binary(target_dir: Path, target: str | None) -> Path:
    directory = target_dir / target if target else target_dir
    directory = directory / "release"
    for name in ("strata.exe", "strata"):
        candidate = directory / name
        if candidate.is_file():
            return candidate
    raise SystemExit(f"release binary not found under {directory}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Reproducible STRATA release build.")
    parser.add_argument("--target", default=os.environ.get("STRATA_TARGET"))
    parser.add_argument("--target-dir", type=Path, default=Path("target"))
    parser.add_argument(
        "--scan",
        type=Path,
        help="Only check this binary for absolute build paths.",
    )
    args = parser.parse_args(argv)
    pairs = remap_pairs()
    if args.scan:
        return scan_binary(args.scan, pairs)
    epoch = player_zip.source_date_epoch()
    flags = rustflags(args.target)
    env = os.environ.copy()
    env["SOURCE_DATE_EPOCH"] = str(epoch)
    env["CARGO_INCREMENTAL"] = "0"
    env["CARGO_ENCODED_RUSTFLAGS"] = encoded_rustflags(flags)
    print(f"SOURCE_DATE_EPOCH={epoch}")
    for prefix, label in pairs:
        print(f"remap {prefix} -> {label}")
    command = [
        "cargo",
        "build",
        "--release",
        "--locked",
        "--target-dir",
        str(args.target_dir),
    ]
    if args.target:
        command.extend(["--target", args.target])
    completed = subprocess.run(command, env=env)
    if completed.returncode != 0:
        return completed.returncode
    return scan_binary(output_binary(args.target_dir, args.target), pairs)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
