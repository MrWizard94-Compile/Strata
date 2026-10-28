"""The player zip writer is byte-stable and stores the commit time, not the clock."""

from __future__ import annotations

import struct
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import player_zip  # noqa: E402


class PlayerZipTests(unittest.TestCase):
    def test_two_writes_match_and_follow_the_epoch(self) -> None:
        epoch = 1792561989  # odd second; DOS time stores the even second below it
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "strata.exe"
            guide = root / "PLAYING.md"
            binary.write_bytes(b"MZ" + bytes(range(256)) * 40)
            guide.write_text("play\n", encoding="utf-8")
            first = root / "a.zip"
            second = root / "b.zip"
            entries = [
                ("strata.exe", binary, 0o755),
                ("PLAYING.md", guide, 0o644),
            ]
            player_zip.write_zip(first, list(reversed(entries)), epoch)
            player_zip.write_zip(second, entries, epoch)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                names = [info.filename for info in archive.infolist()]
                self.assertEqual(names, ["PLAYING.md", "strata.exe"])
                for info in archive.infolist():
                    self.assertEqual(info.extra, b"")
                    self.assertEqual(info.flag_bits, 0)
                    self.assertEqual(info.date_time, (2026, 10, 21, 5, 53, 8))
                binary_info = archive.getinfo("strata.exe")
                guide_info = archive.getinfo("PLAYING.md")
                self.assertTrue((binary_info.external_attr >> 16) & 0o111)
                self.assertFalse((guide_info.external_attr >> 16) & 0o111)
                self.assertEqual(archive.read("PLAYING.md"), b"play\n")

    def test_adjacent_epoch_in_the_same_dos_slot_matches(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = root / "note.txt"
            payload.write_bytes(b"same")
            even = root / "even.zip"
            odd = root / "odd.zip"
            later = root / "later.zip"
            player_zip.write_zip(even, [("note.txt", payload, 0o644)], 1792561988)
            player_zip.write_zip(odd, [("note.txt", payload, 0o644)], 1792561989)
            player_zip.write_zip(later, [("note.txt", payload, 0o644)], 1792561990)
            self.assertEqual(even.read_bytes(), odd.read_bytes())
            self.assertNotEqual(even.read_bytes(), later.read_bytes())

    def test_compare_reports_a_match(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            blob = root / "blob"
            blob.write_bytes(b"abc")
            left = root / "left.zip"
            right = root / "right.zip"
            player_zip.write_zip(left, [("blob", blob, 0o644)], 1_700_000_000)
            player_zip.write_zip(right, [("blob", blob, 0o644)], 1_700_000_000)
            self.assertEqual(player_zip.compare_zips(left, right), 0)

    def test_pe_summary_reads_the_timestamp(self) -> None:
        blob = bytearray(128)
        blob[0:2] = b"MZ"
        struct.pack_into("<I", blob, 0x3C, 64)
        blob[64:68] = b"PE\x00\x00"
        struct.pack_into("<I", blob, 72, 1792561988)
        blob[90:94] = b"RSDS"
        summary = player_zip._pe_summary(bytes(blob))
        self.assertIn("TimeDateStamp=1792561988", summary)
        self.assertIn("RSDS=", summary)


if __name__ == "__main__":
    unittest.main()
