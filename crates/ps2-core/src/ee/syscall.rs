//! Minimal Task 3b EE Kernel HLE Syscall Handler (`crt0` startup and `fioWrite` stdout path).
//!
//! Primary source references:
//! - `ps2tek` (`https://psi-rockin.github.io/ps2tek/#bios`):
//!   * Standard EE kernel syscall numbers (`$v1`, negative numbers denote `i*` interrupt-safe variants).
//! - `local/ps2autotests` ELF disassembly (`tests/cpu/ee/alu.elf`, `tests/cpu/ee/lsu.elf`):
//!   * `_start` (`0x0010_00E0` / `0x0010_00D8`): calls `SetupThread` (`0x3C`), `SetupHeap` (`0x3D`),
//!     `FlushCache` (`0x64`), `InitKernel` (`GetOsdConfigParam` `0x4B`, `SetOsdConfigParam` `0x4A`,
//!     `SetSyscall` `0x74`, `0x5A`, `0x5B`), `InitTLBFunctions`, `ExitDeleteThread` (`0x24`),
//!     `ExitThread` (`0x23`), `GetMemorySize` (`0x7F`), `SetTLBEntry` (`0x56`), and `Exit` (`0x04`).
//!   * `puts` (`0x0011_0390`) / `printf` (`0x0011_02C0`) -> `fioWrite` (`0x0011_25B0`) ->
//!     `fioInit` (`0x0011_3430`) -> `SifInitRpc` (`0x0011_31B8`) -> `SifInitCmd` (`0x0011_37E0`) ->
//!     `SifBindRpc` (`0x0011_3AA8`) -> `SifCallRpc` (`0x0011_2D48`) -> `_SifSendCmd` (`0x0011_3600`) ->
//!     `SifSetDma` (`0x77`).

use std::fmt;

use ps2_common::Reg128;

use crate::bus::{Bus, EE_RAM_SIZE};
use crate::ee::state::EeCpuState;

/// SIF command ID for `SifBindRpc` (`local/ps2autotests/tests/cpu/ee/alu.elf` at `0x0011_3B04`).
const SIF_CMD_RPC_BIND: u32 = 0x8000_0009;
/// SIF command ID for `SifCallRpc` (`local/ps2autotests/tests/cpu/ee/alu.elf` at `0x0011_2E48`).
const SIF_CMD_RPC_CALL: u32 = 0x8000_000A;
/// SIF RPC server ID for `FILEIO` (`local/ps2autotests/tests/cpu/ee/alu.elf` at `0x0011_349C`).
const SIF_RPC_SERVER_FILEIO: u32 = 0x8000_0001;
/// `FILEIO` RPC function number `3` (`FIO_F_WRITE`, `alu.elf` at `0x0011_2648`).
const FIO_RPC_WRITE: u32 = 3;

/// Saved thread descriptor created by `CreateThread` (`syscall 0x20`).
#[derive(Copy, Clone, Debug, Default)]
struct HleThreadParam {
    func: u32,
    stack: u32,
    stack_size: u32,
    gp_reg: u32,
}

/// Saved execution context of the parent (`main`) thread when `StartThread` switches to `test_thread`.
#[derive(Clone, Debug)]
struct SavedThreadContext {
    gpr: [Reg128; 32],
    pc: u32,
    hi: Reg128,
    lo: Reg128,
    sa: u32,
}

/// Error returned when an ELF invokes a syscall not stubbed in Task 3b.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SyscallError {
    /// Syscall number is not implemented in Task 3b (deferred to Task 7a).
    Unimplemented(i16),
}

impl fmt::Display for SyscallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unimplemented(num) => {
                write!(
                    f,
                    "unimplemented syscall {num} (0x{:02X})",
                    num.unsigned_abs()
                )
            }
        }
    }
}

impl std::error::Error for SyscallError {}

