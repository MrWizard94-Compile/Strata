#!/usr/bin/env python3
"""Deterministic player zip for STRATA release builds.

The zip stores UTC DOS timestamps from SOURCE_DATE_EPOCH (2-second resolution),
sorted names, fixed Unix modes, and no extra fields. Deflate uses zlib level 9.
Two runs on the same interpreter and the same epoch are byte-identical.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import struct
import subprocess
import sys
import zlib
from datetime import datetime, timezone
from pathlib import Path

DOS_EPOCH = 315532800  # 1980-01-01 UTC
LOCAL_SIG = 0x04034B50
CENTRAL_SIG = 0x02014B50
EOCD_SIG = 0x06054B50
METHOD_DEFLATE = 8
VERSION_NEEDED = 20
VERSION_MADE = (3 << 8) | 20  # Unix, zip 2.0
GUIDE_NAME = "PLAYING.md"
GUIDE_MODE = 0o644
BINARY_MODE = 0o755


def source_date_epoch() -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH")
    if raw is None or raw == "":
        raw = subprocess.check_output(
            ["git", "log", "-1", "--format=%ct"], text=True
        ).strip()
    if not raw.isdigit():
        raise SystemExit(f"SOURCE_DATE_EPOCH must be a unix time, got {raw!r}")
    epoch = int(raw)
    if epoch < DOS_EPOCH:
        raise SystemExit("SOURCE_DATE_EPOCH is before 1980; zip DOS time cannot store it")
    return epoch


def dos_datetime(epoch: int) -> tuple[int, int]:
    """UTC DOS time and date. Seconds are rounded down to an even second."""
    when = datetime.fromtimestamp(epoch - (epoch % 2), timezone.utc)
    if when.year < 1980 or when.year > 2107:
        raise SystemExit(f"zip DOS date cannot store {when.isoformat()}")
    clock = (when.hour << 11) | (when.minute << 5) | (when.second // 2)
    day = ((when.year - 1980) << 9) | (when.month << 5) | when.day
    return clock, day


def _deflate(payload: bytes) -> bytes:
    compressor = zlib.compressobj(level=9, method=zlib.DEFLATED, wbits=-15)
    return compressor.compress(payload) + compressor.flush()


def _external_attr(mode: int) -> int:
    return (0o100000 | mode) << 16


def write_zip(output: Path, entries: list[tuple[str, Path, int]], epoch: int) -> None:
    """Write entries (archive name, source path, unix mode) into one zip."""
    clock, day = dos_datetime(epoch)
    ordered = sorted(entries, key=lambda item: item[0].encode("ascii"))
    seen: set[str] = set()
    chunks: list[bytes] = []
    central: list[bytes] = []
    offset = 0
    for name, source, mode in ordered:
        if name in seen or "/" in name or "\\" in name or not name.isascii():
            raise SystemExit(f"zip entry name is not a flat ASCII file: {name!r}")
        seen.add(name)
        payload = source.read_bytes()
        compressed = _deflate(payload)
        crc = zlib.crc32(payload) & 0xFFFFFFFF
        name_b = name.encode("ascii")
        local = struct.pack(
            "<IHHHHHIIIHH",
            LOCAL_SIG,
            VERSION_NEEDED,
            0,
            METHOD_DEFLATE,
            clock,
            day,
            crc,
            len(compressed),
            len(payload),
            len(name_b),
            0,
        )
        chunks.append(local + name_b + compressed)
        central.append(
            struct.pack(
                "<IHHHHHHIIIHHHHHII",
                CENTRAL_SIG,
                VERSION_MADE,
                VERSION_NEEDED,
                0,
                METHOD_DEFLATE,
                clock,
                day,
                crc,
                len(compressed),
                len(payload),
                len(name_b),
                0,
                0,
                0,
                0,
                _external_attr(mode),
                offset,
            )
            + name_b
        )
        offset += len(chunks[-1])
    central_blob = b"".join(central)
    eocd = struct.pack(
        "<IHHHHIIH",
        EOCD_SIG,
        0,
        0,
        len(ordered),
        len(ordered),
        len(central_blob),
        offset,
        0,
    )
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(output.suffix + ".tmp")
    temporary.write_bytes(b"".join(chunks) + central_blob + eocd)
    temporary.replace(output)


def package_player(output: Path, player: str, binary: Path, guide: Path, epoch: int) -> None:
    archive_name = {"windows": "strata.exe", "linux": "strata"}.get(player)
    if archive_name is None:
        raise SystemExit(f"unknown player {player!r}")
    if not binary.is_file():
        raise SystemExit(f"binary not found: {binary}")
    if not guide.is_file():
        raise SystemExit(f"player guide not found: {guide}")
    write_zip(
        output,
        [
            (archive_name, binary, BINARY_MODE),
            (GUIDE_NAME, guide, GUIDE_MODE),
        ],
        epoch,
    )


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _payloads(path: Path) -> list[tuple[str, bytes]]:
    import zipfile

    with zipfile.ZipFile(path) as archive:
        return [(info.filename, archive.read(info)) for info in archive.infolist()]


def explain_difference(left: Path, right: Path) -> None:
    import zipfile

    print("zip hashes differ", file=sys.stderr)
    for label, path in (("left", left), ("right", right)):
        with zipfile.ZipFile(path) as archive:
            print(f"[{label}] {path}", file=sys.stderr)
            for info in archive.infolist():
                print(
                    f"  {info.filename} date={info.date_time} method={info.compress_type} "
                    f"flag={info.flag_bits:#x} attr={info.external_attr:#x} "
                    f"extra={info.extra.hex()} crc={info.CRC:#x} size={info.file_size}",
                    file=sys.stderr,
                )
    left_files = _payloads(left)
    right_files = _payloads(right)
    if [name for name, _ in left_files] != [name for name, _ in right_files]:
        print("entry names differ", file=sys.stderr)
        return
    payloads_match = True
    for (name, a), (_, b) in zip(left_files, right_files):
        if a == b:
            print(f"payload identical: {name}", file=sys.stderr)
            continue
        payloads_match = False
        print(f"payload differs: {name} ({len(a)} vs {len(b)} bytes)", file=sys.stderr)
        _explain_bytes(name, a, b)
    if payloads_match:
        print(
            "uncompressed payloads match; the zip container (timestamps, order, or attributes) differs",
            file=sys.stderr,
        )


def _explain_bytes(name: str, left: bytes, right: bytes) -> None:
    limit = min(len(left), len(right))
    index = next((i for i in range(limit) if left[i] != right[i]), limit)
    print(f"  first difference in {name} at offset {index}", file=sys.stderr)
    if left[:2] == b"MZ" or right[:2] == b"MZ":
        for label, blob in (("left", left), ("right", right)):
            print(f"  {label} {_pe_summary(blob)}", file=sys.stderr)
    start = max(0, index - 16)
    print(f"  left  {left[start:index + 16].hex()}", file=sys.stderr)
    print(f"  right {right[start:index + 16].hex()}", file=sys.stderr)


def _pe_summary(blob: bytes) -> str:
    if len(blob) < 0x40 or blob[:2] != b"MZ":
        return "not a PE"
    e_lfanew = struct.unpack_from("<I", blob, 0x3C)[0]
    if e_lfanew + 8 > len(blob) or blob[e_lfanew : e_lfanew + 4] != b"PE\x00\x00":
        return "PE header missing"
    timestamp = struct.unpack_from("<I", blob, e_lfanew + 8)[0]
    text = f"TimeDateStamp={timestamp}"
    # CODEVIEW sits in the file at the debug raw pointer. Search the small RSDS marker.
    marker = blob.find(b"RSDS")
    if marker != -1 and marker + 24 <= len(blob):
        guid = blob[marker + 4 : marker + 20].hex()
        pdb = blob[marker + 24 : marker + 24 + 64].split(b"\x00", 1)[0]
        text += f" RSDS={guid} pdb={pdb.decode('ascii', 'replace')}"
    return text


def compare_zips(left: Path, right: Path) -> int:
    left_hash = sha256_file(left)
    right_hash = sha256_file(right)
    print(f"{left_hash}  {left.name}")
    print(f"{right_hash}  {right.name}")
    if left_hash == right_hash:
        print(f"reproducible {left_hash}")
        return 0
    explain_difference(left, right)
    return 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Write or compare deterministic player zips.")
    sub = parser.add_subparsers(dest="command", required=True)
    package = sub.add_parser("package")
    package.add_argument("--player", choices=("windows", "linux"), required=True)
    package.add_argument("--binary", type=Path, required=True)
    package.add_argument("--guide", type=Path, default=Path("PLAYING.md"))
    package.add_argument("--output", type=Path, required=True)
    compare = sub.add_parser("compare")
    compare.add_argument("left", type=Path)
    compare.add_argument("right", type=Path)
    args = parser.parse_args(argv)
    if args.command == "package":
        epoch = source_date_epoch()
        print(f"SOURCE_DATE_EPOCH={epoch}")
        package_player(args.output, args.player, args.binary, args.guide, epoch)
        print(f"{sha256_file(args.output)}  {args.output.name}")
        return 0
    return compare_zips(args.left, args.right)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
