//! Minimal PS2 ELF32 generator (`ET_EXEC`, `EM_MIPS`) from `.gsp` GS scripts.
//!
//! Emits a standalone no-syscall, no-DMA MIPS R5900 executable that configures GS PCRTC
//! display registers (`PMODE`, `SMODE1`, `SMODE2`, `DISPFB1`, `DISPLAY1`, `BGCOLOR`) for a
//! progressive non-interlaced scanout and streams normalized GS packets to `GIF_FIFO` (`0x1000_6000`).

use ps2_gs::{GsState, GspCommand, GspSurfaceHint, PixelFormat};
use thiserror::Error;

/// Base load virtual/physical address (`KUSEG` Main RAM, 16-byte aligned).
pub const ELF_ENTRY_VADDR: u32 = 0x0010_0000;
/// Size of `Elf32_Ehdr` in bytes.
pub const ELF32_EHDR_SIZE: u32 = 52;
/// Size of `Elf32_Phdr` in bytes.
pub const ELF32_PHDR_SIZE: u32 = 32;
/// 16-byte aligned file offset where the single `PT_LOAD` segment begins.
pub const ELF_LOAD_FILE_OFFSET: u32 = 0x60;
/// Number of 32-bit instructions in the R5900 stub (32 instructions = 128 = `0x80` bytes).
pub const STUB_INSTR_COUNT: usize = 32;
/// Byte size of the R5900 stub (16-byte aligned so `PRIV_BASE` is 16-byte aligned).
pub const STUB_BYTE_LEN: u32 = (STUB_INSTR_COUNT * 4) as u32;
/// Virtual address of the `[u64 mmio_addr, u64 value]` GS privileged register table.
pub const PRIV_TABLE_VADDR: u32 = ELF_ENTRY_VADDR + STUB_BYTE_LEN;
/// GS privileged register MMIO base (`PMODE` at `+0x0000` .. `BGCOLOR` at `+0x00E0`).
pub const GS_PRIV_BASE_ADDR: u64 = 0x1200_0000;
/// EE `GIF_STAT` physical/KUSEG address (`FQC` in bits `24..=28`).
pub const GIF_STAT_ADDR: u32 = 0x1000_3020;
/// EE `GIF_FIFO` physical/KUSEG address (`PATH3` 16-QWC FIFO).
pub const GIF_FIFO_ADDR: u32 = 0x1000_6000;
/// R5900 self-spin opcode (`beq $zero, $zero, -1`).
pub const R5900_SPIN_BEQ: u32 = 0x1000_FFFF;

/// Privileged GS register offsets relative to `0x1200_0000`.
pub mod priv_off {
    pub const PMODE: u32 = 0x0000;
    pub const SMODE1: u32 = 0x0010;
    pub const SMODE2: u32 = 0x0020;
    pub const DISPFB1: u32 = 0x0070;
    pub const DISPLAY1: u32 = 0x0080;
    pub const BGCOLOR: u32 = 0x00E0;
}

/// Errors produced when converting a `.gsp` script to a standalone PS2 ELF32.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ElfEmitError {
    #[error(
        "privileged register offset 0x{0:X} is outside 16-byte aligned GS range 0x0000..=0x00E0"
    )]
    InvalidPrivOffset(u32),
    #[error("emitted ELF segment ({0} bytes) exceeds 30 MiB RDRAM safety limit")]
    PayloadTooLarge(usize),
    #[error("R5900 stub code size mismatch: expected {expected} words, got {actual}")]
    StubLayoutMismatch { expected: usize, actual: usize },
}

/// Decoded/encoded fields of `GS PMODE` (`0x1200_0000`, `FETCHED` `ps2sdk gs_privileged.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PmodeFields {
    pub en1: bool,
    pub en2: bool,
    pub crtmd: u8,
    pub mmod: bool,
    pub amod: bool,
    pub slbg: bool,
    pub alp: u8,
}

impl PmodeFields {
    pub const fn encode(self) -> u64 {
        (self.en1 as u64)
            | ((self.en2 as u64) << 1)
            | (((self.crtmd as u64) & 0x7) << 2)
            | ((self.mmod as u64) << 5)
            | ((self.amod as u64) << 6)
            | ((self.slbg as u64) << 7)
            | ((self.alp as u64) << 8)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            en1: (raw & 1) != 0,
            en2: ((raw >> 1) & 1) != 0,
            crtmd: ((raw >> 2) & 0x7) as u8,
            mmod: ((raw >> 5) & 1) != 0,
            amod: ((raw >> 6) & 1) != 0,
            slbg: ((raw >> 7) & 1) != 0,
            alp: ((raw >> 8) & 0xFF) as u8,
        }
    }
}

