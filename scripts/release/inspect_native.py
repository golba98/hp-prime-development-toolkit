"""Inspect runtime linkage of actual release executables; never execute foreign code."""

import argparse
import json
import re
import struct
import subprocess
from pathlib import Path


def inspect(file, target):
    data = Path(file).read_bytes()
    result = {"target": target, "file": str(file)}
    if "windows" in target:
        pe = struct.unpack_from("<I", data, 0x3C)[0]
        assert data[pe : pe + 4] == b"PE\0\0", "not a PE executable"
        machine, sections = struct.unpack_from("<HH", data, pe + 4)
        assert machine == 0x8664, "Windows artifact must be x86-64"
        optional = pe + 24
        assert struct.unpack_from("<H", data, optional)[0] == 0x20B, "PE32+ required"
        size = struct.unpack_from("<H", data, pe + 20)[0]
        table = optional + size

        def offset(rva):
            for index in range(sections):
                row = table + index * 40
                vsize, va, rawsize, raw = struct.unpack_from("<IIII", data, row + 8)
                if va <= rva < va + max(vsize, rawsize):
                    return raw + rva - va
            raise ValueError("Invalid PE RVA")

        rva = struct.unpack_from("<I", data, optional + 120)[0]
        imports = []
        if rva:
            row = offset(rva)
            while any(data[row : row + 20]):
                name = offset(struct.unpack_from("<I", data, row + 12)[0])
                imports.append(data[name : data.index(0, name)].decode("ascii"))
                row += 20
        assert not any(
            re.search(r"^(vcruntime|msvcp|ucrtbased)", dll, re.IGNORECASE)
            for dll in imports
        ), "unexpected redistributable runtime dependency"
        assert b"PerMonitorV2" in data and b"asInvoker" in data, (
            "DPI/asInvoker application manifest missing"
        )
        result.update(
            format="PE32+", imports=imports, msvc_runtime="static", dpi="PerMonitorV2"
        )
    elif "apple" in target:
        magic, cpu = struct.unpack_from("<II", data)
        assert magic == 0xFEEDFACF, "Mach-O 64 executable required"
        assert cpu == (0x0100000C if target.startswith("aarch64") else 0x01000007), (
            "wrong Mach-O architecture"
        )
        count = struct.unpack_from("<I", data, 16)[0]
        pos = 32
        imports = []
        minimum = None
        for _ in range(count):
            command, size = struct.unpack_from("<II", data, pos)
            if command in (0xC, 0x80000018, 0x8000001F):
                start = pos + struct.unpack_from("<I", data, pos + 8)[0]
                imports.append(data[start : data.index(0, start)].decode())
            if command == 0x32:
                minimum = struct.unpack_from("<I", data, pos + 12)[0]
            if command == 0x24:
                minimum = struct.unpack_from("<I", data, pos + 8)[0]
            pos += size
        assert all(
            name.startswith(("/usr/lib/", "/System/Library/")) for name in imports
        ), "non-system macOS runtime dependency"
        assert minimum is not None and minimum <= 0x000B0000, (
            "unexpected macOS minimum (expected 11.0 or earlier)"
        )
        result.update(
            format="Mach-O 64",
            imports=imports,
            minimum_macos=f"{minimum >> 16}.{(minimum >> 8) & 255}",
            developer_id="not verified",
            notarization="not performed",
        )
    else:
        assert data[:4] == b"\x7fELF", "not ELF"
        machine = struct.unpack_from("<H", data, 18)[0]
        assert machine == (183 if target.startswith("aarch64") else 62), (
            "wrong ELF architecture"
        )
        result.update(
            format="ELF",
            linkage=subprocess.check_output(["readelf", "-d", str(file)], text=True),
        )
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = inspect(args.binary, args.target)
    text = json.dumps(result, indent=2)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(text)
