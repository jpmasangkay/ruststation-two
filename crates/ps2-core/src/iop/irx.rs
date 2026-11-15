//! Minimal IOP Relocatable Executable (`.irx`) loader and MIPS relocation engine (`Task 6a`).
//!
//! Primary source references:
//! - `ps2autotests` (`tests/cpu/iop/*.irx` & `common/common-iop.mk`):
//!   * IOP `.irx` binaries are 32-bit little-endian MIPS ELF files with `e_type = 0xFF80`
//!     (`ET_SCE_IOPRELEXEC`, `ET_LOPROC | 0x80`) and `e_machine = 8` (`EM_MIPS`).
//!   * Program Header `0` has `p_type = 0x7000_0080` (`PT_SCE_IOPMOD`), containing the 27-byte
//!     `.iopmod` structure (`module_info_ptr`, `entry`, `gp`, `text_size`, `data_size`, `bss_size`).
//!   * Program Header `1` has `p_type = 1` (`PT_LOAD`) linked at zero-based virtual address `0x0000_0000`.
//!   * Section headers of type `SHT_REL` (`9`) hold 8-byte `(r_offset, r_info)` MIPS relocations
//!     (`R_MIPS_32 = 2`, `R_MIPS_26 = 4`, `R_MIPS_HI16 = 5`, `R_MIPS_LO16 = 6`) whose in-place
//!     addends are already pre-linked for base `0x0000_0000`.

use crate::bus::{Bus, IOP_RAM_SIZE};
use crate::iop::state::{IopCpuState, IOP_HLE_EXIT_RA};

/// Default IOP RAM load base (`0x0001_0000`, 64 KB into 2 MB IOP RAM).
///
/// Placed above the MIPS exception vector area (`0x0000_0000..0x0000_0100`) and satisfies
/// `branchdelay.c` line 150 (`if (result > 0x100 && result <= 0x7ffffffc)`).
pub const IOP_IRX_DEFAULT_BASE: u32 = 0x0001_0000;

/// Default initial IOP stack pointer (`$sp = 0x001F_FFF0`, top of 2 MB IOP RAM).
pub const IOP_DEFAULT_SP: u32 = (IOP_RAM_SIZE as u32) - 16;

/// ELF `e_type` for Sony IOP Relocatable Executables (`ET_LOPROC | 0x80 = 0xFF80`).
pub const ET_SCE_IOPRELEXEC: u16 = 0xFF80;
/// ELF `e_type` for Sony IOP Relocatable Executables v2 (`0xFF81`).
pub const ET_SCE_IOPRELEXEC2: u16 = 0xFF81;
/// ELF `p_type` for the `.iopmod` program header (`0x7000_0080`).
pub const PT_SCE_IOPMOD: u32 = 0x7000_0080;
/// ELF `p_type` for loadable segments (`1`).
const PT_LOAD: u32 = 1;
/// ELF `sh_type` for `SHT_REL` relocation sections (`9`).
const SHT_REL: u32 = 9;

/// MIPS relocation types (`r_info & 0xFF`).
const R_MIPS_NONE: u8 = 0;
const R_MIPS_16: u8 = 1;
const R_MIPS_32: u8 = 2;
const R_MIPS_26: u8 = 4;
const R_MIPS_HI16: u8 = 5;
const R_MIPS_LO16: u8 = 6;

/// Metadata describing an `.irx` module loaded into IOP RAM.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LoadedIrx {
    /// Relocated IOP RAM base address (`0x0001_0000`).
    pub base_addr: u32,
    /// Relocated entry point virtual address (`base_addr + e_entry`).
    pub entry_pc: u32,
    /// Relocated `$gp` register value (`base_addr + iopmod.gp`).
    pub gp: u32,
    /// Total number of `SHT_REL` relocation entries applied.
    pub relocations_applied: usize,
}

