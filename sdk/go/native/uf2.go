package gdbnative

import (
	"bytes"
	"encoding/binary"
	"sort"
)

// RP2040 ELF → UF2 packaging, ported from sdk/rust/src/flash/uf2.rs.
//
// Two details matter and both were learned the hard way on hardware: every
// block must be 256-byte aligned (the BOOTSEL bootloader silently ignores a
// file that is not), and the flash segments have to be flattened into one image
// because a segment's file order is not its address order — `.boot2` regularly
// comes last in the file while it has to be at 0x10000000.

// Family id RP2040 writes into every block.
const (
	FamilyRP2040 = 0xE48BFF56
	XIPBase      = 0x10000000
	XIPLimit     = 0x11000000
	// UF2BlockSize is one UF2 block; UF2Payload is the flash bytes it carries.
	UF2BlockSize = 512
	UF2Payload   = 256
)

const (
	uf2MagicStart0 = 0x0A324655
	uf2MagicStart1 = 0x9E5D5157
	uf2MagicEnd    = 0x0AB16F30
	uf2FlagFamily  = 0x00002000
	elfProgramLoad = 1
	elfHeaderSize  = 52
)

// UFPack packs the flash image of an RP2040 ELF into a UF2 file.
func UFPack(elf []byte) ([]byte, error) {
	if len(elf) <= elfHeaderSize || !bytes.Equal(elf[:4], []byte{0x7f, 'E', 'L', 'F'}) {
		return nil, errorf(Invalid, "不是 ELF 檔")
	}
	if elf[4] != 1 || elf[5] != 1 {
		return nil, errorf(Invalid, "只支援 32-bit little-endian ELF")
	}
	headerOffset, err := u32(elf, 28)
	if err != nil {
		return nil, err
	}
	entrySize, err := u16(elf, 42)
	if err != nil {
		return nil, err
	}
	count, err := u16(elf, 44)
	if err != nil {
		return nil, err
	}

	type segment struct {
		address uint32
		data    []byte
	}
	segments := make([]segment, 0, count)
	for index := 0; index < int(count); index++ {
		base := int(headerOffset) + index*int(entrySize)
		if base+32 > len(elf) {
			return nil, errorf(Invalid, "program header 超出檔案")
		}
		kind, err := u32(elf, base)
		if err != nil {
			return nil, err
		}
		offset, err := u32(elf, base+4)
		if err != nil {
			return nil, err
		}
		address, err := u32(elf, base+12)
		if err != nil {
			return nil, err
		}
		size, err := u32(elf, base+16)
		if err != nil {
			return nil, err
		}
		if kind != elfProgramLoad || size == 0 || address < XIPBase || address >= XIPLimit {
			continue
		}
		end := uint64(offset) + uint64(size)
		if end > uint64(len(elf)) {
			return nil, errorf(Invalid, "segment 超出檔案")
		}
		segments = append(segments, segment{
			address: address,
			data:    elf[offset:end],
		})
	}
	if len(segments) == 0 {
		return nil, errorf(Invalid, "ELF 沒有任何 flash segment；這不是 RP2040 建置？")
	}
	sort.SliceStable(segments, func(left, right int) bool {
		return segments[left].address < segments[right].address
	})

	// Flatten to one 256-byte aligned image; gaps stay erased (0xFF).
	start := segments[0].address & ^uint32(UF2Payload-1)
	end := start
	for _, part := range segments {
		if limit := part.address + uint32(len(part.data)); limit > end {
			end = limit
		}
	}
	end = (end + UF2Payload - 1) & ^uint32(UF2Payload-1)
	image := bytes.Repeat([]byte{0xFF}, int(end-start))
	for _, part := range segments {
		at := int(part.address - start)
		copy(image[at:], part.data)
	}

	total := len(image) / UF2Payload
	out := make([]byte, 0, total*UF2BlockSize)
	block := make([]byte, UF2BlockSize)
	for number := 0; number < total; number++ {
		header := [8]uint32{
			uf2MagicStart0,
			uf2MagicStart1,
			uf2FlagFamily,
			start + uint32(number*UF2Payload),
			UF2Payload,
			uint32(number),
			uint32(total),
			FamilyRP2040,
		}
		for index, value := range header {
			binary.LittleEndian.PutUint32(block[index*4:], value)
		}
		copy(block[32:], image[number*UF2Payload:(number+1)*UF2Payload])
		for index := 32 + UF2Payload; index < UF2BlockSize-4; index++ {
			block[index] = 0
		}
		binary.LittleEndian.PutUint32(block[UF2BlockSize-4:], uf2MagicEnd)
		out = append(out, block...)
	}
	return out, nil
}

// UF2Blocks is the block count of a packed image; the build logs it and the
// tests assert on it.
func UF2Blocks(uf2 []byte) int { return len(uf2) / UF2BlockSize }

func u16(data []byte, at int) (uint16, error) {
	if at+2 > len(data) {
		return 0, errorf(Invalid, "ELF 太短")
	}
	return binary.LittleEndian.Uint16(data[at:]), nil
}

func u32(data []byte, at int) (uint32, error) {
	if at+4 > len(data) {
		return 0, errorf(Invalid, "ELF 太短")
	}
	return binary.LittleEndian.Uint32(data[at:]), nil
}
