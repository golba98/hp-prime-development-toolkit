"""Real ZIP creation/extraction and fail-closed archive regression tests."""
import importlib.util
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import zipfile

helper=Path(__file__).with_name("native_zip.py")
spec=importlib.util.spec_from_file_location("native_zip",helper)
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class ArchiveTests(unittest.TestCase):
    def test_unicode_and_space_names_roundtrip(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);source=root/"hpdev-windows";source.mkdir()
            (source/"hpdev.exe").write_bytes(b"MZ"+bytes(range(256)))
            (source/"計算 é.txt").write_text("Program fixture",encoding="utf-8")
            archive=root/"release.zip"
            subprocess.run([sys.executable,"-B",str(helper),"create",str(archive),str(source)],check=True)
            subprocess.run([sys.executable,"-B",str(helper),"extract",str(archive),str(root/"out")],check=True)
            self.assertEqual((root/"out"/source.name/"hpdev.exe").read_bytes(),(source/"hpdev.exe").read_bytes())
            self.assertEqual((root/"out"/source.name/"計算 é.txt").read_bytes(),(source/"計算 é.txt").read_bytes())
    def test_traversal_links_duplicate_and_case_collisions_fail(self):
        for names,mode in [
            (["../private"],None),(["root\\private"],None),(["C:/file"],None),
            (["root/file","root/file"],None),(["root/File","root/file"],None),
            (["root/link"],stat.S_IFLNK|0o777),
        ]:
            with self.subTest(names=names),tempfile.TemporaryDirectory() as directory:
                path=Path(directory)/"bad.zip"
                with zipfile.ZipFile(path,"w") as archive:
                    for name in names:
                        item=zipfile.ZipInfo(name)
                        if mode:item.external_attr=mode<<16
                        archive.writestr(item,b"payload")
                with zipfile.ZipFile(path) as archive:
                    with self.assertRaises(ValueError):module.members(archive)
    def test_extraction_never_follows_an_existing_symlink(self):
        if not hasattr(__import__("os"),"symlink"):self.skipTest("no symlink support")
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);outside=root/"outside";outside.mkdir()
            target=root/"out";target.mkdir()
            try:(target/"root").symlink_to(outside,target_is_directory=True)
            except OSError:self.skipTest("symlinks require privileges on this runner")
            archive=root/"input.zip"
            with zipfile.ZipFile(archive,"w") as zip:zip.writestr("root/secret",b"data")
            result=subprocess.run([sys.executable,"-B",str(helper),"extract",str(archive),str(target)],capture_output=True)
            self.assertNotEqual(result.returncode,0)
            self.assertFalse((outside/"secret").exists())

if __name__=="__main__":unittest.main()
