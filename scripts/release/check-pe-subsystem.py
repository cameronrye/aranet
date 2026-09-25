#!/usr/bin/env python3
"""Check the Subsystem field of a Windows executable's PE optional header.

Usage: scripts/release/check-pe-subsystem.py <file.exe> <expected>

<expected> is 2 (IMAGE_SUBSYSTEM_WINDOWS_GUI: no console window) or
3 (IMAGE_SUBSYSTEM_WINDOWS_CUI: opens a console).
Reference: https://learn.microsoft.com/en-us/windows/win32/debug/pe-format#windows-subsystem

The release smoke test (.github/workflows/release-smoke-test.yml) runs this on
the installed aranet-gui.exe (expects 2) and aranet.exe (expects 3).
"""

import struct
import sys

if len(sys.argv) != 3:
    sys.exit("usage: check-pe-subsystem.py <file.exe> <expected>")
path, expected = sys.argv[1], int(sys.argv[2])
with open(path, "rb") as f:
    data = f.read()
pe = struct.unpack_from("<I", data, 0x3C)[0]
if data[pe : pe + 4] != b"PE\0\0":
    sys.exit(f"{path}: not a PE file")
# The optional header follows the 4-byte signature and the 20-byte COFF header.
# Subsystem is at offset 68 in both PE32 and PE32+ optional headers.
subsystem = struct.unpack_from("<H", data, pe + 24 + 68)[0]
print(f"{path}: subsystem={subsystem} (expected {expected})")
sys.exit(0 if subsystem == expected else 1)
