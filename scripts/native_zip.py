"""Windows ZIP handling for release preparation/validation (Python stdlib only)."""
import json
import os
from pathlib import Path, PurePosixPath
import stat
import sys
import zipfile

MAX_MEMBER = 128 * 1024 * 1024

def members(archive):
    seen = set()
    result = []
    total = 0
    for item in archive.infolist():
        name = item.filename
        total += item.file_size
        if total > 512*1024*1024 or len(result)>100000:
            raise ValueError("ZIP exceeds distribution size/member limits")
        parts = PurePosixPath(name.rstrip("/")).parts
        if (not name or "\\" in name or ":" in name or name.startswith("/")
                or any(p in ("", ".", "..") for p in name.rstrip("/").split("/"))
                or name.rstrip("/").casefold() in seen or item.file_size > MAX_MEMBER):
            raise ValueError("Unsafe, duplicate or oversized ZIP member: " + repr(name))
        seen.add(name.rstrip("/").casefold())
        mode = item.external_attr >> 16
        if stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR)):
            raise ValueError("ZIP links and special files are forbidden")
        result.append({"name": name, "directory": item.is_dir(), "mode": mode & 0o777})
    return result

def main():
    operation, file, *args = sys.argv[1:]
    if operation == "create":
        source = Path(args[0]).resolve()
        with zipfile.ZipFile(file, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in [source, *sorted(source.rglob("*"))]:
                if path.is_symlink() or not (path.is_file() or path.is_dir()):
                    raise ValueError("Only ordinary files/directories may be packaged")
                archive.write(path, (Path(source.name) / path.relative_to(source)).as_posix())
        return
    with zipfile.ZipFile(file) as archive:
        listing = members(archive)
        if operation == "list":
            print(json.dumps(listing))
        elif operation == "read":
            sys.stdout.buffer.write(archive.read(args[0]))
        elif operation == "extract":
            destination = Path(args[0]).resolve()
            for row in listing:
                path = destination / row["name"]
                # Refuse traversal through existing links, including destination.
                if destination.is_symlink() or any(p.is_symlink() for p in path.parents):
                    raise ValueError("Extraction path contains a symlink")
                if row["directory"]:
                    path.mkdir(parents=True, exist_ok=True)
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(archive.read(row["name"]))
                    if os.name != "nt":
                        path.chmod(row["mode"] or 0o644)
        else:
            raise ValueError("Unknown ZIP operation")

if __name__ == "__main__":
    main()