/// Minimal EE HLE kernel state for Task 3b ELF execution.
pub struct EeHleState {
    /// Bottom of the main thread stack set by `SetupThread` (`0x3C`).
    pub stack_bottom: u32,
    /// End of the heap set by `SetupHeap` (`0x3D`) and queried by `EndOfHeap` (`0x3E`).
    pub heap_end: u32,
    /// `_root` thread exit trampoline address passed to `SetupThread` (`$t0`).
    pub root_return_addr: u32,
    /// Next semaphore ID returned by `CreateSema` (`0x40`).
    next_sema_id: i32,
    /// Threads registered via `CreateThread` (`0x20`).
    threads: Vec<HleThreadParam>,
    /// Saved caller context while a single child thread spawned by `StartThread` (`0x22`) runs.
    caller_context: Option<SavedThreadContext>,
    /// OSD configuration word (`GetOsdConfigParam` `0x4B` / `SetOsdConfigParam` `0x4A`).
    /// Initialized with `version = 1` (`1 << 13`) so `InitKernel` detects a patched kernel.
    osd_config: u32,
    /// Custom syscall table entries installed via `SetSyscall` (`0x74`).
    custom_syscalls: [(u16, u32); 16],
    custom_syscall_count: usize,
    /// Whether the guest program has terminated via `Exit` (`0x04`), `ExitThread` (`0x23`), or `ExitDeleteThread` (`0x24`).
    pub exited: bool,
    /// Exit status code passed to `Exit($a0)`.
    pub exit_code: i32,
}

impl Default for EeHleState {
    fn default() -> Self {
        Self::new()
    }
}

impl EeHleState {
    /// Create a fresh HLE state for running a single EE ELF binary.
    pub fn new() -> Self {
        Self {
            stack_bottom: (EE_RAM_SIZE as u32) - 0x0002_0000,
            heap_end: (EE_RAM_SIZE as u32) - 0x0002_0000,
            root_return_addr: 0,
            next_sema_id: 1,
            threads: Vec::new(),
            caller_context: None,
            osd_config: 1 << 13,
            custom_syscalls: [(0, 0); 16],
            custom_syscall_count: 0,
            exited: false,
            exit_code: 0,
        }
    }