/// Decoded/encoded fields of `GS SMODE1` (`0x1200_0010`, `FETCHED` `ps2sdk ee/libgs/include/libgs.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Smode1Fields {
    pub rc: u8,
    pub lc: u8,
    pub t1248: u8,
    pub slck: bool,
    pub cmod: u8,
    pub ex: bool,
    pub prst: bool,
    pub sint: bool,
    pub xpck: bool,
    pub pck2: u8,
    pub spml: u8,
    pub gcont: bool,
    pub phs: bool,
    pub pvs: bool,
    pub pehs: bool,
    pub pevs: bool,
    pub clksel: u8,
    pub nvck: bool,
    pub slck2: bool,
    pub vcksel: u8,
    pub vhp: u8,
}

impl Smode1Fields {
    /// NTSC non-interlaced PLL/clock settings (`FETCHED` `ps2sdk ee/graph/src/graph_mode.c`).
    pub const fn ntsc_progressive_default() -> Self {
        Self {
            rc: 4,
            lc: 32,
            t1248: 1,
            slck: false,
            cmod: 2,
            ex: false,
            prst: false,
            sint: false,
            xpck: false,
            pck2: 0,
            spml: 4,
            gcont: false,
            phs: false,
            pvs: false,
            pehs: false,
            pevs: false,
            clksel: 1,
            nvck: true,
            slck2: true,
            vcksel: 1,
            vhp: 0,
        }
    }

    pub const fn encode(self) -> u64 {
        ((self.rc as u64) & 0x7)
            | (((self.lc as u64) & 0x7F) << 3)
            | (((self.t1248 as u64) & 0x3) << 10)
            | ((self.slck as u64) << 12)
            | (((self.cmod as u64) & 0x3) << 13)
            | ((self.ex as u64) << 15)
            | ((self.prst as u64) << 16)
            | ((self.sint as u64) << 17)
            | ((self.xpck as u64) << 18)
            | (((self.pck2 as u64) & 0x3) << 19)
            | (((self.spml as u64) & 0xF) << 21)
            | ((self.gcont as u64) << 25)
            | ((self.phs as u64) << 26)
            | ((self.pvs as u64) << 27)
            | ((self.pehs as u64) << 28)
            | ((self.pevs as u64) << 29)
            | (((self.clksel as u64) & 0x3) << 30)
            | ((self.nvck as u64) << 32)
            | ((self.slck2 as u64) << 33)
            | (((self.vcksel as u64) & 0x3) << 34)
            | (((self.vhp as u64) & 0x7) << 36)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            rc: (raw & 0x7) as u8,
            lc: ((raw >> 3) & 0x7F) as u8,
            t1248: ((raw >> 10) & 0x3) as u8,
            slck: ((raw >> 12) & 1) != 0,
            cmod: ((raw >> 13) & 0x3) as u8,
            ex: ((raw >> 15) & 1) != 0,
            prst: ((raw >> 16) & 1) != 0,
            sint: ((raw >> 17) & 1) != 0,
            xpck: ((raw >> 18) & 1) != 0,
            pck2: ((raw >> 19) & 0x3) as u8,
            spml: ((raw >> 21) & 0xF) as u8,
            gcont: ((raw >> 25) & 1) != 0,
            phs: ((raw >> 26) & 1) != 0,
            pvs: ((raw >> 27) & 1) != 0,
            pehs: ((raw >> 28) & 1) != 0,
            pevs: ((raw >> 29) & 1) != 0,
            clksel: ((raw >> 30) & 0x3) as u8,
            nvck: ((raw >> 32) & 1) != 0,
            slck2: ((raw >> 33) & 1) != 0,
            vcksel: ((raw >> 34) & 0x3) as u8,
            vhp: ((raw >> 36) & 0x7) as u8,
        }
    }
}

/// Decoded/encoded fields of `GS SMODE2` (`0x1200_0020`, `FETCHED` `Play! GSHandler.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Smode2Fields {
    pub interlace: bool,
    pub ffmd: bool,
    pub dpms: u8,
}

impl Smode2Fields {
    pub const fn encode(self) -> u64 {
        (self.interlace as u64) | ((self.ffmd as u64) << 1) | (((self.dpms as u64) & 0x3) << 2)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            interlace: (raw & 1) != 0,
            ffmd: ((raw >> 1) & 1) != 0,
            dpms: ((raw >> 2) & 0x3) as u8,
        }
    }
}

