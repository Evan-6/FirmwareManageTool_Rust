//! RP2040 ELF → UF2 packaging.
//!
//! Two details matter and both were learned the hard way on hardware:
//! every block must be 256-byte aligned (the BOOTSEL bootloader silently ignores
//! a file that is not), and the flash segments have to be flattened into one
//! image because a segment's file order is not its address order — `.boot2`
//! regularly comes last while it has to be at `0x10000000`.

use crate::error::{Error, Result};

/// Family id RP2040 writes into every block.
pub const FAMILY_RP2040: u32 = 0xE48B_FF56;
pub const XIP_BASE: u32 = 0x1000_0000;
pub const XIP_LIMIT: u32 = 0x1100_0000;
const BLOCK: usize = 512;
const PAYLOAD: usize = 256;
const MAGIC_START: [u32; 2] = [0x0A32_4655, 0x9E5D_5157];
const MAGIC_END: u32 = 0x0AB1_6F30;
const FLAG_FAMILY: u32 = 0x0000_2000;

/// Packs the flash image of an RP2040 ELF into a UF2 file.
pub fn pack(elf: &[u8]) -> Result<Vec<u8>> {
    if elf.len() <= 52 || elf[..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(Error::invalid("不是 ELF 檔"));
    }
    if elf[4] != 1 || elf[5] != 1 {
        return Err(Error::invalid("只支援 32-bit little-endian ELF"));
    }
    let phoff = read_u32(elf, 28)? as usize;
    let phentsize = read_u16(elf, 42)? as usize;
    let phnum = read_u16(elf, 44)? as usize;

    let mut segments = Vec::new();
    for index in 0..phnum {
        let base = phoff + index * phentsize;
        if base + 32 > elf.len() {
            return Err(Error::invalid("program header 超出檔案"));
        }
        let kind = read_u32(elf, base)?;
        let offset = read_u32(elf, base + 4)? as usize;
        let paddr = read_u32(elf, base + 12)?;
        let filesz = read_u32(elf, base + 16)? as usize;
        if kind != 1 || filesz == 0 || !(XIP_BASE..XIP_LIMIT).contains(&paddr) {
            continue;
        }
        let end = offset
            .checked_add(filesz)
            .filter(|end| *end <= elf.len())
            .ok_or_else(|| Error::invalid("segment 超出檔案"))?;
        segments.push((paddr, &elf[offset..end]));
    }
    if segments.is_empty() {
        return Err(Error::invalid(
            "ELF 沒有任何 flash segment；這不是 RP2040 建置？",
        ));
    }
    segments.sort_by_key(|segment| segment.0);

    // Flatten to one 256-byte aligned image; gaps stay erased (0xFF).
    let start = segments[0].0 & !(PAYLOAD as u32 - 1);
    let end = segments
        .iter()
        .map(|(address, data)| address + data.len() as u32)
        .max()
        .unwrap_or(start);
    let end = (end + PAYLOAD as u32 - 1) & !(PAYLOAD as u32 - 1);
    let mut image = vec![0xffu8; (end - start) as usize];
    for (address, data) in &segments {
        let at = (address - start) as usize;
        image[at..at + data.len()].copy_from_slice(data);
    }

    let total = image.len() / PAYLOAD;
    let mut out = Vec::with_capacity(total * BLOCK);
    for (number, payload) in image.chunks(PAYLOAD).enumerate() {
        let mut block = Vec::with_capacity(BLOCK);
        for value in [
            MAGIC_START[0],
            MAGIC_START[1],
            FLAG_FAMILY,
            start + (number * PAYLOAD) as u32,
            PAYLOAD as u32,
            number as u32,
            total as u32,
            FAMILY_RP2040,
        ] {
            block.extend_from_slice(&value.to_le_bytes());
        }
        block.extend_from_slice(payload);
        block.resize(BLOCK - 4, 0);
        block.extend_from_slice(&MAGIC_END.to_le_bytes());
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// Block count of a packed image; the caller logs it, the tests assert on it.
pub fn blocks(uf2: &[u8]) -> usize {
    uf2.len() / BLOCK
}

fn read_u16(data: &[u8], at: usize) -> Result<u16> {
    let slice = data
        .get(at..at + 2)
        .ok_or_else(|| Error::invalid("ELF 太短"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    let slice = data
        .get(at..at + 4)
        .ok_or_else(|| Error::invalid("ELF 太短"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal ELF32 with the given (paddr, payload) loadable segments.
    fn elf(segments: &[(u32, &[u8])]) -> Vec<u8> {
        let phoff = 52usize;
        let data_at = phoff + segments.len() * 32;
        let mut out = vec![0u8; data_at];
        out[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        out[4] = 1;
        out[5] = 1;
        out[28..32].copy_from_slice(&(phoff as u32).to_le_bytes());
        out[42..44].copy_from_slice(&32u16.to_le_bytes());
        out[44..46].copy_from_slice(&(segments.len() as u16).to_le_bytes());
        let mut cursor = data_at;
        for (index, (paddr, payload)) in segments.iter().enumerate() {
            let base = phoff + index * 32;
            out[base..base + 4].copy_from_slice(&1u32.to_le_bytes());
            out[base + 4..base + 8].copy_from_slice(&(cursor as u32).to_le_bytes());
            out[base + 12..base + 16].copy_from_slice(&paddr.to_le_bytes());
            out[base + 16..base + 20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
            cursor += payload.len();
        }
        out
    }

    fn block_field(uf2: &[u8], block: usize, index: usize) -> u32 {
        let at = block * BLOCK + index * 4;
        u32::from_le_bytes(uf2[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn blocks_are_aligned_and_ordered_by_address() {
        // boot2 last in file order but lowest address, plus an unaligned start.
        let image = elf(&[(0x1000_0100, &[1, 2, 3, 4]), (0x1000_0000, &[0xaa; 256])]);
        let uf2 = pack(&image).unwrap();
        assert_eq!(blocks(&uf2), 2);
        assert_eq!(block_field(&uf2, 0, 3), 0x1000_0000);
        assert_eq!(block_field(&uf2, 1, 3), 0x1000_0100);
        for index in 0..2 {
            assert_eq!(block_field(&uf2, index, 1), MAGIC_START[1]);
            assert_eq!(block_field(&uf2, index, 2), FLAG_FAMILY);
            assert_eq!(block_field(&uf2, index, 4), 256);
            assert_eq!(block_field(&uf2, index, 5), index as u32);
            assert_eq!(block_field(&uf2, index, 6), 2);
            assert_eq!(block_field(&uf2, index, 7), FAMILY_RP2040);
            let end = index * BLOCK + BLOCK - 4;
            assert_eq!(
                u32::from_le_bytes(uf2[end..end + 4].try_into().unwrap()),
                MAGIC_END
            );
        }
        // Block 0 carries boot2, block 1 the unaligned segment plus erased gap.
        assert_eq!(&uf2[32..288], &[0xaa; 256]);
        assert_eq!(&uf2[BLOCK + 32..BLOCK + 36], &[1, 2, 3, 4]);
        assert_eq!(&uf2[BLOCK + 36..BLOCK + 288], &[0xff; 252]);
    }

    #[test]
    fn rejects_an_image_without_flash_segments() {
        let image = elf(&[(0x2000_0000, &[1, 2, 3, 4])]);
        assert!(pack(&image).is_err());
        assert!(pack(b"not an elf").is_err());
    }
}
