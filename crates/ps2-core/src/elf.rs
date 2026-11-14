//! Minimal ELF32 little-endian MIPS (`EM_MIPS`) loader for EE executables.
//!
//! Primary source references:
//! - System V Application Binary Interface (ELF32 specification):
//!   * `e_ident`: `\x7FELF`, `EI_CLASS = 1` (`ELFCLASS32`), `EI_DATA = 1` (`ELFDATA2LSB`).
//!   * `e_type = 2` (`ET_EXEC`), `e_machine = 8` (`EM_MIPS`).
//!   * Program header `PT_LOAD = 1`: loads `p_filesz` bytes from file offset `p_offset`
//!     into guest address `p_vaddr`, and zero-fills `[p_vaddr + p_filesz .. p_vaddr + p_memsz)`.
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#memorymap`):
//!   * EE user/kernel executables load into 32 MB Main RDRAM (`0x0000_0000..=0x01FF_FFFF`,
//!     or KSEG0/KSEG1 mirrors `0x8000_0000..=0x81FF_FFFF` / `0xA000_0000..=0xA1FF_FFFF`).

use std::fmt;

use crate::bus::Bus;
use crate::ee::EeCpuState;
use crate::mmu::vtlb::VtlbTarget;

/// ELF32 file header size in bytes.
const ELF32_EHDR_SIZE: usize = 52;
/// Standard ELF32 program header entry size in bytes.
const ELF32_PHDR_MIN_SIZE: usize = 32;
/// ELF magic bytes `\x7FELF`.
const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
/// `ELFCLASS32` (`1`).
const ELFCLASS32: u8 = 1;
/// `ELFDATA2LSB` (`1`, little-endian).
const ELFDATA2LSB: u8 = 1;
/// `ET_EXEC` (`2`, executable file).
const ET_EXEC: u16 = 2;
/// `EM_MIPS` (`8`, MIPS architecture).
const EM_MIPS: u16 = 8;
/// `PT_LOAD` (`1`, loadable program segment).
const PT_LOAD: u32 = 1;

/// Metadata returned after successfully loading an ELF32 image into [`Bus`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadedElf {
    /// Entry point virtual address (`e_entry`).
    pub entry_pc: u32,
    /// Number of `PT_LOAD` segments loaded into memory.
    pub loaded_segments: usize,
    /// Highest virtual address (`p_vaddr + p_memsz`) across all loaded `PT_LOAD` segments.
    pub max_vaddr: u32,
}

/// Errors returned when rejecting a malformed or unsupported ELF32 binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfError {
    /// Input buffer is smaller than the 52-byte ELF32 header.
    TooSmall(usize),
    /// Missing `\x7FELF` magic header.
    BadMagic([u8; 4]),
    /// Not a 32-bit ELF (`EI_CLASS != ELFCLASS32`).
    UnsupportedClass(u8),
    /// Not a little-endian ELF (`EI_DATA != ELFDATA2LSB`).
    UnsupportedEndianness(u8),
    /// Not an executable ELF (`e_type != ET_EXEC`).
    UnsupportedType(u16),
    /// Not a MIPS ELF (`e_machine != EM_MIPS`).
    UnsupportedMachine(u16),
    /// Program header table offset/size overflows the input buffer.
    InvalidProgramHeaderTable {
        phoff: u32,
        phentsize: u16,
        phnum: u16,
        file_len: usize,
    },
    /// A `PT_LOAD` segment has `p_filesz > p_memsz` or overflows the file/address space.
    InvalidSegmentBounds {
        index: usize,
        offset: u32,
        vaddr: u32,
        filesz: u32,
        memsz: u32,
        file_len: usize,
    },
    /// A `PT_LOAD` segment targets an unmapped or non-RAM guest address.
    UnmappedLoadAddress { index: usize, vaddr: u32 },
    /// No loadable (`PT_LOAD`) segments were present in the ELF.
    NoLoadableSegments,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall(len) => {
                write!(
                    f,
                    "ELF buffer too small ({len} bytes, need >= {ELF32_EHDR_SIZE})"
                )
            }
            Self::BadMagic(magic) => write!(f, "invalid ELF magic: {magic:02X?}"),
            Self::UnsupportedClass(class) => {
                write!(f, "unsupported ELF class {class} (expected ELFCLASS32 = 1)")
            }
            Self::UnsupportedEndianness(data) => {
                write!(
                    f,
                    "unsupported ELF endianness {data} (expected ELFDATA2LSB = 1)"
                )
            }
            Self::UnsupportedType(kind) => {
                write!(f, "unsupported ELF e_type 0x{kind:04X} (expected ET_EXEC)")
            }
            Self::UnsupportedMachine(mach) => {
                write!(f, "unsupported ELF e_machine {mach} (expected EM_MIPS = 8)")
            }
            Self::InvalidProgramHeaderTable {
                phoff,
                phentsize,
                phnum,
                file_len,
            } => write!(
                f,
                "invalid program header table (phoff=0x{phoff:X}, phentsize={phentsize}, phnum={phnum}, file_len={file_len})"
            ),
            Self::InvalidSegmentBounds {
                index,
                offset,
                vaddr,
                filesz,
                memsz,
                file_len,
            } => write!(
                f,
                "invalid PT_LOAD[{index}] bounds (offset=0x{offset:X}, vaddr=0x{vaddr:08X}, filesz=0x{filesz:X}, memsz=0x{memsz:X}, file_len={file_len})"
            ),
            Self::UnmappedLoadAddress { index, vaddr } => write!(
                f,
                "PT_LOAD[{index}] targets non-RAM virtual address 0x{vaddr:08X}"
            ),
            Self::NoLoadableSegments => write!(f, "ELF contains no non-empty PT_LOAD segments"),
        }
    }
}

