#!/usr/bin/env python3
"""Convert an RP2040 ELF to UF2 without picotool's binary-info requirement.

picotool refuses a Rust ELF that has no `.bi_entries` ABI block, so this script
reads the ELF program headers itself: every loadable segment whose physical
address sits in flash becomes a run of 256-byte UF2 blocks with the RP2040 family
id. Boot2 at 0x10000000 is included because it is a normal loadable section.
"""
from pathlib import Path
import argparse
import struct

UF2_MAGIC_START0 = 0x0A324655
UF2_MAGIC_START1 = 0x9E5D5157
UF2_MAGIC_END = 0x0AB16F30
UF2_FLAG_FAMILY_ID = 0x00002000
RP2040_FAMILY_ID = 0xE48BFF56

XIP_BASE = 0x10000000
FLASH_LIMIT = 0x11000000
PAYLOAD = 256


def loadable_segments(elf: bytes):
    if elf[:4] != b'\x7fELF' or elf[4] != 1 or elf[5] != 1:
        raise SystemExit('not a little-endian ELF32 file')
    phoff, = struct.unpack_from('<I', elf, 28)
    phentsize, phnum = struct.unpack_from('<HH', elf, 42)
    for index in range(phnum):
        base = phoff + index * phentsize
        p_type, p_offset, _vaddr, paddr, filesz, _memsz, _flags, _align = struct.unpack_from(
            '<IIIIIIII', elf, base)
        if p_type == 1 and filesz and XIP_BASE <= paddr < FLASH_LIMIT:
            yield paddr, elf[p_offset:p_offset + filesz]


def blocks(elf: bytes):
    # The RP2040 BOOTSEL bootloader only consumes 256-byte aligned blocks, so
    # flatten the flash segments into one aligned image instead of emitting each
    # segment's own (possibly unaligned) runs. Gaps stay erased (0xFF).
    segments = sorted(loadable_segments(elf), key=lambda segment: segment[0])
    if not segments:
        raise SystemExit('no flash segments found; is this an RP2040 build?')
    start = segments[0][0] & ~(PAYLOAD - 1)
    end = max(address + len(data) for address, data in segments)
    end = (end + PAYLOAD - 1) & ~(PAYLOAD - 1)
    image = bytearray(bytes([0xFF]) * (end - start))
    for address, data in segments:
        image[address - start:address - start + len(data)] = data

    chunks = [(start + offset, bytes(image[offset:offset + PAYLOAD]))
              for offset in range(0, len(image), PAYLOAD)]
    total = len(chunks)
    for number, (address, payload) in enumerate(chunks):
        block = struct.pack('<IIIIIIII', UF2_MAGIC_START0, UF2_MAGIC_START1,
                            UF2_FLAG_FAMILY_ID, address, len(payload), number, total,
                            RP2040_FAMILY_ID)
        block += payload.ljust(476, b'\x00')
        block += struct.pack('<I', UF2_MAGIC_END)
        yield block


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('elf', type=Path)
    parser.add_argument('uf2', type=Path)
    args = parser.parse_args()
    data = args.elf.read_bytes()
    image = b''.join(blocks(data))
    args.uf2.write_bytes(image)
    print(f'{args.uf2}: {len(image) // 512} blocks, {len(image)} bytes')


if __name__ == '__main__':
    main()
