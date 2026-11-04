"""Flags, environment, path leaks, and the UTC zip contract for player builds."""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import player_zip  # noqa: E402
import repro_build  # noqa: E402

WINDOWS_TARGET = "x86_64-pc-windows-msvc"
CRT_AND_BREPRO = ["-C", "target-feature=+crt-static", "-C", "link-arg=/Brepro"]


class ReproBuildTests(unittest.TestCase):
    def test_windows_flags_repeat_only_crt_static_and_brepro(self) -> None:
        flags = repro_build.rustflags(WINDOWS_TARGET)
        head: list[str] = []
        for flag in flags:
            if flag == "--remap-path-prefix":
                break
            head.append(flag)
        # .cargo/config.toml is not read. A rustflag added only there is absent.
        self.assertEqual(head, CRT_AND_BREPRO)
        prefixes = self._remap_prefixes(flags)
        self.assertGreaterEqual(len(prefixes), 1)
        lengths = [len(prefix) for prefix in prefixes]
        self.assertEqual(lengths, sorted(lengths))

    def test_other_targets_do_not_take_the_windows_link_flags(self) -> None:
        for target in (None, "x86_64-unknown-linux-gnu"):
            flags = repro_build.rustflags(target)
            self.assertNotIn("target-feature=+crt-static", flags)
            self.assertNotIn("link-arg=/Brepro", flags)
            self.assertGreaterEqual(len(self._remap_prefixes(flags)), 1)

    def test_encoded_rustflags_separate_arguments_with_unit_separator(self) -> None:
        encoded = repro_build.encoded_rustflags(CRT_AND_BREPRO)
        self.assertEqual(encoded, "\x1f".join(CRT_AND_BREPRO))
        self.assertNotIn(" ", encoded)

    def test_environment_pins_epoch_incremental_and_encoded_flags(self) -> None:
        base = {
            "PATH": "kept",
            "RUSTFLAGS": "-C debuginfo=0",
            "SOURCE_DATE_EPOCH": "1",
        }
        env = repro_build.build_environment(base, 1792561988, CRT_AND_BREPRO)
        self.assertEqual(env["PATH"], "kept")
        self.assertEqual(env["RUSTFLAGS"], "-C debuginfo=0")
        self.assertEqual(env["SOURCE_DATE_EPOCH"], "1792561988")
        self.assertEqual(env["CARGO_INCREMENTAL"], "0")
        self.assertEqual(env["CARGO_ENCODED_RUSTFLAGS"], "\x1f".join(CRT_AND_BREPRO))
        self.assertEqual(base["SOURCE_DATE_EPOCH"], "1")

    def test_prefix_forms_follow_the_path_kind(self) -> None:
        self.assertEqual(repro_build._forms("ab"), set())
        self.assertEqual(repro_build._forms("/opt/cargo"), {"/opt/cargo"})
        self.assertEqual(
            repro_build._forms(r"C:\Users\runner\.cargo"),
            {r"C:\Users\runner\.cargo", "C:/Users/runner/.cargo"},
        )

    def test_scan_binary_rejects_utf8_and_utf16_leaks(self) -> None:
        prefix = r"C:\build\Strata"
        pairs = [(prefix, "strata-workspace")]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            leaked = root / "leaked.bin"
            leaked.write_bytes(b"xx" + prefix.encode("utf-8") + b"yy")
            wide = root / "wide.bin"
            wide.write_bytes(b"zz" + prefix.encode("utf-16le"))
            clean = root / "clean.bin"
            clean.write_bytes(b"strata-workspace only")
            for path in (leaked, wide):
                err = io.StringIO()
                with contextlib.redirect_stderr(err):
                    code = repro_build.scan_binary(path, pairs)
                self.assertEqual(code, 1)
                self.assertIn(repr(prefix), err.getvalue())
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                code = repro_build.scan_binary(clean, pairs)
            self.assertEqual(code, 0)
            self.assertIn("no absolute", out.getvalue())

    def test_zip_sorts_names_and_stores_utc(self) -> None:
        # 2026-01-01 00:00:00 UTC. A local-time stamp west of UTC would be 2025.
        epoch = 1767225600
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "strata.exe"
            guide = root / "PLAYING.md"
            binary.write_bytes(b"MZ-binary")
            guide.write_bytes(b"guide")
            first = root / "first.zip"
            second = root / "second.zip"
            entries = [
                ("strata.exe", binary, 0o755),
                ("PLAYING.md", guide, 0o644),
            ]
            player_zip.write_zip(first, list(reversed(entries)), epoch)
            player_zip.write_zip(second, entries, epoch + 1)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                self.assertEqual(
                    [info.filename for info in archive.infolist()],
                    ["PLAYING.md", "strata.exe"],
                )
                for info in archive.infolist():
                    self.assertEqual(info.date_time, (2026, 1, 1, 0, 0, 0))

    def _remap_prefixes(self, flags: list[str]) -> list[str]:
        prefixes: list[str] = []
        index = 0
        while index < len(flags):
            if flags[index] == "--remap-path-prefix":
                prefix, label = flags[index + 1].split("=", 1)
                self.assertNotEqual(label, "")
                prefixes.append(prefix)
                index += 2
                continue
            index += 1
        return prefixes


if __name__ == "__main__":
    unittest.main()