impl std::error::Error for ElfError {}

#[inline]
fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

#[inline]
fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parse an ELF32 MIPS executable, load all `PT_LOAD` segments into [`Bus`] RDRAM, and set
/// `cpu.pc = e_entry` (`cpu.next_pc = e_entry + 4`, `cpu.in_delay_slot = false`).
pub fn load_elf32(
    bytes: &[u8],
    cpu: &mut EeCpuState,
    bus: &mut Bus,
) -> Result<LoadedElf, ElfError> {
    if bytes.len() < ELF32_EHDR_SIZE {
        return Err(ElfError::TooSmall(bytes.len()));
    }

    let magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if magic != ELF_MAGIC {
        return Err(ElfError::BadMagic(magic));
    }

    let ei_class = bytes[4];
    if ei_class != ELFCLASS32 {
        return Err(ElfError::UnsupportedClass(ei_class));
    }

    let ei_data = bytes[5];
    if ei_data != ELFDATA2LSB {
        return Err(ElfError::UnsupportedEndianness(ei_data));
    }

    let e_type = read_u16_le(bytes, 16).ok_or(ElfError::TooSmall(bytes.len()))?;
    if e_type != ET_EXEC {
        return Err(ElfError::UnsupportedType(e_type));
    }

    let e_machine = read_u16_le(bytes, 18).ok_or(ElfError::TooSmall(bytes.len()))?;
    if e_machine != EM_MIPS {
        return Err(ElfError::UnsupportedMachine(e_machine));
    }

    let e_entry = read_u32_le(bytes, 24).ok_or(ElfError::TooSmall(bytes.len()))?;
    let e_phoff = read_u32_le(bytes, 28).ok_or(ElfError::TooSmall(bytes.len()))?;
    let e_phentsize = read_u16_le(bytes, 42).ok_or(ElfError::TooSmall(bytes.len()))?;
    let e_phnum = read_u16_le(bytes, 44).ok_or(ElfError::TooSmall(bytes.len()))?;

    if e_phnum == 0 || (e_phentsize as usize) < ELF32_PHDR_MIN_SIZE {
        return Err(ElfError::InvalidProgramHeaderTable {
            phoff: e_phoff,
            phentsize: e_phentsize,
            phnum: e_phnum,
            file_len: bytes.len(),
        });
    }

    let ph_table_bytes = (e_phentsize as usize).checked_mul(e_phnum as usize).ok_or(
        ElfError::InvalidProgramHeaderTable {
            phoff: e_phoff,
            phentsize: e_phentsize,
            phnum: e_phnum,
            file_len: bytes.len(),
        },
    )?;
    let ph_table_end = (e_phoff as usize).checked_add(ph_table_bytes).ok_or(
        ElfError::InvalidProgramHeaderTable {
            phoff: e_phoff,
            phentsize: e_phentsize,
            phnum: e_phnum,
            file_len: bytes.len(),
        },
    )?;

    if ph_table_end > bytes.len() {
        return Err(ElfError::InvalidProgramHeaderTable {
            phoff: e_phoff,
            phentsize: e_phentsize,
            phnum: e_phnum,
            file_len: bytes.len(),
        });
    }

    // First pass: validate all PT_LOAD segments before mutating guest RAM.
    let mut load_segments = Vec::new();
    for idx in 0..(e_phnum as usize) {
        let ph_base = (e_phoff as usize) + idx * (e_phentsize as usize);
        let p_type = read_u32_le(bytes, ph_base).ok_or(ElfError::TooSmall(bytes.len()))?;
        if p_type != PT_LOAD {
            continue;
        }

        let p_offset = read_u32_le(bytes, ph_base + 4).ok_or(ElfError::TooSmall(bytes.len()))?;
        let p_vaddr = read_u32_le(bytes, ph_base + 8).ok_or(ElfError::TooSmall(bytes.len()))?;
        let p_filesz = read_u32_le(bytes, ph_base + 16).ok_or(ElfError::TooSmall(bytes.len()))?;
        let p_memsz = read_u32_le(bytes, ph_base + 20).ok_or(ElfError::TooSmall(bytes.len()))?;

        if p_memsz == 0 {
            continue;
        }

        if p_filesz > p_memsz {
            return Err(ElfError::InvalidSegmentBounds {
                index: idx,
                offset: p_offset,
                vaddr: p_vaddr,
                filesz: p_filesz,
                memsz: p_memsz,
                file_len: bytes.len(),
            });
        }

        let file_end = (p_offset as usize).checked_add(p_filesz as usize).ok_or(
            ElfError::InvalidSegmentBounds {
                index: idx,
                offset: p_offset,
                vaddr: p_vaddr,
                filesz: p_filesz,
                memsz: p_memsz,
                file_len: bytes.len(),
            },
        )?;
        if file_end > bytes.len() {
            return Err(ElfError::InvalidSegmentBounds {
                index: idx,
                offset: p_offset,
                vaddr: p_vaddr,
                filesz: p_filesz,
                memsz: p_memsz,
                file_len: bytes.len(),
            });
        }

        let end_vaddr = p_vaddr
            .checked_add(p_memsz)
            .ok_or(ElfError::InvalidSegmentBounds {
                index: idx,
                offset: p_offset,
                vaddr: p_vaddr,
                filesz: p_filesz,
                memsz: p_memsz,
                file_len: bytes.len(),
            })?;

        // Verify start and end bytes of the segment translate to Main RDRAM or Scratchpad.
        let start_trans = bus.vtlb.translate(p_vaddr);
        let last_trans = bus.vtlb.translate(end_vaddr - 1);
        if !matches!(start_trans.target, VtlbTarget::Ram | VtlbTarget::Scratchpad) {
            return Err(ElfError::UnmappedLoadAddress {
                index: idx,
                vaddr: p_vaddr,
            });
        }
        if !matches!(last_trans.target, VtlbTarget::Ram | VtlbTarget::Scratchpad) {
            return Err(ElfError::UnmappedLoadAddress {
                index: idx,
                vaddr: end_vaddr - 1,
            });
        }

        load_segments.push((p_offset as usize, p_vaddr, p_filesz, p_memsz));
    }

    if load_segments.is_empty() {
        return Err(ElfError::NoLoadableSegments);
    }

    // Validate entry point maps to executable RAM or BIOS.
    let entry_trans = bus.vtlb.translate(e_entry);
    if !matches!(
        entry_trans.target,
        VtlbTarget::Ram | VtlbTarget::Scratchpad | VtlbTarget::Bios
    ) {
        return Err(ElfError::UnmappedLoadAddress {
            index: 0,
            vaddr: e_entry,
        });
    }

    // Second pass: copy PT_LOAD file contents and zero-fill .bss tail.
    let mut max_vaddr = 0u32;
    for &(file_off, vaddr, filesz, memsz) in &load_segments {
        let src_slice = &bytes[file_off..file_off + (filesz as usize)];
        for (i, &byte) in src_slice.iter().enumerate() {
            bus.write8(vaddr.wrapping_add(i as u32), byte);
        }
        for i in filesz..memsz {
            bus.write8(vaddr.wrapping_add(i), 0);
        }
        let seg_end = vaddr.wrapping_add(memsz);
        if seg_end > max_vaddr {
            max_vaddr = seg_end;
        }
    }

    cpu.pc = e_entry;
    cpu.next_pc = e_entry.wrapping_add(4);
    cpu.in_delay_slot = false;

    Ok(LoadedElf {
        entry_pc: e_entry,
        loaded_segments: load_segments.len(),
        max_vaddr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_minimal_mips_elf(entry: u32, vaddr: u32, payload: &[u8], memsz: u32) -> Vec<u8> {
        let mut buf = vec![0u8; 52 + 32 + payload.len()];
        // e_ident
        buf[0..4].copy_from_slice(&ELF_MAGIC);
        buf[4] = ELFCLASS32;
        buf[5] = ELFDATA2LSB;
        buf[6] = 1; // EV_CURRENT
                    // e_type = ET_EXEC (2), e_machine = EM_MIPS (8), e_version = 1
        buf[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());
        buf[18..20].copy_from_slice(&EM_MIPS.to_le_bytes());
        buf[20..24].copy_from_slice(&1u32.to_le_bytes());
        // e_entry
        buf[24..28].copy_from_slice(&entry.to_le_bytes());
        // e_phoff = 52
        buf[28..32].copy_from_slice(&52u32.to_le_bytes());
        // e_ehsize = 52, e_phentsize = 32, e_phnum = 1
        buf[40..42].copy_from_slice(&52u16.to_le_bytes());
        buf[42..44].copy_from_slice(&32u16.to_le_bytes());
        buf[44..46].copy_from_slice(&1u16.to_le_bytes());

        // Program header 0 at offset 52
        let p_offset = 84u32;
        let p_filesz = payload.len() as u32;
        buf[52..56].copy_from_slice(&PT_LOAD.to_le_bytes());
        buf[56..60].copy_from_slice(&p_offset.to_le_bytes());
        buf[60..64].copy_from_slice(&vaddr.to_le_bytes());
        buf[64..68].copy_from_slice(&vaddr.to_le_bytes());
        buf[68..72].copy_from_slice(&p_filesz.to_le_bytes());
        buf[72..76].copy_from_slice(&memsz.to_le_bytes());
        buf[76..80].copy_from_slice(&0x7u32.to_le_bytes()); // PF_R | PF_W | PF_X

        buf[84..84 + payload.len()].copy_from_slice(payload);
        buf
    }

    #[test]
    fn elf_loader_loads_pt_load_and_zeroes_bss() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();
        // Pre-dirty RAM at 0x0010_0000..0x0010_0010 to verify .bss zeroing.
        for off in 0..16u32 {
            bus.write8(0x0010_0000 + off, 0xAA);
        }

        let code = [0x3C, 0x02, 0x12, 0x34, 0x00, 0x00, 0x00, 0x0C]; // lui $v0, 0x1234; syscall
        let elf = build_minimal_mips_elf(0x0010_0000, 0x0010_0000, &code, 16);
        let loaded = load_elf32(&elf, &mut cpu, &mut bus).expect("valid ELF should load");

        assert_eq!(loaded.entry_pc, 0x0010_0000);
        assert_eq!(cpu.pc, 0x0010_0000);
        assert_eq!(cpu.next_pc, 0x0010_0004);
        assert_eq!(loaded.loaded_segments, 1);
        assert_eq!(loaded.max_vaddr, 0x0010_0010);
        assert_eq!(bus.read32(0x0010_0000), 0x3412_023C);
        assert_eq!(bus.read32(0x0010_0004), 0x0C00_0000);
        // .bss bytes [8..16) must be zeroed.
        assert_eq!(bus.read32(0x0010_0008), 0);
        assert_eq!(bus.read32(0x0010_000C), 0);
    }

    #[test]
    fn elf_loader_rejects_malformed_inputs_without_panicking() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();

        // 1. Too small
        assert_eq!(
            load_elf32(&[0x7F, b'E', b'L'], &mut cpu, &mut bus),
            Err(ElfError::TooSmall(3))
        );

        // 2. Bad magic
        let mut bad_magic = build_minimal_mips_elf(0x0010_0000, 0x0010_0000, &[1, 2, 3, 4], 4);
        bad_magic[0] = b'X';
        assert!(matches!(
            load_elf32(&bad_magic, &mut cpu, &mut bus),
            Err(ElfError::BadMagic(_))
        ));

        // 3. Wrong ELF class (64-bit)
        let mut elf64 = build_minimal_mips_elf(0x0010_0000, 0x0010_0000, &[1, 2, 3, 4], 4);
        elf64[4] = 2;
        assert_eq!(
            load_elf32(&elf64, &mut cpu, &mut bus),
            Err(ElfError::UnsupportedClass(2))
        );

        // 4. Wrong machine (x86 = 3)
        let mut wrong_arch = build_minimal_mips_elf(0x0010_0000, 0x0010_0000, &[1, 2, 3, 4], 4);
        wrong_arch[18..20].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(
            load_elf32(&wrong_arch, &mut cpu, &mut bus),
            Err(ElfError::UnsupportedMachine(3))
        );

        // 5. filesz > memsz
        let bad_bounds =
            build_minimal_mips_elf(0x0010_0000, 0x0010_0000, &[1, 2, 3, 4, 5, 6, 7, 8], 4);
        assert!(matches!(
            load_elf32(&bad_bounds, &mut cpu, &mut bus),
            Err(ElfError::InvalidSegmentBounds { .. })
        ));

        // 6. Unmapped virtual address (0x0400_0000 is beyond 32 MB RDRAM)
        let unmapped = build_minimal_mips_elf(0x0400_0000, 0x0400_0000, &[1, 2, 3, 4], 4);
        assert!(matches!(
            load_elf32(&unmapped, &mut cpu, &mut bus),
            Err(ElfError::UnmappedLoadAddress { .. })
        ));
    }
}