/// Decoded/encoded fields of `GS DISPFB1` (`0x1200_0070`, `FETCHED` `Play! GSHandler.h`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DispFb1Fields {
    pub fbp: u16,
    pub fbw: u8,
    pub psm: u8,
    pub dbx: u16,
    pub dby: u16,
}

impl DispFb1Fields {
    pub const fn encode(self) -> u64 {
        ((self.fbp as u64) & 0x1FF)
            | (((self.fbw as u64) & 0x3F) << 9)
            | (((self.psm as u64) & 0x1F) << 15)
            | (((self.dbx as u64) & 0x7FF) << 32)
            | (((self.dby as u64) & 0x7FF) << 43)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            fbp: (raw & 0x1FF) as u16,
            fbw: ((raw >> 9) & 0x3F) as u8,
            psm: ((raw >> 15) & 0x1F) as u8,
            dbx: ((raw >> 32) & 0x7FF) as u16,
            dby: ((raw >> 43) & 0x7FF) as u16,
        }
    }
}

/// Decoded/encoded fields of `GS DISPLAY1` (`0x1200_0080`, `FETCHED` `Play! GSHandler.cpp`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Display1Fields {
    pub dx: u16,
    pub dy: u16,
    pub magh: u8,
    pub magv: u8,
    pub dw: u16,
    pub dh: u16,
}

impl Display1Fields {
    pub const fn encode(self) -> u64 {
        ((self.dx as u64) & 0xFFF)
            | (((self.dy as u64) & 0x7FF) << 12)
            | (((self.magh as u64) & 0xF) << 23)
            | (((self.magv as u64) & 0x3) << 27)
            | (((self.dw as u64) & 0xFFF) << 32)
            | (((self.dh as u64) & 0x7FF) << 44)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            dx: (raw & 0xFFF) as u16,
            dy: ((raw >> 12) & 0x7FF) as u16,
            magh: ((raw >> 23) & 0xF) as u8,
            magv: ((raw >> 27) & 0x3) as u8,
            dw: ((raw >> 32) & 0xFFF) as u16,
            dh: ((raw >> 44) & 0x7FF) as u16,
        }
    }
}

/// Decoded/encoded fields of `GS BGCOLOR` (`0x1200_00E0`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BgColorFields {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl BgColorFields {
    pub const fn encode(self) -> u64 {
        (self.r as u64) | ((self.g as u64) << 8) | ((self.b as u64) << 16)
    }

    pub const fn decode(raw: u64) -> Self {
        Self {
            r: (raw & 0xFF) as u8,
            g: ((raw >> 8) & 0xFF) as u8,
            b: ((raw >> 16) & 0xFF) as u8,
        }
    }
}

/// Full 6-register PCRTC display configuration prepended to emitted ELFs (`G-016`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DisplaySetupRegs {
    pub pmode: PmodeFields,
    pub smode1: Smode1Fields,
    pub smode2: Smode2Fields,
    pub dispfb1: DispFb1Fields,
    pub display1: Display1Fields,
    pub bgcolor: BgColorFields,
}

impl DisplaySetupRegs {
    /// Derive progressive NTSC `240p`-style PCRTC defaults for a `.gsp` target surface.
    pub fn from_surface(surface: &GspSurfaceHint) -> Self {
        let w = surface.width.max(1);
        let h = surface.height.clamp(1, 2048);
        let magh_plus_1 = (2560 / w).clamp(1, 16);
        let magh = (magh_plus_1 - 1) as u8;
        let clamped_w = w.min(4096 / magh_plus_1);
        let dw = (magh_plus_1 * clamped_w).saturating_sub(1) as u16;
        let dh = (h - 1) as u16;

        // With MMOD=1, SLBG=1, ALP=0xFF, BGCOLOR=0, PCRTC outputs RC1 unchanged regardless of
        // framebuffer alpha (`A=0x80` in fixtures 04..10 is never dimmed to 50%).
        Self {
            pmode: PmodeFields {
                en1: true,
                en2: false,
                crtmd: 1,
                mmod: true,
                amod: false,
                slbg: true,
                alp: 0xFF,
            },
            smode1: Smode1Fields::ntsc_progressive_default(),
            smode2: Smode2Fields {
                interlace: false,
                ffmd: false,
                dpms: 0,
            },
            dispfb1: DispFb1Fields {
                fbp: ((surface.bp_blocks / 32) & 0x1FF) as u16,
                fbw: (surface.bw & 0x3F) as u8,
                psm: scanout_psm_bits(surface.format),
                dbx: 0,
                dby: 0,
            },
            display1: Display1Fields {
                dx: 636,
                dy: 25,
                magh,
                magv: 0,
                dw,
                dh,
            },
            bgcolor: BgColorFields { r: 0, g: 0, b: 0 },
        }
    }