/// Errors returned when validating or relocating an IOP `.irx` image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrxError {
    /// File is smaller than the 52-byte 32-bit ELF header.
    BufferTooSmall(usize),
    /// Missing `\x7FELF` magic.
    InvalidMagic,
    /// Not a 32-bit little-endian MIPS ELF (`EI_CLASS = 1`, `EI_DATA = 1`, `e_machine = 8`).
    UnsupportedTarget {
        ei_class: u8,
        ei_data: u8,
        e_machine: u16,
    },
    /// ELF `e_type` is not an IOP `.irx` (`0xFF80`/`0xFF81`) or executable (`2`).
    UnsupportedElfType(u16),
    /// Program header or section header table extends past EOF.
    HeaderTableOutOfBounds,
    /// Segment file range extends past EOF.
    SegmentOutOfFileBounds { offset: u32, filesz: u32 },
    /// Relocated segment exceeds the 2 MB IOP RAM range.
    SegmentOutOfIopRamBounds { vaddr: u32, memsz: u32 },
    /// No `PT_LOAD` segment found in the `.irx`.
    NoLoadableSegments,
    /// Encountered an unsupported MIPS relocation type.
    UnsupportedRelocationType { r_type: u8, r_offset: u32 },
}

impl std::fmt::Display for IrxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BufferTooSmall(sz) => write!(f, "IRX buffer too small ({sz} bytes)"),
            Self::InvalidMagic => write!(f, "invalid ELF magic"),
            Self::UnsupportedTarget {
                ei_class,
                ei_data,
                e_machine,
            } => write!(
                f,
                "unsupported ELF target (class={ei_class}, data={ei_data}, machine={e_machine})"
            ),
            Self::UnsupportedElfType(t) => write!(f, "unsupported ELF e_type 0x{t:04X} for IRX"),
            Self::HeaderTableOutOfBounds => {
                write!(f, "ELF program or section header table out of bounds")
            }
            Self::SegmentOutOfFileBounds { offset, filesz } => write!(
                f,
                "PT_LOAD segment (offset=0x{offset:X}, filesz=0x{filesz:X}) out of file bounds"
            ),
            Self::SegmentOutOfIopRamBounds { vaddr, memsz } => write!(
                f,
                "PT_LOAD segment (vaddr=0x{vaddr:08X}, memsz=0x{memsz:X}) exceeds 2 MB IOP RAM"
            ),
            Self::NoLoadableSegments => write!(f, "IRX contains no PT_LOAD segments"),
            Self::UnsupportedRelocationType { r_type, r_offset } => write!(
                f,
                "unsupported MIPS relocation type {r_type} at offset 0x{r_offset:06X}"
            ),
        }
    }
}

impl std::error::Error for IrxError {}

/// Returns `true` if `bytes` has a 32-bit little-endian ELF header with `e_type == ET_SCE_IOPRELEXEC` (`0xFF80` or `0xFF81`).
pub fn is_irx_binary(bytes: &[u8]) -> bool {
    if bytes.len() < 52 || &bytes[0..4] != b"\x7FELF" {
        return false;
    }
    let e_type = u16::from_le_bytes([bytes[16], bytes[17]]);
    e_type == ET_SCE_IOPRELEXEC || e_type == ET_SCE_IOPRELEXEC2
}

/// Load and relocate an IOP `.irx` image into `bus.iop_ram` at [`IOP_IRX_DEFAULT_BASE`] (`0x0001_0000`)
/// and initialize `iop` to enter `_start(argc = 0, argv = NULL)`.
pub fn load_irx(bytes: &[u8], iop: &mut IopCpuState, bus: &mut Bus) -> Result<LoadedIrx, IrxError> {
    load_irx_at(bytes, IOP_IRX_DEFAULT_BASE, iop, bus)
}

