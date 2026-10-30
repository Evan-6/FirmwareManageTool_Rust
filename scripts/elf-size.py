#!/usr/bin/env python3
"""Flash/RAM footprint of an RP2040 ELF, without the ARM toolchain.

Flash = allocated file-backed sections loaded into the XIP window (that includes
`.data`, whose initialiser bytes are stored in flash and copied at boot).
RAM   = allocated sections whose address is in SRAM (`.data`, `.bss`, `.uninit`).
"""
from pathlib import Path
import argparse
import struct

XIP_BASE = 0x10000000
XIP_LIMIT = 0x11000000
SRAM_BASE = 0x20000000
SRAM_LIMIT = 0x21000000

SHF_ALLOC = 0x2
SHT_NOBITS = 8


def sections(elf: bytes):
    shoff, = struct.unpack_from('<I', elf, 32)
    shentsize, shnum = struct.unpack_from('<HH', elf, 46)
    for index in range(shnum):
        base = shoff + index * shentsize
        _name, kind, flags, addr, _offset, size = struct.unpack_from('<IIIIII', elf, base)
        yield kind, flags, addr, size


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('elf', type=Path)
    args = parser.parse_args()

    flash = 0
    ram = 0
    for kind, flags, addr, size in sections(args.elf.read_bytes()):
        if not flags & SHF_ALLOC or size == 0:
            continue
        in_flash = XIP_BASE <= addr < XIP_LIMIT
        in_ram = SRAM_BASE <= addr < SRAM_LIMIT
        if in_flash and kind != SHT_NOBITS:
            flash += size
        elif in_ram:
            ram += size
            if kind != SHT_NOBITS:
                flash += size
    print(
        f'flash {flash} bytes ({flash / 1024:.1f} KiB of 2048 KiB), '
        f'ram {ram} bytes ({ram / 1024:.1f} KiB of 264 KiB)'
    )


if __name__ == '__main__':
    main()