    pub const fn to_priv_writes(&self) -> [(u32, u64); 6] {
        [
            (priv_off::PMODE, self.pmode.encode()),
            (priv_off::SMODE1, self.smode1.encode()),
            (priv_off::SMODE2, self.smode2.encode()),
            (priv_off::DISPFB1, self.dispfb1.encode()),
            (priv_off::DISPLAY1, self.display1.encode()),
            (priv_off::BGCOLOR, self.bgcolor.encode()),
        ]
    }

    /// Apply this setup directly to a [`GsState`] for deterministic gate verification.
    pub fn apply_to_gs(&self, gs: &mut GsState) {
        for (off, val) in self.to_priv_writes() {
            gs.write_privileged_display(off, val);
        }
    }
}

const fn scanout_psm_bits(fmt: PixelFormat) -> u8 {
    // Framebuffer scanout PSM field (`DISPFB.PSM`, 5-bit): mask Z-bit/IDs cleanly onto valid 5 bits.
    (fmt as u8) & 0x1F
}

/// Complete generated ELF binary and structured metadata for diagnostics and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedElf {
    pub bytes: Vec<u8>,
    pub stub_words: Vec<u32>,
    pub priv_writes: Vec<(u32, u64)>,
    pub qwc_stream: Vec<[u64; 2]>,
    pub priv_table_vaddr: u32,
    pub qwc_stream_vaddr: u32,
}

fn flush_ad_batch(pending: &mut Vec<(u8, u64)>, out_qwcs: &mut Vec<[u64; 2]>) {
    let mut start = 0usize;
    while start < pending.len() {
        let count = (pending.len() - start).min(0x7FFF);
        // A+D GIFtag: NLOOP=count, EOP=1, PRE=0, FLG=0 (PACKED), NREG=1, REGS[0]=0xE (A+D).
        let tag_lo = (count as u64) | (1u64 << 15) | (1u64 << 60);
        let tag_hi = 0x0Eu64;
        out_qwcs.push([tag_lo, tag_hi]);
        for &(addr, data) in &pending[start..start + count] {
            out_qwcs.push([data, u64::from(addr)]);
        }
        start += count;
    }
    pending.clear();
}