    /// Handle an EE `SYSCALL` instruction using the syscall number in `$v1` (`gpr[3]`).
    ///
    /// Returns `Ok(Some(next_pc))` if the syscall redirected `PC` (e.g. `StartThread` or `ExitThread`),
    /// `Ok(None)` if execution should advance normally past the `SYSCALL` instruction, or
    /// `Err(SyscallError::Unimplemented(num))` for any syscall outside the verified Task 3b set.
    pub fn handle_syscall(
        &mut self,
        cpu: &mut EeCpuState,
        bus: &mut Bus,
        resume_pc: u32,
    ) -> Result<Option<u32>, SyscallError> {
        // Reference: ps2tek (https://psi-rockin.github.io/ps2tek/#bios):
        // Syscall index is passed in $v1 (sign-extended 16-bit immediate from `addiu $v1, $0, imm`).
        let num = cpu.gpr_u32(3) as i16;
        let a0 = cpu.gpr_u32(4);
        let a1 = cpu.gpr_u32(5);
        let a2 = cpu.gpr_u32(6);
        let a3 = cpu.gpr_u32(7);
        let t0 = cpu.gpr_u32(8);

        match num {
            // 0x04: Exit(int status)
            // Reference: ps2tek #bios & crt0 in alu.elf (0x0010_0250) / lsu.elf (0x0010_0218).
            0x04 => {
                self.exit_code = a0 as i32;
                self.exited = true;
                Ok(None)
            }

            // 0x10: AddIntcHandler, 0x11: RemoveIntcHandler
            // Reference: ps2tek #bios & _ps2sdk_time_init in crt0 / libc init
            // (alu.elf at 0x0011_0848 -> 0x0011_22E0, performance.elf at 0x0010_4A80).
            0x10 | 0x11 => {
                cpu.set_gpr_sext32(2, 1);
                Ok(None)
            }

            // 0x12: AddDmacHandler, 0x13: RemoveDmacHandler
            // Reference: ps2tek #bios & SifInitCmd in alu.elf (0x0011_3938 -> 0x0011_3CC0).
            0x12 | 0x13 => {
                cpu.set_gpr_sext32(2, 1);
                Ok(None)
            }

            // 0x14: EnableIntc, 0x15: DisableIntc, -0x15: iDisableIntc
            // Reference: ps2tek #bios & _ps2sdk_time_init in crt0 / libc init
            // (alu.elf at 0x0011_0854 -> 0x0011_2908, performance.elf at 0x0010_5650).
            0x14 | 0x15 | -0x15 => {
                cpu.set_gpr_sext32(2, 1);
                Ok(None)
            }

            // 0x16: EnableDmac, 0x17: DisableDmac, -0x17: iDisableDmac
            // Reference: ps2tek #bios & SifInitCmd in alu.elf (0x0011_3948 -> 0x0011_3CE0).
            0x16 | 0x17 | -0x17 => {
                cpu.set_gpr_sext32(2, 1);
                Ok(None)
            }

            // 0x20: CreateThread(ThreadParam *param)
            // Reference: ps2tek #bios & common-ee.cpp main in lsu.elf (0x0010_4014 -> 0x0010_8150).
            0x20 => {
                let param = HleThreadParam {
                    func: bus.read32(a0.wrapping_add(4)),
                    stack: bus.read32(a0.wrapping_add(8)),
                    stack_size: bus.read32(a0.wrapping_add(12)),
                    gp_reg: bus.read32(a0.wrapping_add(16)),
                };
                self.threads.push(param);
                let thread_id = self.threads.len() as i32;
                cpu.set_gpr_sext32(2, thread_id);
                Ok(None)
            }

            // 0x22: StartThread(int thread_id, void *arg)
            // Reference: ps2tek #bios & common-ee.cpp main in lsu.elf (0x0010_4020 -> 0x0010_8160).
            0x22 => {
                let idx = (a0 as usize).saturating_sub(1);
                if let Some(&thread) = self.threads.get(idx) {
                    self.caller_context = Some(SavedThreadContext {
                        gpr: cpu.gpr,
                        pc: resume_pc,
                        hi: cpu.hi,
                        lo: cpu.lo,
                        sa: cpu.sa,
                    });
                    let sp = thread.stack.wrapping_add(thread.stack_size) & !0xF;
                    cpu.set_gpr_sext32(29, sp as i32);
                    if thread.gp_reg != 0 {
                        cpu.set_gpr_sext32(28, thread.gp_reg as i32);
                    }
                    cpu.set_gpr_sext32(4, a1 as i32);
                    cpu.set_gpr_sext32(31, self.root_return_addr as i32);
                    Ok(Some(thread.func))
                } else {
                    cpu.set_gpr_sext32(2, -1);
                    Ok(None)
                }
            }

            // 0x23: ExitThread(), 0x24: ExitDeleteThread()
            // Reference: ps2tek #bios & crt0 _root in alu.elf (0x0010_0248) and lsu.elf (0x0010_021C).
            0x23 | 0x24 => {
                if let Some(saved) = self.caller_context.take() {
                    cpu.gpr = saved.gpr;
                    cpu.hi = saved.hi;
                    cpu.lo = saved.lo;
                    cpu.sa = saved.sa;
                    cpu.set_gpr_sext32(2, 0);
                    Ok(Some(saved.pc))
                } else {
                    self.exited = true;
                    Ok(None)
                }
            }

            // 0x3C: RFU060 / SetupThread(void *gp, void *stack, int stack_size, char *args, void *root)
            // Reference: ps2tek #bios & crt0 in alu.elf (0x0010_0148) and lsu.elf (0x0010_0134).
            0x3C => {
                let stack_top = if a1 == 0xFFFF_FFFF {
                    EE_RAM_SIZE as u32
                } else {
                    a1.wrapping_add(a2)
                };
                self.stack_bottom = stack_top.wrapping_sub(a2);
                self.root_return_addr = t0;
                if a0 != 0 {
                    cpu.set_gpr_sext32(28, a0 as i32);
                }
                if a3 != 0 {
                    bus.write32(a3, 0); // argc = 0
                }
                cpu.set_gpr_sext32(2, stack_top as i32);
                Ok(None)
            }

            // 0x3D: RFU061 / SetupHeap(void *heap_start, int heap_size)
            // Reference: ps2tek #bios & crt0 in alu.elf (0x0010_0164) and lsu.elf (0x0010_0150).
            0x3D => {
                self.heap_end = if a1 == 0xFFFF_FFFF {
                    self.stack_bottom
                } else {
                    a0.wrapping_add(a1)
                };
                cpu.set_gpr_sext32(2, a0 as i32);
                Ok(None)
            }

            // 0x3E: EndOfHeap()
            // Reference: ps2tek #bios & sbrk in alu.elf (0x0011_2304).
            0x3E => {
                cpu.set_gpr_sext32(2, self.heap_end as i32);
                Ok(None)
            }

            // 0x40: CreateSema(SemaParam *param)
            // Reference: ps2tek #bios & _ps2sdk_libc_init / fioInit / SifBindRpc / SifCallRpc in alu.elf (0x0011_2314).
            0x40 => {
                let id = self.next_sema_id;
                self.next_sema_id = self.next_sema_id.wrapping_add(1).max(1);
                cpu.set_gpr_sext32(2, id);
                Ok(None)
            }

            // 0x41: DeleteSema, 0x42: SignalSema, -0x43: iSignalSema, 0x44: WaitSema
            // Reference: ps2tek #bios & fioWrite / SifCallRpc in alu.elf (0x0011_2324..0x0011_2344, 0x0011_2A24).
            0x41 | 0x42 | -0x43 | 0x44 => {
                cpu.set_gpr_sext32(2, a0 as i32);
                Ok(None)
            }

            // 0x4A: SetOsdConfigParam(void *addr), 0x4B: GetOsdConfigParam(void *addr)
            // Reference: ps2tek #bios & InitKernel in lsu.elf (0x0010_88A8 -> 0x0010_8D20 / 0x0010_8D30).
            0x4A => {
                self.osd_config = bus.read32(a0);
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }
            0x4B => {
                bus.write32(a0, self.osd_config);
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }

            // 0x55: PutTLBEntry, 0x56: SetTLBEntry, 0x5A: Copy, 0x5B: Setup, 0x74: SetSyscall
            // Reference: ps2tek #bios & InitKernel / InitTLBFunctions / InitTLB in lsu.elf (0x0010_8920..0x0010_8CC8).
            0x74 => {
                if self.custom_syscall_count < self.custom_syscalls.len() {
                    self.custom_syscalls[self.custom_syscall_count] = (a0 as u16, a1);
                    self.custom_syscall_count += 1;
                }
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }
            0x5A => {
                // Custom word-copy helper installed by InitTLBFunctions (lsu.elf 0x0010_89E8).
                let word_count = a2 >> 2;
                for i in 0..word_count {
                    let off = i.wrapping_mul(4);
                    let w = bus.read32(a1.wrapping_add(off));
                    bus.write32(a0.wrapping_add(off), w);
                }
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }
            0x55 | 0x56 | 0x5B => {
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }

            // 0x64: FlushCache(int mode)
            // Reference: ps2tek #bios & crt0 in alu.elf (0x0010_0170).
            0x64 => {
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }

            // 0x77: SifSetDma(SifDmaTransfer_t *dmat, int count), -0x77: iSifSetDma
            // Reference: ps2tek #bios & _SifSendCmd / SifBindRpc / SifCallRpc / fioWrite in alu.elf
            // (0x0011_25B0, 0x0011_2D48, 0x0011_3600, 0x0011_3AA8, 0x0011_3D10).
            0x77 | -0x77 => {
                self.handle_sif_set_dma(bus, a0, a1);
                cpu.set_gpr_sext32(2, 1);
                Ok(None)
            }

            // 0x78: SifSetDChain(), -0x78: iSifSetDChain()
            // Reference: ps2tek #bios & SifInitCmd in alu.elf (0x0011_3D30, 0x0011_3E20).
            0x78 | -0x78 => {
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }

            // 0x79: SifSetReg(u32 reg, u32 val), 0x7A: SifGetReg(u32 reg)
            // Reference: ps2tek #bios & SifInitCmd / SifInitRpc in alu.elf (0x0011_32B4, 0x0011_35E0..0x0011_35F0).
            0x79 => {
                cpu.set_gpr_sext32(2, 0);
                Ok(None)
            }
            0x7A => {
                let val = match a0 {
                    // SIF_REG_SMFLAG (4): SIF_STAT_SIFINIT (0x10000) | SIF_STAT_CMDINIT (0x20000) | SIF_STAT_BOOTEND (0x40000)
                    4 => 0x0007_0000u32,
                    // 0x8000_0000 (SIF cmd init) and 0x8000_0002 (SIF RPC init): non-zero indicates initialized.
                    0x8000_0000 | 0x8000_0002 => 1,
                    _ => 0,
                };
                cpu.set_gpr_sext32(2, val as i32);
                Ok(None)
            }

            // 0x7F: GetMemorySize()
            // Reference: ps2tek #bios & InitTLB caller in lsu.elf (0x0010_8AE8 -> 0x0010_8D60).
            0x7F => {
                cpu.set_gpr_sext32(2, EE_RAM_SIZE as i32);
                Ok(None)
            }

            _ => Err(SyscallError::Unimplemented(num)),
        }
    }

