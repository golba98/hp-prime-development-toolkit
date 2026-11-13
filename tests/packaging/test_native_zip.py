"""Real ZIP creation/extraction and fail-closed archive regression tests."""

import importlib.util
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

helper = Path(__file__).resolve().parents[2] / "scripts/release/native_zip.py"
spec = importlib.util.spec_from_file_location("native_zip", helper)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ArchiveTests(unittest.TestCase):
    def test_timestamps_outside_zip_range_preserve_names_and_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "hpdev-windows"
            old_directory = source / "licenses"
            old_directory.mkdir(parents=True)
            old_notice = old_directory / "LICENSE.txt"
            old_notice.write_bytes(b"Original dependency licence\n")
            future_file = source / "future.txt"
            future_file.write_bytes(b"Future-dated file\n")
            for path, timestamp in [
                (source, 0),
                (old_directory, 0),
                (old_notice, 0),
                (future_file, 4354905600),
            ]:
                os.utime(path, (timestamp, timestamp))
            archive = root / "release.zip"
            subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(helper),
                    "create",
                    str(archive),
                    str(source),
                ],
                check=True,
            )
            with zipfile.ZipFile(archive) as result:
                for name in [
                    "hpdev-windows/",
                    "hpdev-windows/licenses/",
                    "hpdev-windows/licenses/LICENSE.txt",
                ]:
                    self.assertEqual(result.getinfo(name).date_time[:3], (1980, 1, 1))
                self.assertEqual(
                    result.getinfo("hpdev-windows/future.txt").date_time[:3],
                    (2107, 12, 31),
                )
                self.assertEqual(
                    result.read("hpdev-windows/licenses/LICENSE.txt"),
                    old_notice.read_bytes(),
                )
                self.assertEqual(
                    result.read("hpdev-windows/future.txt"), future_file.read_bytes()
                )

    def test_unicode_and_space_names_roundtrip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "hpdev-windows"
            source.mkdir()
            (source / "hpdev.exe").write_bytes(b"MZ" + bytes(range(256)))
            (source / "計算 é.txt").write_text("Program fixture", encoding="utf-8")
            archive = root / "release.zip"
            subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(helper),
                    "create",
                    str(archive),
                    str(source),
                ],
                check=True,
            )
            subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(helper),
                    "extract",
                    str(archive),
                    str(root / "out"),
                ],
                check=True,
            )
            self.assertEqual(
                (root / "out" / source.name / "hpdev.exe").read_bytes(),
                (source / "hpdev.exe").read_bytes(),
            )
            self.assertEqual(
                (root / "out" / source.name / "計算 é.txt").read_bytes(),
                (source / "計算 é.txt").read_bytes(),
            )

    def test_traversal_links_duplicate_and_case_collisions_fail(self):
        for names, mode in [
            (["../private"], None),
            (["root\\private"], None),
            (["C:/file"], None),
            (["root/file", "root/file"], None),
            (["root/File", "root/file"], None),
            (["root/link"], stat.S_IFLNK | 0o777),
        ]:
            with self.subTest(names=names), tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "bad.zip"
                with zipfile.ZipFile(path, "w") as archive:
                    for name in names:
                        item = zipfile.ZipInfo(name)
                        # Preserve hostile separators even on Windows, where
                        # ZipInfo's constructor normally rewrites them to '/'.
                        item.filename = name
                        if mode:
                            item.external_attr = mode << 16
                        archive.writestr(item, b"payload")
                with zipfile.ZipFile(path) as archive, self.assertRaises(ValueError):
                    module.members(archive)

    def test_extraction_never_follows_an_existing_symlink(self):
        if not hasattr(__import__("os"), "symlink"):
            self.skipTest("no symlink support")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outside = root / "outside"
            outside.mkdir()
            target = root / "out"
            target.mkdir()
            try:
                (target / "root").symlink_to(outside, target_is_directory=True)
            except OSError:
                self.skipTest("symlinks require privileges on this runner")
            archive = root / "input.zip"
            with zipfile.ZipFile(archive, "w") as zip:
                zip.writestr("root/secret", b"data")
            result = subprocess.run(
                [
                    sys.executable,
                    "-B",
                    str(helper),
                    "extract",
                    str(archive),
                    str(target),
                ],
                capture_output=True,
                check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((outside / "secret").exists())


if __name__ == "__main__":
    unittest.main()