/// Build an R5900 MIPS ELF32 binary replaying `commands` using PCRTC defaults for `surface`.
pub fn build_elf_from_gsp(
    commands: &[GspCommand],
    surface: &GspSurfaceHint,
) -> Result<EmittedElf, ElfEmitError> {
    let display = DisplaySetupRegs::from_surface(surface);
    let mut priv_writes = Vec::with_capacity(6 + commands.len());
    priv_writes.extend_from_slice(&display.to_priv_writes());

    let mut qwc_stream = Vec::new();
    let mut pending_ad = Vec::new();

    for &cmd in commands {
        match cmd {
            GspCommand::InternalReg { addr, data } => {
                pending_ad.push((addr, data));
            }
            GspCommand::Qwc(q) => {
                flush_ad_batch(&mut pending_ad, &mut qwc_stream);
                qwc_stream.push([q.lo_u64(), q.hi_u64()]);
            }
            GspCommand::PrivilegedReg { offset, data } => {
                if offset > priv_off::BGCOLOR || (offset & 0xF) != 0 {
                    return Err(ElfEmitError::InvalidPrivOffset(offset));
                }
                priv_writes.push((offset, data));
            }
            GspCommand::Surface(_) => {}
        }
    }
    flush_ad_batch(&mut pending_ad, &mut qwc_stream);

    let priv_table_vaddr = PRIV_TABLE_VADDR;
    let priv_bytes = priv_writes.len() * 16;
    let qwc_stream_vaddr = priv_table_vaddr + (priv_bytes as u32);
    let qwc_bytes = qwc_stream.len() * 16;
    let payload_len = (STUB_BYTE_LEN as usize) + priv_bytes + qwc_bytes;

    if payload_len > 30 * 1024 * 1024 {
        return Err(ElfEmitError::PayloadTooLarge(payload_len));
    }

    let stub_words = assemble_r5900_stub(
        priv_table_vaddr,
        priv_writes.len() as u32,
        qwc_stream_vaddr,
        qwc_stream.len() as u32,
    )?;

    let total_len = (ELF_LOAD_FILE_OFFSET as usize) + payload_len;
    let mut bytes = Vec::with_capacity(total_len);

    // 52-byte Elf32_Ehdr
    bytes.extend_from_slice(b"\x7FELF");
    bytes.push(1); // EI_CLASS = ELFCLASS32
    bytes.push(1); // EI_DATA = ELFDATA2LSB
    bytes.push(1); // EI_VERSION = EV_CURRENT
    bytes.extend_from_slice(&[0u8; 9]); // EI_OSABI..padding
    bytes.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    bytes.extend_from_slice(&8u16.to_le_bytes()); // e_machine = EM_MIPS
    bytes.extend_from_slice(&1u32.to_le_bytes()); // e_version = 1
    bytes.extend_from_slice(&ELF_ENTRY_VADDR.to_le_bytes()); // e_entry
    bytes.extend_from_slice(&ELF32_EHDR_SIZE.to_le_bytes()); // e_phoff = 52
    bytes.extend_from_slice(&0u32.to_le_bytes()); // e_shoff = 0
    bytes.extend_from_slice(&0u32.to_le_bytes()); // e_flags = 0
    bytes.extend_from_slice(&(ELF32_EHDR_SIZE as u16).to_le_bytes()); // e_ehsize = 52
    bytes.extend_from_slice(&(ELF32_PHDR_SIZE as u16).to_le_bytes()); // e_phentsize = 32
    bytes.extend_from_slice(&1u16.to_le_bytes()); // e_phnum = 1
    bytes.extend_from_slice(&0u16.to_le_bytes()); // e_shentsize = 0
    bytes.extend_from_slice(&0u16.to_le_bytes()); // e_shnum = 0
    bytes.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx = 0

    // 32-byte Elf32_Phdr
    bytes.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    bytes.extend_from_slice(&ELF_LOAD_FILE_OFFSET.to_le_bytes()); // p_offset = 0x60
    bytes.extend_from_slice(&ELF_ENTRY_VADDR.to_le_bytes()); // p_vaddr
    bytes.extend_from_slice(&ELF_ENTRY_VADDR.to_le_bytes()); // p_paddr
    bytes.extend_from_slice(&(payload_len as u32).to_le_bytes()); // p_filesz
    bytes.extend_from_slice(&(payload_len as u32).to_le_bytes()); // p_memsz
    bytes.extend_from_slice(&7u32.to_le_bytes()); // p_flags = PF_R | PF_W | PF_X
    bytes.extend_from_slice(&16u32.to_le_bytes()); // p_align = 16

    // Pad from byte 84 to 96 (0x60) so LQ/SQ data alignment inside file and RAM match.
    bytes.resize(ELF_LOAD_FILE_OFFSET as usize, 0);

    for &word in &stub_words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for &(off, val) in &priv_writes {
        bytes.extend_from_slice(&(GS_PRIV_BASE_ADDR + u64::from(off)).to_le_bytes());
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    for &[lo, hi] in &qwc_stream {
        bytes.extend_from_slice(&lo.to_le_bytes());
        bytes.extend_from_slice(&hi.to_le_bytes());
    }

    Ok(EmittedElf {
        bytes,
        stub_words,
        priv_writes,
        qwc_stream,
        priv_table_vaddr,
        qwc_stream_vaddr,
    })
}

const REG_ZERO: u32 = 0;
const REG_T0: u32 = 8;
const REG_T1: u32 = 9;
const REG_T2: u32 = 10;
const REG_T3: u32 = 11;
const REG_T4: u32 = 12;
const REG_S0: u32 = 16;
const REG_S1: u32 = 17;

#[inline]
const fn enc_i(op: u32, rs: u32, rt: u32, imm: u16) -> u32 {
    ((op & 0x3F) << 26) | ((rs & 31) << 21) | ((rt & 31) << 16) | (imm as u32)
}

#[inline]
const fn enc_nop() -> u32 {
    0
}

#[inline]
const fn enc_srl(rd: u32, rt: u32, shamt: u32) -> u32 {
    ((rt & 31) << 16) | ((rd & 31) << 11) | ((shamt & 31) << 6) | 0x02
}

#[inline]
const fn enc_beq(rs: u32, rt: u32, off: i16) -> u32 {
    enc_i(0x04, rs, rt, off as u16)
}

#[inline]
const fn enc_bne(rs: u32, rt: u32, off: i16) -> u32 {
    enc_i(0x05, rs, rt, off as u16)
}

#[inline]
const fn enc_addiu(rt: u32, rs: u32, imm: i16) -> u32 {
    enc_i(0x09, rs, rt, imm as u16)
}

#[inline]
const fn enc_sltiu(rt: u32, rs: u32, imm: u16) -> u32 {
    enc_i(0x0B, rs, rt, imm)
}

#[inline]
const fn enc_andi(rt: u32, rs: u32, imm: u16) -> u32 {
    enc_i(0x0C, rs, rt, imm)
}

#[inline]
const fn enc_ori(rt: u32, rs: u32, imm: u16) -> u32 {
    enc_i(0x0D, rs, rt, imm)
}

#[inline]
const fn enc_lui(rt: u32, imm: u16) -> u32 {
    enc_i(0x0F, REG_ZERO, rt, imm)
}

#[inline]
const fn enc_lq(rt: u32, off: i16, rs: u32) -> u32 {
    enc_i(0x1E, rs, rt, off as u16)
}

#[inline]
const fn enc_sq(rt: u32, off: i16, rs: u32) -> u32 {
    enc_i(0x1F, rs, rt, off as u16)
}

#[inline]
const fn enc_lw(rt: u32, off: i16, rs: u32) -> u32 {
    enc_i(0x23, rs, rt, off as u16)
}

#[inline]
const fn enc_ld(rt: u32, off: i16, rs: u32) -> u32 {
    enc_i(0x37, rs, rt, off as u16)
}

#[inline]
const fn enc_sd(rt: u32, off: i16, rs: u32) -> u32 {
    enc_i(0x3F, rs, rt, off as u16)
}

fn push_u32_load(code: &mut Vec<u32>, rt: u32, value: u32) {
    code.push(enc_lui(rt, (value >> 16) as u16));
    code.push(enc_ori(rt, rt, (value & 0xFFFF) as u16));
}

#[inline]
fn branch_offset(branch_idx: usize, target_idx: usize) -> i16 {
    (target_idx as isize - (branch_idx as isize + 1)) as i16
}

fn assemble_r5900_stub(
    priv_vaddr: u32,
    n_priv: u32,
    qwc_vaddr: u32,
    n_qwc: u32,
) -> Result<Vec<u32>, ElfEmitError> {
    let mut code = Vec::with_capacity(STUB_INSTR_COUNT);

    // Part A: Write all N_PRIV privileged GS registers (`N_PRIV >= 6` always).
    push_u32_load(&mut code, REG_T0, priv_vaddr);
    push_u32_load(&mut code, REG_T1, n_priv);

    let priv_loop_idx = code.len();
    code.push(enc_ld(REG_T2, 0, REG_T0));
    code.push(enc_ld(REG_T3, 8, REG_T0));
    code.push(enc_addiu(REG_T0, REG_T0, 16));
    code.push(enc_addiu(REG_T1, REG_T1, -1));
    let priv_bne_idx = code.len();
    code.push(enc_bne(
        REG_T1,
        REG_ZERO,
        branch_offset(priv_bne_idx, priv_loop_idx),
    ));
    code.push(enc_sd(REG_T3, 0, REG_T2)); // delay slot

    // Part B: Stream N_QWC quadwords to GIF_FIFO (`0x1000_6000`), waiting on GIF_STAT.FQC < 15.
    push_u32_load(&mut code, REG_T1, n_qwc);
    let skip_qwc_branch_idx = code.len();
    code.push(0); // patched once `spin_idx` is known
    code.push(enc_lui(REG_T0, (qwc_vaddr >> 16) as u16)); // delay slot
    code.push(enc_ori(REG_T0, REG_T0, (qwc_vaddr & 0xFFFF) as u16));
    push_u32_load(&mut code, REG_S0, GIF_FIFO_ADDR);
    push_u32_load(&mut code, REG_S1, GIF_STAT_ADDR);

    let qwc_loop_idx = code.len();
    code.push(enc_lq(REG_T2, 0, REG_T0));
    let wait_fifo_idx = code.len();
    code.push(enc_lw(REG_T3, 0, REG_S1));
    code.push(enc_srl(REG_T3, REG_T3, 24));
    code.push(enc_andi(REG_T3, REG_T3, 0x1F));
    code.push(enc_sltiu(REG_T4, REG_T3, 15));
    let wait_beq_idx = code.len();
    code.push(enc_beq(
        REG_T4,
        REG_ZERO,
        branch_offset(wait_beq_idx, wait_fifo_idx),
    ));
    code.push(enc_nop());
    code.push(enc_addiu(REG_T0, REG_T0, 16));
    code.push(enc_addiu(REG_T1, REG_T1, -1));
    let qwc_bne_idx = code.len();
    code.push(enc_bne(
        REG_T1,
        REG_ZERO,
        branch_offset(qwc_bne_idx, qwc_loop_idx),
    ));
    code.push(enc_sq(REG_T2, 0, REG_S0)); // delay slot

    // Part C: Position-independent infinite idle loop (`0x1000_FFFF`).
    let spin_idx = code.len();
    code[skip_qwc_branch_idx] = enc_beq(
        REG_T1,
        REG_ZERO,
        branch_offset(skip_qwc_branch_idx, spin_idx),
    );
    code.push(enc_beq(
        REG_ZERO,
        REG_ZERO,
        branch_offset(spin_idx, spin_idx),
    ));
    code.push(enc_nop());

    if code.len() != STUB_INSTR_COUNT {
        return Err(ElfEmitError::StubLayoutMismatch {
            expected: STUB_INSTR_COUNT,
            actual: code.len(),
        });
    }

    Ok(code)
}

fn reg_name(r: u32) -> &'static str {
    match r {
        0 => "$zero",
        8 => "$t0",
        9 => "$t1",
        10 => "$t2",
        11 => "$t3",
        12 => "$t4",
        16 => "$s0",
        17 => "$s1",
        _ => "$r?",
    }
}

