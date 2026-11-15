//! High-Level Emulation (HLE) of IOP IRX module imports for `ps2autotests` (`Task 6a`).
//!
//! Primary source references:
//! - `ps2autotests` (`tests/cpu/iop/*.irx` & `common/common-iop.mk`):
//!   * IOP IRX import tables in `.text` start with the 20-byte header:
//!     - `+0x00`: `0x41E0_0000` (`IOP_IMPORT_MAGIC`, `bc0f` instruction encoding)
//!     - `+0x04`: `0x0000_0000` (`next` pointer)
//!     - `+0x08`: `u32` `version` (`0x0102` for `stdio`, `0x0101` for `sysclib`)
//!     - `+0x0C..+0x14`: 8-byte ASCII module name (`"stdio\0\0\0"`, `"sysclib\0"`)
//!   * Followed by 8-byte stub entries (`0x03E0_0008` `jr $ra`; `0x2400_xxxx` `addiu $zero, $zero, index`)
//!     terminated by `0x0000_0000, 0x0000_0000`.
//!   * Across the 7 `cpu/iop/*.irx` test binaries:
//!     - `stdio` index `4` (`printf`) is called by all 7 suites (`alu`, `branch`, `branchdelay`,
//!       `hilodelay`, `lsu`, `lsudelay`, `muldiv`).
//!     - `sysclib` index `12` (`memcpy`) is called by `lsu.irx` (`tests/cpu/iop/lsu.c` lines 135, 141, 166, 173).

use crate::bus::Bus;
use crate::iop::state::IopCpuState;

/// Magic first word of an IOP `.iop.import` stub table (`0x41E0_0000`, `ps2autotests` `cpu/iop/*.irx`).
pub const IOP_IMPORT_MAGIC: u32 = 0x41E0_0000;

/// Instruction encoding of `jr $ra` (`0x03E0_0008`), first word of every IOP import stub.
pub const MIPS_JR_RA: u32 = 0x03E0_0008;

/// Error returned when an `.irx` binary invokes an unimplemented IOP module import stub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnimplementedIopImport {
    /// Guest IOP PC of the stub entry (`jr $ra`).
    pub pc: u32,
    /// 8-byte ASCII module name from the enclosing `.iop.import` header.
    pub module: String,
    /// Import function index from the delay-slot `addiu $zero, $zero, index` instruction.
    pub index: u16,
}

impl std::fmt::Display for UnimplementedIopImport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UnimplementedIopImport(pc=0x{:08X}, module={:?}, index={})",
            self.pc, self.module, self.index
        )
    }
}

/// Inspect whether `pc` (`instr == 0x03E0_0008`, `jr $ra`) is an IOP `.iop.import` stub entry.
///
/// If `pc` is inside an `IOP_IMPORT_MAGIC` (`0x41E0_0000`) table, executes the HLE handler for
/// `(module, index)` or returns `Err(UnimplementedIopImport)`.
pub fn try_handle_iop_import_stub(
    iop: &mut IopCpuState,
    bus: &mut Bus,
    pc: u32,
) -> Result<bool, UnimplementedIopImport> {
    let delay_instr = bus.iop_read32(pc.wrapping_add(4));
    // Every IOP import stub delay slot is `addiu $zero, $zero, index` (`0x2400_xxxx`).
    if (delay_instr & 0xFFFF_0000) != 0x2400_0000 {
        return Ok(false);
    }
    let import_index = (delay_instr & 0xFFFF) as u16;

    // Scan backwards by 8-byte stub slots to locate the enclosing 20-byte `0x41E0_0000` header.
    let Some(module_name) = find_enclosing_import_module(bus, pc) else {
        return Ok(false);
    };

    match (module_name.as_str(), import_index) {
        // Verified against `local/ps2autotests/tests/cpu/iop/*.irx`:
        // `stdio` export #4 is `int printf(const char *fmt, ...)`.
        ("stdio", 4) => {
            let written = hle_iop_printf(iop, bus);
            iop.write_gpr(2, written as u32);
            Ok(true)
        }
        // Verified against `local/ps2autotests/tests/cpu/iop/lsu.irx` (`lsu.c` lines 135, 141, 166, 173):
        // `sysclib` export #12 is `void *memcpy(void *dst, const void *src, size_t len)`.
        ("sysclib", 12) => {
            let dst = iop.read_gpr(4);
            let src = iop.read_gpr(5);
            let len = iop.read_gpr(6);
            for i in 0..len {
                let b = bus.iop_read8(src.wrapping_add(i));
                bus.iop_write8(dst.wrapping_add(i), b);
            }
            iop.write_gpr(2, dst);
            Ok(true)
        }
        _ => Err(UnimplementedIopImport {
            pc,
            module: module_name,
            index: import_index,
        }),
    }
}