    /// Inspect a `SifSetDma` transfer list and service `SifBindRpc` (`0x8000_0009`) and
    /// `SifCallRpc` (`0x8000_000A`) for `FILEIO` `FIO_F_WRITE` (`fd = 1` / `2` -> EE stdout).
    ///
    /// Verified against disassembly of `alu.elf`:
    /// - `_SifSendCmd` (`0x0011_3600`):
    ///   * When `send_size > 0`, `dmat[0]` is `{ src: sendbuf, dest: server_buf, size: send_size, attr: 0 }`
    ///     and `dmat[1]` is `{ src: packet, dest: ..., size: 64, attr: 0x44 }`.
    ///   * When `send_size == 0`, `dmat[0]` is `{ src: packet, dest: ..., size: 64, attr: 0x44 }`.
    /// - `SifBindRpc` (`0x0011_3AA8`): `packet + 8 = 0x8000_0009`, `packet + 28 = client`, `packet + 32 = sid`.
    ///   Sets `client->server` (`client + 36`) to `sid` (non-zero) and clears `packet + 16` (`paddr` in-use flag).
    /// - `SifCallRpc` (`0x0011_2D48`) from `fioWrite` (`0x0011_25B0`):
    ///   * `packet + 8 = 0x8000_000A`, `packet + 32 = rpc_number` (`3` = `FIO_F_WRITE`),
    ///     `packet + 40 = receive` (`&recvbuf`), `packet + 52 = server` (`0x8000_0001` = `FILEIO`).
    ///   * `dmat[0].src` points to `fio_write_arg { fd: u32, ptr: u32, size: u32, mis: u32, aligned: [u8; 16] }`.
    fn handle_sif_set_dma(&mut self, bus: &mut Bus, dmat_ptr: u32, count: u32) {
        if count == 0 || count > 16 {
            return;
        }

        let cmd_entry_addr = dmat_ptr.wrapping_add((count - 1) * 16);
        let packet_addr = bus.read32(cmd_entry_addr);
        if packet_addr == 0 {
            return;
        }

        let cid = bus.read32(packet_addr.wrapping_add(8));
        // Free the RPC packet slot (`_sif_get_fpacket` at `0x0011_3340` checks bit 0 of `packet + 16`).
        bus.write32(packet_addr.wrapping_add(16), 0);

        match cid {
            SIF_CMD_RPC_BIND => {
                let client = bus.read32(packet_addr.wrapping_add(28));
                let sid = bus.read32(packet_addr.wrapping_add(32));
                if client != 0 {
                    bus.write32(client.wrapping_add(20), 1); // client->buff = non-zero
                    bus.write32(client.wrapping_add(36), sid); // client->server = sid
                }
            }
            SIF_CMD_RPC_CALL => {
                let rpc_number = bus.read32(packet_addr.wrapping_add(32));
                let receive = bus.read32(packet_addr.wrapping_add(40));
                let server = bus.read32(packet_addr.wrapping_add(52));

                if server == SIF_RPC_SERVER_FILEIO && rpc_number == FIO_RPC_WRITE && count >= 2 {
                    let sendbuf = bus.read32(dmat_ptr);
                    let fd = bus.read32(sendbuf);
                    let ptr = bus.read32(sendbuf.wrapping_add(4));
                    let size = bus.read32(sendbuf.wrapping_add(8));

                    if (fd == 1 || fd == 2) && size <= (EE_RAM_SIZE as u32) {
                        for i in 0..size {
                            let b = bus.read8(ptr.wrapping_add(i));
                            bus.stdout_buffer.push(b);
                        }
                    }
                    if receive != 0 {
                        bus.write32(receive, size);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hle_sif_rpc_fio_write_captures_stdout_and_rejects_unimplemented_syscalls() {
        let mut cpu = EeCpuState::new();
        let mut bus = Bus::new();
        let mut hle = EeHleState::new();

        // Write "Hi\n" at 0x0010_2000
        bus.write8(0x0010_2000, b'H');
        bus.write8(0x0010_2001, b'i');
        bus.write8(0x0010_2002, b'\n');

        // Build `fio_write_arg` at 0x0010_2100: { fd = 1, ptr = 0x0010_2000, size = 3, mis = 0 }
        bus.write32(0x0010_2100, 1);
        bus.write32(0x0010_2104, 0x0010_2000);
        bus.write32(0x0010_2108, 3);
        bus.write32(0x0010_210C, 0);

        // Build `SifCmdRpcCall` packet at 0x0010_2200:
        // +8: cid = 0x8000_000A, +16: paddr = 1 (in-use), +32: rpc_number = 3 (FIO_F_WRITE),
        // +40: receive = 0x0010_2300, +52: server = 0x8000_0001 (FILEIO)
        bus.write32(0x0010_2208, SIF_CMD_RPC_CALL);
        bus.write32(0x0010_2210, 1);
        bus.write32(0x0010_2220, FIO_RPC_WRITE);
        bus.write32(0x0010_2228, 0x0010_2300);
        bus.write32(0x0010_2234, SIF_RPC_SERVER_FILEIO);

        // Build 2-entry SifDmaTransfer table at 0x0010_2400:
        // dmat[0].src = 0x0010_2100 (sendbuf), dmat[1].src = 0x0010_2200 (packet)
        bus.write32(0x0010_2400, 0x0010_2100);
        bus.write32(0x0010_2410, 0x0010_2200);

        // Invoke SifSetDma ($v1 = 0x77, $a0 = 0x0010_2400, $a1 = 2)
        cpu.set_gpr_sext32(3, 0x77);
        cpu.set_gpr_sext32(4, 0x0010_2400);
        cpu.set_gpr_sext32(5, 2);
        assert_eq!(
            hle.handle_syscall(&mut cpu, &mut bus, 0x0010_0008),
            Ok(None)
        );
        assert_eq!(&bus.stdout_buffer, b"Hi\n");
        assert_eq!(bus.read32(0x0010_2300), 3);
        assert_eq!(bus.read32(0x0010_2210), 0);

        // Invoke an unimplemented syscall ($v1 = 0x7E) -> returns Err(SyscallError::Unimplemented(0x7E))
        cpu.set_gpr_sext32(3, 0x7E);
        assert_eq!(
            hle.handle_syscall(&mut cpu, &mut bus, 0x0010_0008),
            Err(SyscallError::Unimplemented(0x7E))
        );
    }
}