/// Disassemble one R5900 stub instruction word at virtual address `pc`.
pub fn disassemble_r5900_word(pc: u32, word: u32) -> String {
    if word == 0 {
        return format!("0x{pc:08X}:  {word:08X}   nop");
    }
    let op = (word >> 26) & 0x3F;
    let rs = (word >> 21) & 0x1F;
    let rt = (word >> 16) & 0x1F;
    let rd = (word >> 11) & 0x1F;
    let sa = (word >> 6) & 0x1F;
    let funct = word & 0x3F;
    let imm = (word & 0xFFFF) as u16;
    let simm = imm as i16;
    let target = pc
        .wrapping_add(4)
        .wrapping_add((i32::from(simm) << 2) as u32);

    let asm = match op {
        0x00 if funct == 0x02 => format!("srl   {}, {}, {}", reg_name(rd), reg_name(rt), sa),
        0x04 => format!("beq   {}, {}, 0x{target:08X}", reg_name(rs), reg_name(rt)),
        0x05 => format!("bne   {}, {}, 0x{target:08X}", reg_name(rs), reg_name(rt)),
        0x09 => format!("addiu {}, {}, {}", reg_name(rt), reg_name(rs), simm),
        0x0B => format!("sltiu {}, {}, {imm}", reg_name(rt), reg_name(rs)),
        0x0C => format!("andi  {}, {}, 0x{imm:02X}", reg_name(rt), reg_name(rs)),
        0x0D => format!("ori   {}, {}, 0x{imm:04X}", reg_name(rt), reg_name(rs)),
        0x0F => format!("lui   {}, 0x{imm:04X}", reg_name(rt)),
        0x1E => format!("lq    {}, {simm}({})", reg_name(rt), reg_name(rs)),
        0x1F => format!("sq    {}, {simm}({})", reg_name(rt), reg_name(rs)),
        0x23 => format!("lw    {}, {simm}({})", reg_name(rt), reg_name(rs)),
        0x37 => format!("ld    {}, {simm}({})", reg_name(rt), reg_name(rs)),
        0x3F => format!("sd    {}, {simm}({})", reg_name(rt), reg_name(rs)),
        _ => format!(".word 0x{word:08X}"),
    };
    format!("0x{pc:08X}:  {word:08X}   {asm}")
}