/// Walk backwards from `stub_pc` in 8-byte steps (`MAX_STUBS = 128`) looking for the 20-byte
/// `IOP_IMPORT_MAGIC` (`0x41E0_0000`) header.
fn find_enclosing_import_module(bus: &mut Bus, stub_pc: u32) -> Option<String> {
    let mut cursor = stub_pc;
    for _ in 0..128 {
        if cursor < 0x14 {
            return None;
        }
        let header_candidate = cursor - 0x14;
        if bus.iop_read32(header_candidate) == IOP_IMPORT_MAGIC
            && bus.iop_read32(header_candidate + 4) == 0
        {
            let mut name_bytes = [0u8; 8];
            for (i, slot) in name_bytes.iter_mut().enumerate() {
                *slot = bus.iop_read8(header_candidate + 12 + (i as u32));
            }
            let end = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
            if let Ok(s) = std::str::from_utf8(&name_bytes[..end]) {
                if !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic()) {
                    return Some(s.to_owned());
                }
            }
        }
        // Previous 8-byte entry in the table must also be a `jr $ra` stub.
        if cursor < 8 || bus.iop_read32(cursor - 8) != MIPS_JR_RA {
            return None;
        }
        cursor -= 8;
    }
    None
}

/// Read the `arg_index`-th 32-bit argument according to the MIPS O32 calling convention:
/// - `arg_index = 0..=3`: `$a0..$a3` (`gpr[4]..gpr[7]`)
/// - `arg_index >= 4`: stack word at `$sp + 4 * arg_index`
#[inline]
fn read_o32_arg(iop: &IopCpuState, bus: &mut Bus, arg_index: usize) -> u32 {
    if arg_index < 4 {
        iop.read_gpr((4 + arg_index) as u8)
    } else {
        let sp = iop.read_gpr(29);
        bus.iop_read32(sp.wrapping_add((arg_index as u32) * 4))
    }
}