/// Load and relocate an IOP `.irx` image into `bus.iop_ram` at `base_addr`.
pub fn load_irx_at(
    bytes: &[u8],
    base_addr: u32,
    iop: &mut IopCpuState,
    bus: &mut Bus,
) -> Result<LoadedIrx, IrxError> {
    if bytes.len() < 52 {
        return Err(IrxError::BufferTooSmall(bytes.len()));
    }
    if &bytes[0..4] != b"\x7FELF" {
        return Err(IrxError::InvalidMagic);
    }

    let ei_class = bytes[4];
    let ei_data = bytes[5];
    let e_type = read_u16(bytes, 16);
    let e_machine = read_u16(bytes, 18);
    if ei_class != 1 || ei_data != 1 || e_machine != 8 {
        return Err(IrxError::UnsupportedTarget {
            ei_class,
            ei_data,
            e_machine,
        });
    }
    if e_type != ET_SCE_IOPRELEXEC && e_type != ET_SCE_IOPRELEXEC2 && e_type != 2 {
        return Err(IrxError::UnsupportedElfType(e_type));
    }

    let e_entry = read_u32(bytes, 24);
    let e_phoff = read_u32(bytes, 28) as usize;
    let e_shoff = read_u32(bytes, 32) as usize;
    let e_phentsize = read_u16(bytes, 42) as usize;
    let e_phnum = read_u16(bytes, 44) as usize;
    let e_shentsize = read_u16(bytes, 46) as usize;
    let e_shnum = read_u16(bytes, 48) as usize;

    let ph_end = e_phoff
        .checked_add(
            e_phentsize
                .checked_mul(e_phnum)
                .ok_or(IrxError::HeaderTableOutOfBounds)?,
        )
        .ok_or(IrxError::HeaderTableOutOfBounds)?;
    if ph_end > bytes.len() {
        return Err(IrxError::HeaderTableOutOfBounds);
    }

    let mut gp_offset = 0u32;
    let mut loaded_segments = 0usize;

    for i in 0..e_phnum {
        let ph = e_phoff + i * e_phentsize;
        let p_type = read_u32(bytes, ph);
        let p_offset = read_u32(bytes, ph + 4);
        let p_vaddr = read_u32(bytes, ph + 8);
        let p_filesz = read_u32(bytes, ph + 16);
        let p_memsz = read_u32(bytes, ph + 20);

        if p_type == PT_SCE_IOPMOD && (p_offset as usize).saturating_add(12) <= bytes.len() {
            gp_offset = read_u32(bytes, (p_offset as usize) + 8);
            continue;
        }
        if p_type != PT_LOAD {
            continue;
        }

        let file_end = (p_offset as usize).checked_add(p_filesz as usize).ok_or(
            IrxError::SegmentOutOfFileBounds {
                offset: p_offset,
                filesz: p_filesz,
            },
        )?;
        if file_end > bytes.len() {
            return Err(IrxError::SegmentOutOfFileBounds {
                offset: p_offset,
                filesz: p_filesz,
            });
        }

        let dest_start = base_addr.wrapping_add(p_vaddr) as usize;
        let dest_end =
            dest_start
                .checked_add(p_memsz as usize)
                .ok_or(IrxError::SegmentOutOfIopRamBounds {
                    vaddr: base_addr.wrapping_add(p_vaddr),
                    memsz: p_memsz,
                })?;
        if dest_end > IOP_RAM_SIZE || p_filesz > p_memsz {
            return Err(IrxError::SegmentOutOfIopRamBounds {
                vaddr: base_addr.wrapping_add(p_vaddr),
                memsz: p_memsz,
            });
        }

        bus.iop_ram[dest_start..dest_end].fill(0);
        bus.iop_ram[dest_start..dest_start + (p_filesz as usize)]
            .copy_from_slice(&bytes[p_offset as usize..file_end]);
        loaded_segments += 1;
    }

    if loaded_segments == 0 {
        return Err(IrxError::NoLoadableSegments);
    }

    // Apply all SHT_REL relocation sections if present.
    let mut relocations_applied = 0usize;
    if e_shoff != 0 && e_shnum != 0 {
        let sh_end = e_shoff
            .checked_add(
                e_shentsize
                    .checked_mul(e_shnum)
                    .ok_or(IrxError::HeaderTableOutOfBounds)?,
            )
            .ok_or(IrxError::HeaderTableOutOfBounds)?;
        if sh_end > bytes.len() {
            return Err(IrxError::HeaderTableOutOfBounds);
        }

        for i in 0..e_shnum {
            let sh = e_shoff + i * e_shentsize;
            let sh_type = read_u32(bytes, sh + 4);
            if sh_type != SHT_REL {
                continue;
            }
            let sh_offset = read_u32(bytes, sh + 16) as usize;
            let sh_size = read_u32(bytes, sh + 20) as usize;
            let rel_end = sh_offset
                .checked_add(sh_size)
                .ok_or(IrxError::HeaderTableOutOfBounds)?;
            if rel_end > bytes.len() {
                return Err(IrxError::HeaderTableOutOfBounds);
            }

            let count = sh_size / 8;
            let mut pending_hi16_addrs: Vec<(u32, u16)> = Vec::new();
            let mut last_hi16_imm: u16 = 0;

            for r in 0..count {
                let entry_off = sh_offset + r * 8;
                let r_offset = read_u32(bytes, entry_off);
                let r_info = read_u32(bytes, entry_off + 4);
                let r_type = (r_info & 0xFF) as u8;
                let target_vaddr = base_addr.wrapping_add(r_offset);

                match r_type {
                    R_MIPS_NONE => {}
                    R_MIPS_16 => {
                        let word = bus.iop_read32(target_vaddr);
                        let imm16 = (word & 0xFFFF) as i16 as i32;
                        let new_imm = (imm16.wrapping_add(base_addr as i32) as u32) & 0xFFFF;
                        bus.iop_write32(target_vaddr, (word & 0xFFFF_0000) | new_imm);
                    }
                    R_MIPS_32 => {
                        let word = bus.iop_read32(target_vaddr);
                        bus.iop_write32(target_vaddr, word.wrapping_add(base_addr));
                    }
                    R_MIPS_26 => {
                        let word = bus.iop_read32(target_vaddr);
                        let orig_target = (word & 0x03FF_FFFF) << 2;
                        let new_target = orig_target.wrapping_add(base_addr);
                        let patched = (word & 0xFC00_0000) | ((new_target >> 2) & 0x03FF_FFFF);
                        bus.iop_write32(target_vaddr, patched);
                    }
                    R_MIPS_HI16 => {
                        let word = bus.iop_read32(target_vaddr);
                        let hi16 = (word & 0xFFFF) as u16;
                        last_hi16_imm = hi16;
                        pending_hi16_addrs.push((target_vaddr, hi16));
                    }
                    R_MIPS_LO16 => {
                        let lo_word = bus.iop_read32(target_vaddr);
                        let lo16 = (lo_word & 0xFFFF) as i16 as i32;
                        if pending_hi16_addrs.is_empty() {
                            let ahl = ((last_hi16_imm as u32) << 16).wrapping_add(lo16 as u32);
                            let relocated = ahl.wrapping_add(base_addr);
                            let new_lo = relocated & 0xFFFF;
                            bus.iop_write32(target_vaddr, (lo_word & 0xFFFF_0000) | new_lo);
                        } else {
                            let mut new_lo = 0u32;
                            for (hi_vaddr, hi16) in pending_hi16_addrs.drain(..) {
                                let ahl = ((hi16 as u32) << 16).wrapping_add(lo16 as u32);
                                let relocated = ahl.wrapping_add(base_addr);
                                let new_hi = (relocated.wrapping_add(0x8000) >> 16) & 0xFFFF;
                                new_lo = relocated & 0xFFFF;
                                let hi_word = bus.iop_read32(hi_vaddr);
                                bus.iop_write32(hi_vaddr, (hi_word & 0xFFFF_0000) | new_hi);
                            }
                            bus.iop_write32(target_vaddr, (lo_word & 0xFFFF_0000) | new_lo);
                        }
                    }
                    other => {
                        return Err(IrxError::UnsupportedRelocationType {
                            r_type: other,
                            r_offset,
                        });
                    }
                }
                relocations_applied += 1;
            }
        }
    }

    let entry_pc = base_addr.wrapping_add(e_entry);
    let gp = base_addr.wrapping_add(gp_offset);

    iop.reset();
    iop.set_pc(entry_pc);
    // _start(argc = 0, argv = NULL)
    iop.write_gpr(4, 0);
    iop.write_gpr(5, 0);
    iop.write_gpr(28, gp);
    iop.write_gpr(29, IOP_DEFAULT_SP);
    iop.write_gpr(31, IOP_HLE_EXIT_RA);

    Ok(LoadedIrx {
        base_addr,
        entry_pc,
        gp,
        relocations_applied,
    })
}

#[inline(always)]
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

#[inline(always)]
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}