/// Disassemble every word in an emitted R5900 stub slice starting at `ELF_ENTRY_VADDR`.
pub fn disassemble_stub(stub_words: &[u32]) -> Vec<String> {
    stub_words
        .iter()
        .enumerate()
        .map(|(i, &w)| disassemble_r5900_word(ELF_ENTRY_VADDR + (i as u32) * 4, w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ps2_gs::parse_gsp_script;

    #[test]
    fn decode_all_six_display_registers_field_by_field() {
        let surface = GspSurfaceHint {
            bp_blocks: 0,
            bw: 5,
            format: PixelFormat::Psmct32,
            width: 320,
            height: 240,
        };
        let setup = DisplaySetupRegs::from_surface(&surface);
        let writes = setup.to_priv_writes();

        let (off0, raw_pmode) = writes[0];
        assert_eq!(off0, priv_off::PMODE);
        assert_eq!(raw_pmode, 0x0000_0000_0000_FFA5);
        assert_eq!(
            PmodeFields::decode(raw_pmode),
            PmodeFields {
                en1: true,
                en2: false,
                crtmd: 1,
                mmod: true,
                amod: false,
                slbg: true,
                alp: 0xFF,
            }
        );

        let (off1, raw_smode1) = writes[1];
        assert_eq!(off1, priv_off::SMODE1);
        assert_eq!(raw_smode1, 0x0000_0007_4080_4504);
        assert_eq!(
            Smode1Fields::decode(raw_smode1),
            Smode1Fields::ntsc_progressive_default()
        );

        let (off2, raw_smode2) = writes[2];
        assert_eq!(off2, priv_off::SMODE2);
        assert_eq!(raw_smode2, 0x0000_0000_0000_0000);
        assert_eq!(
            Smode2Fields::decode(raw_smode2),
            Smode2Fields {
                interlace: false,
                ffmd: false,
                dpms: 0,
            }
        );

        let (off3, raw_dispfb1) = writes[3];
        assert_eq!(off3, priv_off::DISPFB1);
        assert_eq!(raw_dispfb1, 0x0000_0000_0000_0A00);
        assert_eq!(
            DispFb1Fields::decode(raw_dispfb1),
            DispFb1Fields {
                fbp: 0,
                fbw: 5,
                psm: 0,
                dbx: 0,
                dby: 0,
            }
        );

        let (off4, raw_display1) = writes[4];
        assert_eq!(off4, priv_off::DISPLAY1);
        assert_eq!(raw_display1, 0x000E_F9FF_0381_927C);
        assert_eq!(
            Display1Fields::decode(raw_display1),
            Display1Fields {
                dx: 636,
                dy: 25,
                magh: 7,
                magv: 0,
                dw: 2559,
                dh: 239,
            }
        );

        let (off5, raw_bgcolor) = writes[5];
        assert_eq!(off5, priv_off::BGCOLOR);
        assert_eq!(raw_bgcolor, 0x0000_0000_0000_0000);
        assert_eq!(
            BgColorFields::decode(raw_bgcolor),
            BgColorFields { r: 0, g: 0, b: 0 }
        );
    }

    #[test]
    fn stub_disassembly_and_elf_layout_are_valid() {
        let surface = GspSurfaceHint {
            bp_blocks: 0,
            bw: 5,
            format: PixelFormat::Psmct32,
            width: 320,
            height: 240,
        };
        let cmds = parse_gsp_script(
            "reg 0x1A 1\n\
             reg 0 6\n\
             qwc 0x8001 0x0E\n",
        )
        .expect("parse gsp");
        let elf = build_elf_from_gsp(&cmds, &surface).expect("emit elf");
        assert_eq!(&elf.bytes[0..4], b"\x7FELF");
        assert_eq!(elf.priv_table_vaddr & 0xF, 0);
        assert_eq!(elf.qwc_stream_vaddr & 0xF, 0);
        assert_eq!(elf.priv_writes.len(), 6);
        // First 2 `reg` commands pack into 1 A+D GIFtag + 2 payload QWCs = 3 QWCs, plus 1 raw Qwc = 4 QWCs.
        assert_eq!(elf.qwc_stream.len(), 4);

        let disasm = disassemble_stub(&elf.stub_words);
        assert_eq!(disasm.len(), STUB_INSTR_COUNT);
        // Every stub word must decode to a named instruction, never `.word`.
        assert!(
            disasm.iter().all(|line| !line.contains(".word")),
            "unexpected unknown instruction in stub: {disasm:?}"
        );
        // Final spin loop instruction is exact `0x1000_FFFF` jumping to itself (`0x0010_0078`).
        assert_eq!(*elf.stub_words.iter().rev().nth(1).unwrap(), R5900_SPIN_BEQ);
        assert!(disasm[30].contains("beq   $zero, $zero, 0x00100078"));
        // Part A loops back to `0x0010_0010` (`priv_loop_idx = 4`).
        assert!(disasm[8].contains("bne   $t1, $zero, 0x00100010"));
        // Zero-QWC branch targets `spin` at `0x0010_0078`.
        assert!(disasm[12].contains("beq   $t1, $zero, 0x00100078"));
        // FIFO polling loop targets `wait_fifo` at `0x0010_0050`.
        assert!(disasm[24].contains("beq   $t4, $zero, 0x00100050"));
        // QWC stream loop targets `qwc_loop` at `0x0010_004C`.
        assert!(disasm[28].contains("bne   $t1, $zero, 0x0010004C"));
    }
}