/// Read a null-terminated byte string from IOP memory starting at `vaddr`.
fn read_iop_c_string(bus: &mut Bus, vaddr: u32, max_len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..max_len {
        let b = bus.iop_read8(vaddr.wrapping_add(i as u32));
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

/// HLE implementation of IOP `stdio` export #4 (`printf`).
///
/// Formats the guest format string at `$a0` and variadic arguments (`$a1..$a3`, stack `$sp + 16..`)
/// and appends the output bytes to `bus.stdout_buffer`.
fn hle_iop_printf(iop: &IopCpuState, bus: &mut Bus) -> usize {
    let fmt_ptr = read_o32_arg(iop, bus, 0);
    let fmt_bytes = read_iop_c_string(bus, fmt_ptr, 4096);
    let mut arg_idx = 1usize;
    let mut out = Vec::with_capacity(fmt_bytes.len() + 32);
    let mut i = 0usize;

    while i < fmt_bytes.len() {
        if fmt_bytes[i] != b'%' {
            out.push(fmt_bytes[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= fmt_bytes.len() {
            break;
        }
        if fmt_bytes[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }

        // Parse flags: '-', '+', ' ', '0', '#'
        let mut left_align = false;
        let mut plus_sign = false;
        let mut space_sign = false;
        let mut zero_pad = false;
        while i < fmt_bytes.len() {
            match fmt_bytes[i] {
                b'-' => left_align = true,
                b'+' => plus_sign = true,
                b' ' => space_sign = true,
                b'0' => zero_pad = true,
                b'#' => {}
                _ => break,
            }
            i += 1;
        }

        // Parse field width
        let mut width = 0usize;
        if i < fmt_bytes.len() && fmt_bytes[i] == b'*' {
            width = read_o32_arg(iop, bus, arg_idx) as usize;
            arg_idx += 1;
            i += 1;
        } else {
            while i < fmt_bytes.len() && fmt_bytes[i].is_ascii_digit() {
                width = width
                    .saturating_mul(10)
                    .saturating_add((fmt_bytes[i] - b'0') as usize);
                i += 1;
            }
        }

        // Skip optional precision (`.number` or `.*`)
        if i < fmt_bytes.len() && fmt_bytes[i] == b'.' {
            i += 1;
            if i < fmt_bytes.len() && fmt_bytes[i] == b'*' {
                let _ = read_o32_arg(iop, bus, arg_idx);
                arg_idx += 1;
                i += 1;
            } else {
                while i < fmt_bytes.len() && fmt_bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }

        // Skip optional length modifiers ('h', 'l')
        while i < fmt_bytes.len() && matches!(fmt_bytes[i], b'h' | b'l') {
            i += 1;
        }

        if i >= fmt_bytes.len() {
            break;
        }
        let spec = fmt_bytes[i];
        i += 1;

        match spec {
            b's' => {
                let s_ptr = read_o32_arg(iop, bus, arg_idx);
                arg_idx += 1;
                let s_bytes = if s_ptr == 0 {
                    b"(null)".to_vec()
                } else {
                    read_iop_c_string(bus, s_ptr, 4096)
                };
                append_padded(&mut out, &s_bytes, width, left_align, b' ');
            }
            b'd' | b'i' => {
                let val = read_o32_arg(iop, bus, arg_idx) as i32;
                arg_idx += 1;
                let sign_prefix: &[u8] = if val < 0 {
                    b"-"
                } else if plus_sign {
                    b"+"
                } else if space_sign {
                    b" "
                } else {
                    b""
                };
                let digits = val.unsigned_abs().to_string();
                format_number_with_sign(
                    &mut out,
                    sign_prefix,
                    digits.as_bytes(),
                    width,
                    left_align,
                    zero_pad,
                );
            }
            b'u' => {
                let val = read_o32_arg(iop, bus, arg_idx);
                arg_idx += 1;
                let digits = val.to_string();
                format_number_with_sign(
                    &mut out,
                    b"",
                    digits.as_bytes(),
                    width,
                    left_align,
                    zero_pad,
                );
            }
            b'x' | b'p' => {
                let val = read_o32_arg(iop, bus, arg_idx);
                arg_idx += 1;
                let digits = format!("{val:x}");
                format_number_with_sign(
                    &mut out,
                    b"",
                    digits.as_bytes(),
                    width,
                    left_align,
                    zero_pad,
                );
            }
            b'X' => {
                let val = read_o32_arg(iop, bus, arg_idx);
                arg_idx += 1;
                let digits = format!("{val:X}");
                format_number_with_sign(
                    &mut out,
                    b"",
                    digits.as_bytes(),
                    width,
                    left_align,
                    zero_pad,
                );
            }
            b'c' => {
                let val = (read_o32_arg(iop, bus, arg_idx) & 0xFF) as u8;
                arg_idx += 1;
                append_padded(&mut out, &[val], width, left_align, b' ');
            }
            other => {
                out.push(b'%');
                out.push(other);
            }
        }
    }

    let written = out.len();
    bus.stdout_buffer.extend_from_slice(&out);
    written
}

fn append_padded(out: &mut Vec<u8>, content: &[u8], width: usize, left_align: bool, pad: u8) {
    let pad_len = width.saturating_sub(content.len());
    if !left_align {
        for _ in 0..pad_len {
            out.push(pad);
        }
    }
    out.extend_from_slice(content);
    if left_align {
        for _ in 0..pad_len {
            out.push(b' ');
        }
    }
}

fn format_number_with_sign(
    out: &mut Vec<u8>,
    sign: &[u8],
    digits: &[u8],
    width: usize,
    left_align: bool,
    zero_pad: bool,
) {
    let total_len = sign.len() + digits.len();
    let pad_len = width.saturating_sub(total_len);
    if !left_align && !zero_pad {
        for _ in 0..pad_len {
            out.push(b' ');
        }
    }
    out.extend_from_slice(sign);
    if !left_align && zero_pad {
        for _ in 0..pad_len {
            out.push(b'0');
        }
    }
    out.extend_from_slice(digits);
    if left_align {
        for _ in 0..pad_len {
            out.push(b' ');
        }
    }
}
