# PS2 Hardware Quirks & Verification Notes (`docs/HARDWARE_NOTES.md`)

This document records non-obvious PlayStation 2 hardware behaviors encountered during Phase 1 implementation, the exact `ps2autotests` suite and line numbers that verify each behavior, and whether the implementation is derived from a primary source (`ps2tek`, `Play!`, or `ps2autotests` test code) or inferred from test vectors only (`"fits test vectors; may not generalize"`).

---

## 1. R5900 Base Scalar ISA & Load/Store (`ee/interpreter.rs`)

### 1.1 32-Bit Scalar Shifts Sign-Extend Bit 31 Even When `shamt == 0`
- **Behavior**: All 32-bit scalar ALU and shift instructions (`ADD`, `ADDU`, `SUB`, `SUBU`, `ADDI`, `ADDIU`, `LUI`, `SLL`, `SRL`, `SRA`, `SLLV`, `SRLV`, `SRAV`) sign-extend bit 31 of the 32-bit result into bits `32..=63` of `GPR[rd]` while preserving bits `64..=127`. For variable shifts (`SRLV`, `SLLV`, `SRAV`) with `(GPR[rs] & 0x1F) == 0`, bit 31 of `GPR[rt]` is still sign-extended into bits `32..=63` (e.g., `SRLV` of `0x0000_0000_FFFF_8000` by `0` produces `0xFFFF_FFFF_FFFF_8000`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/alu.expected` line 713 (`srlv C_S16_MIN, C_S16_MIN: 0000133a 00001339 ffffffff ffff8000`) and lines 8–20.
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` & `ps2autotests` `cpu/ee/alu.cpp`).

### 1.2 Integer Division by Zero and `i32::MIN / -1` (`DIV`, `DIVU`, `DIV1`, `DIVU1`, `PDIVW`, `PDIVUW`, `PDIVBW`)
- **Behavior**:
  - Signed 32-bit division by `0` sets `HI = sext32(dividend)` and `LO = 1` (if `dividend < 0`) or `-1` (if `dividend >= 0`).
  - Signed `i32::MIN / -1` (`0x8000_0000 / -1`) sets `HI = 0` and `LO = sext32(i32::MIN)` (`0xFFFF_FFFF_8000_0000`) without trapping.
  - Unsigned 32-bit division by `0` sets `HI = sext32(dividend)` and `LO = -1` (`0xFFFF_FFFF_FFFF_FFFF`).
  - Scalar `MULT`/`MULTU`/`DIV`/`DIVU` modify only `LO0`/`HI0` (`bits 0..=63`) and preserve `LO1`/`HI1` (`bits 64..=127`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 6, 16, 18, 46, and `local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 2–107.
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` & `ps2autotests` `cpu/ee/muldiv.cpp`).

### 1.3 Unaligned Load Word Right (`LWR`) Conditional Sign-Extension
- **Behavior**: `LWR rt, offset(base)` sign-extends bit 31 into bits `32..=63` of `GPR[rt]` **only** when `(vaddr & 3) == 0` (when all 4 bytes of the 32-bit word are loaded). When `(vaddr & 3) != 0` (`1`, `2`, or `3`), `LWR` replaces only the lowest `4 - (vaddr & 3)` bytes of `GPR[rt]` and leaves bits `24..=63` unmodified.
- **Evidence**: `local/ps2autotests/tests/cpu/ee/lsu.expected` lines 76–80.
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` & `ps2autotests` `cpu/ee/lsu.cpp`).

### 1.4 Branch Instructions Inside a Branch Delay Slot (`BEQ`/Scalar vs `BC1F`/`BC1T`/`BC1FL`/`BC1TL`)
- **Behavior**:
  - When a taken scalar branch/jump (`BEQ`, `J`, `JALR`, etc.) or a taken `COP1` Branch Likely (`BC1FL`, `BC1TL`) executes inside the delay slot of an already-taken branch, it overrides the pending branch target (`next_pc`), jumping to the inner branch's target immediately after the delay slot instruction (`Branch To Second Target - Work Inside Delay Slot`).
  - In `cpu/ee_fpu/branchdelay.elf` (`test_bc1t`), two back-to-back non-likely `BC1T` instructions following `C.EQ.S` execute the first branch's target instruction (`target1`, which sets `$t0 = 1`) as the delay slot of the second `BC1T` before jumping to the second `BC1T`'s target (`target4`, printing `bc1t -> bc1t: Branch To Second Target - Work Inside Delay Slot`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/branchdelay.expected` lines 1–20 and `local/ps2autotests/tests/cpu/ee_fpu/branchdelay.expected` lines 1–5.
- **Source Classification**:
  - Scalar and `BC1FL`/`BC1TL` delay-slot target override: **Primary source** (`ps2autotests` `cpu/ee/branchdelay.cpp` & `cpu/ee_fpu/branchdelay.cpp`).
  - Non-likely `BC1F`/`BC1T` delay-slot chaining (`StepOutcome::Cop1BranchTaken`): **Inferred from test vectors (`fits test vectors; may not generalize`)** — on real R5900 silicon this may alternatively be caused by a 2-instruction `C.<cond>.S -> BC1` pipeline hazard on `FCR31.C`.

---

## 2. R5900 `COP0` System Control & Performance Counters (`ee/cop0.rs`, `ee/interpreter.rs`)

### 2.1 `COP0.Perf` (`Reg 25`) Specifier Decoding (`MFPS`, `MTPS`, `MFPC`, `MTPC`) and `PCCR` Mask
- **Behavior**:
  - `MTPS rt, reg` (`opcode=0x10, rs=0x04, rd=25, bit 0=0, reg=(instr >> 1) & 0x1F`) writes `PCCR = GPR[rt] & 0x800F_FBFE` **only when `reg == 0`**; any non-zero `reg` (`1..=31`) is ignored.
  - `MFPS rt, reg` (`opcode=0x10, rs=0x00, rd=25, bit 0=0`) returns `sext32(PCCR)` for **all** `reg` values (`0..=31`).
  - `MTPC` / `MFPC` (`bit 0=1`) select `PCR0` for **even** `reg` (`(reg & 1) == 0`) and `PCR1` for **odd** `reg` (`(reg & 1) == 1`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_cop0/performance.cpp` & `performance.expected` lines 1–24.
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/ee_cop0/performance.cpp`).

---

## 3. R5900 `COP1` FPU & `ps2float` (`ps2-common/src/ps2float.rs`, `ee/cop1.rs`)

### 3.1 `ADD.S` / `SUB.S` Single Guard Bit and Absence of Sticky Bit
- **Behavior**: R5900 `ADD.S` and `SUB.S` align mantissas using a single guard bit (`25`-bit intermediate significand) with no round or sticky bit, and round toward zero after normalization. When `exp_diff >= 25`, the smaller operand shifts completely out to zero (e.g., `1.0 - 2^-26 = 1.0` `0x3F80_0000` instead of IEEE-754 `0x3F7F_FFFF`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/arithmetic.expected` lines 1–160.
- **Source Classification**: **Primary source** (`Play!` BSD-2 `Source/ee/FpAddTruncate.cpp` & `ps2autotests` `cpu/ee_fpu/arithmetic.cpp`).

### 3.2 `MUL.S` / `MULA.S` Radix-4 Booth Multiplier Low-Group Truncation
- **Behavior**: When the second multiplier operand `ft` (`t`) has significand bits `sig2[1:0] >= 2` (which encodes a negative Radix-4 Booth digit `-2` or `-1` in the lowest 2-bit Booth group), the R5900 multiplier omits the `+1` two's-complement hot-one of that lowest partial product (`prod = sig1 * sig2 - sig1` before shifting right by `23 + carry`). Consequently, multiplication is not commutative in the lowest bit for operands with `sig[1:0] >= 2`: `MUL.S(1.0, 0x3FFF_FFFF) = 0x3FFF_FFFE`, whereas `MUL.S(0x3FFF_FFFF, 1.0) = 0x3FFF_FFFF`.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 308, 311, 314–317.
- **Source Classification**: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.3 `MADD.S` / `MADDA.S` / `MSUB.S` / `MSUBA.S` Product-Overflow Immediate Saturation
- **Behavior**: When the intermediate product `fs * ft` in a multiply-accumulate instruction overflows biased exponent `255` (`0xFF`), the hardware MAC unit immediately saturates the entire operation result to `±0x7FFF_FFFF` (with the sign of `±(fs * ft)`) and sets `FCR31.O | FCR31.SO`, without adding or subtracting `ACC` (e.g., `MADD.S(acc = -MAX, fs = +MAX, ft = +MAX) = +MAX = 0x7FFF_FFFF`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 57, 60, 63, 118, 121, 124, 178, 181, 191, 193, 204.
- **Source Classification**: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.4 `DIV.S`, `SQRT.S`, and `RSQRT.S` Rounding and Exception Flags
- **Behavior**:
  - `DIV.S` and `SQRT.S` only update `I` (Invalid) and `D` (Divide-by-zero) cause/sticky flags in `FCR31`; they never set `O` (Overflow) or `U` (Underflow) even when the quotient saturates to `±0x7FFF_FFFF` or flushes to `±0.0`.
  - `SQRT.S(-0.0)` and `SQRT.S(-denormal)` return `+0.0` (`0x0000_0000`, clearing bit 31) while setting `FCR31.I | FCR31.SI`.
  - `DIV.S` and `SQRT.S` compute a 25-bit significand (`1` guard bit) and round via `(q25 + 1) >> 1`.
  - `RSQRT.S(fs, ft)` computes a 24-bit rounded reciprocal significand `1 / sqrt(ft)` and multiplies `fs` by that 24-bit reciprocal before truncating to 24 bits.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` line 48, `muldiv.expected` line 29, and `sqrt.expected` lines 10–65.
- **Source Classification**:
  - `DIV.S`/`SQRT.S` flag behavior and `SQRT.S(-0.0) -> +0.0`: **Primary source** (`ps2tek` `#eefpu` & `ps2autotests` `fcr.cpp` / `sqrt.cpp`).
  - Exact 25-bit guard-bit rounding formula `(x25 + 1) >> 1` for `DIV.S`/`SQRT.S`/`RSQRT.S`: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.5 `FCR0` / `FCR31` Control Register Mirroring, Masking, and Cause Flag Clearing
- **Behavior**:
  - `CFC1 rt, fs` reads `FCR0` (`0x0000_2E30`) for `fs = 0..=15` and `FCR31` for `fs = 16..=31`.
  - `CTC1 rt, fs` modifies `FCR31` only when `fs == 31`, applying writable mask `0x0083_C078` and hardwired bits `0x0100_0001`; writes with `fs != 31` are ignored.
  - `ABS.S`, `NEG.S`, `MAX.S`, and `MIN.S` clear the `I, D, O, U` cause bits in `FCR31` without flushing denormal operands; `MOV.S`, `CVT.W.S`, `CVT.S.W`, and `C.<cond>.S` leave `I, D, O, U` unchanged.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` lines 3–29, `transfer.expected` line 20, and `arithmetic.expected`.
- **Source Classification**: **Primary source** (`ps2tek` `#eefpu` & `ps2autotests` `cpu/ee_fpu/{fcr,transfer,arithmetic}.cpp`).

---

## 4. R5900 Multimedia Instructions (`ee/mmi.rs`)

### 4.1 `PHMSBH` Upper-Word Bitwise Inversion in `LO` / `HI`
- **Behavior**: `PHMSBH rd, rs, rt` (`MMI2` `shamt = 0x15`) computes `r[i] = p[2*i + 1] - p[2*i]` for each pair of 16-bit signed products (`i = 0..3`). In `LO` and `HI`, the lower 32-bit word of each 64-bit half receives `r[i]`, while the upper 32-bit word receives the bitwise NOT `!(p[2*i + 1] as u32)` (whereas `PHMADH` stores `p[2*i + 1] as u32` uninverted).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 145–171 (`phmsbh` followed by `pmfhl.uw`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/ee_simd/muldiv.cpp` & `muldiv.expected`).

---

## 5. EE 10-Channel DMAC & Scratchpad DMA (`bus/dmac.rs`)

### 5.1 `D8_SADR` / `D9_SADR` Masking (`0x3FF0`) and `D8_MADR` / `D9_MADR` Bit-31 Masking
- **Behavior**:
  - `SADR` on `fromSPR` (`D8`) and `toSPR` (`D9`) is masked with `0x3FF0` on register writes and wraps modulo `0x4000` (`(sadr + 16) & 0x3FF0`) during transfers.
  - `MADR` on `fromSPR` (`D8`) and `toSPR` (`D9`) clears bit 31 (`0x8000_0000`, the Scratchpad selection bit valid on other channels) as well as bits `0..=3` (`val & 0x7FFF_FFF0`).
- **Evidence**: `local/ps2autotests/tests/dma/spr/normal.expected` lines 2–36 and `local/ps2autotests/tests/dma/spr/chain.expected` lines 2–25.
- **Source Classification**: **Primary source** (`ps2tek` `#dmacchannels` & `ps2autotests` `dma/spr/{normal,chain}.cpp`).

### 5.2 `D_SQWC` 8-Bit Field Widths (`SQWC` / `TQWC`) and Interleave `MADR` Advancement
- **Behavior**:
  - In `D_SQWC` (`0x1000_E030`), `SQWC` (`bits 0..=7`) and `TQWC` (`bits 16..=23`) are 8-bit fields (`& 0xFF`).
  - In Interleave Mode (`CHCR.MOD == 2`), each slice transfers `TQWC` quadwords (advancing both `MADR` and `SADR` by `TQWC * 16` and decrementing `QWC` by `TQWC`) and then skips `SQWC` quadwords on `MADR` (`MADR += SQWC * 16`), including after the final slice when `QWC` reaches `0`.
- **Evidence**: `local/ps2autotests/tests/dma/spr/interleave.expected` lines 40–628 (`skip = 256` -> `0`, `skip = 257` -> `1`, `transfer = 257` -> `1`, and `MADR` delta warnings).
- **Source Classification**: **Primary source** (`ps2tek` `#dmacio` & `ps2autotests` `dma/spr/interleave.cpp`).

### 5.3 Source Chain `TADR` Update on `END` / `RET` vs `REFE` / `REF` and `CHCR.TIE` Tag Interrupts
- **Behavior**:
  - In Source Chain Mode (`CHCR.MOD == 1`), `REFE` (`ID = 0`) and `REF` (`ID = 3`) set `MADR = tag.ADDR` and advance `TADR += 16`, whereas `END` (`ID = 7`) and `RET` (`ID = 6` when `CHCR.ASP == 0`) set `MADR = TADR + 16` and leave `TADR` pointing at the `END`/`RET` tag.
  - When `CHCR.TIE == 1` (bit 7) and the active DMATag has `IRQ == 1` (bit 31 of low word), the channel terminates (`CHCR.STR = 0`, `D_STAT.CISn = 1`) immediately after transferring the tag's `QWC` payload.
- **Evidence**: `local/ps2autotests/tests/dma/dmac/tagintr.expected` lines 2–9 and `local/ps2autotests/tests/dma/spr/chain.expected` lines 4, 9, 15.
- **Source Classification**: **Primary source** (`ps2tek` `#dmactags` & `ps2autotests` `dma/dmac/tagintr.cpp`).

### 5.4 DMA Transfer Data Movement at Start with Cycle-Driven Deferred Completion
- **Behavior**:
  - When a channel transfer is triggered (`CHCR.STR = 1` in `EeDmac::start_channel_transfer`), the payload data copy (`RDRAM <-> Scratchpad`) and channel pointer/counter updates (`MADR`, `SADR`, `TADR`, `QWC`, `CHCR.TAG`, `CHCR.ASP`) occur immediately at transfer start, while `CHCR.STR` stays `1` and `D_STAT.CIS[ch]` stays `0`.
  - A `HardwareEventKind::DmacChannelComplete(ch)` event is scheduled on `bus.scheduler` for `((qwc_transferred + tags_read).max(1) * 2)` EE cycles in the future (`1 BUSCLK = 2 EE cycles` per QWC/tag, `ps2tek` `#dmac`).
  - `bus.scheduler` is advanced strictly by executed EE CPU cycles (`EeCpuState::step` and `step_hle` call `bus.step_timers(1)` after every executed EE instruction, which calls `self.scheduler.advance(1)` and pops due events). Polling `CHCR` (`EeDmac::read32(&self, paddr)`) is a pure read and never skips time forward.
  - When `DmacChannelComplete(ch)` becomes due and is popped in `bus.step_timers`, `EeDmac::complete_channel(ch)` clears `CHCR.STR` to `0` and sets `D_STAT.CIS[ch]` to `1`.
- **Evidence**: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#dmac`), `local/ps2autotests/tests/dma/dmasend.cpp` lines 31–34 & 60–63 (`while (--i > 0 && chan->chcr.Ongoing())`), and `dma/dmac/cpcond.expected` / `tagintr.expected`.
- **Source Classification**: **Primary source** for `1 QWC / BUSCLK` (`2 EE cycles / QWC`) rate (`ps2tek` `#dmac`); **Phase 1 architectural simplification** for moving the payload bytes upfront at transfer start while deferring `CHCR.STR = 0` and `D_STAT.CIS[ch] = 1` to the cycle-accurate completion event.

---

## 6. R3000A IOP CPU, Load Delay Slot, and `.irx` Relocations (`crates/ps2-core/src/iop/`)

### 6.1 R3000A 1-Instruction Load Delay Slot & Delay-Slot Overwrite Cancellation
- **Behavior**:
  - Unlike the R5900 EE (which has hardware interlocks on loads), the R3000A IOP (`MIPS I`) has a 1-instruction architectural load delay slot for `LB`, `LBU`, `LH`, `LHU`, `LW`, `LWL`, `LWR`, and `MFC0`.
  - The instruction immediately following a load (`N + 1`, the load delay slot) reads the **old** value of the target register `rt`.
  - If the load delay slot instruction `N + 1` itself writes to `rt` (either via an ALU/link write such as `ori $t0, $0, 0` or by queuing a new load targeting `rt`), the pending load from instruction `N` is **cancelled** so that `N + 1`'s write wins.
  - When `LWL` and `LWR` are executed back-to-back targeting the same register `rt`, the second instruction merges against the in-flight `PendingLoad` value from the first instruction rather than the stale architectural register value.
- **Evidence**: `local/ps2autotests/tests/cpu/iop/lsudelay.c` and `lsudelay.expected` lines 2–10 (`lb 0: 00000000`, `lw 0: 00000000`, `lb 1: 11223344`, `lw 1: 11223344`, `ld 1: 13371337 00000000`, `lw then branch: 2`), and `local/ps2autotests/tests/cpu/iop/lsu.expected` lines 56–68.
- **Source Classification**: **Primary source** (MIPS I R3000A architecture & `ps2autotests` `cpu/iop/lsudelay.c`).

### 6.2 R3000A Branch-and-Link Register Write Order vs. Branch Delay Slot
- **Behavior**:
  - `JAL`, `JALR`, `BLTZAL`, and `BGEZAL` write the link register (`$ra` or `rd`) during the branch instruction **before** the branch delay slot instruction executes, so if the delay slot modifies the link register (`li $ra, 2`), the delay slot's write overwrites the return address.
  - For `JALR rd, rs` where `rd == rs`, `rs` is read as the branch target before `rd` is overwritten with `inst_pc + 8`.
  - `BLTZAL` (`REGIMM` `rt = 0x10`) and `BGEZAL` (`REGIMM` `rt = 0x11`) write `$ra = inst_pc + 8` unconditionally, even when the branch condition is false.
- **Evidence**: `local/ps2autotests/tests/cpu/iop/branchdelay.expected` lines 2–10 and `local/ps2autotests/tests/cpu/iop/branch.expected` lines 57–83 & 144–170.
- **Source Classification**: **Primary source** (MIPS I R3000A architecture & `ps2autotests` `cpu/iop/{branch,branchdelay}.c`).

### 6.3 R3000A `DIV` / `DIVU` Corner Cases and Non-Stalling `MFHI` / `MFLO`
- **Behavior**:
  - Signed `DIV` by zero (`rt == 0`): `HI = rs`; `LO = 0xFFFF_FFFF` (`-1`) if `(rs as i32) >= 0`, else `0x0000_0001` (`+1`).
  - Signed `DIV` overflow (`0x8000_0000 / -1`): `HI = 0x0000_0000`, `LO = 0x8000_0000`.
  - Unsigned `DIVU` by zero (`rt == 0`): `HI = rs`, `LO = 0xFFFF_FFFF`.
  - `MFHI` and `MFLO` have no architectural delay slot (`hilodelay.expected` lines 2–5).
- **Evidence**: `local/ps2autotests/tests/cpu/iop/muldiv.expected` lines 2–58 and `local/ps2autotests/tests/cpu/iop/hilodelay.expected` lines 2–5.
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/{muldiv,hilodelay}.c`).

### 6.4 IOP `.irx` (`ET_SCE_IOPRELEXEC = 0xFF80`) Pre-Linked Addends and `.iop.import` Stubs
- **Behavior**:
  - In IOP `.irx` binaries (`e_type = 0xFF80`), the `PT_LOAD` segment is linked at virtual address `0x0000_0000` with internal symbol values already encoded into the instruction/data words at `r_offset`; relocating the module to `base_addr` (`0x0001_0000`) adds `base_addr` to each `SHT_REL` target (`R_MIPS_32`, `R_MIPS_26`, and paired `R_MIPS_HI16` / `R_MIPS_LO16`).
  - Import stub tables in `.text` start with a 20-byte header (`0x41E0_0000`, `0`, `version`, 8-byte module name) followed by 8-byte function stubs (`0x03E0_0008` `jr $ra`; `0x2400_xxxx` `addiu $zero, $zero, import_index`).
- **Evidence**: `local/ps2autotests/tests/cpu/iop/*.irx` ELF headers, `SHT_REL` sections, and `.iop.import` tables.
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/*.irx` binaries).

---

## 7. LLE BIOS Boot Register Stubs & IOP Cache Isolation (`Task 6b`)

### 7.1 EE DMAC `0x1000_F500` Undocumented Control Latch (`EeDmac::d_f500`)
- **Behavior**:
  - Backed by a 32-bit R/W latch (`EeDmac::d_f500`) at physical address `0x1000_F500` in the `0x1000_F5xx` DMAC disable/enable register block (`D_ENABLEW` `0x1000_F520`, `D_ENABLER` `0x1000_F590`).
- **Evidence**: Hit on Iteration 1 of coupled LLE BIOS boot (`EE PC = 0xBFC0_0838`, `sw $v0, 0($v1)` with `$v1 = 0xB000_F500`, `val = 0x0000_0001`).
- **Source Classification**: `TODO(verify)` (`ps2tek` `#dmacio` documents `0x1000_F520` and `0x1000_F590` in the `0x1000_F5xx` block, but omits `0x1000_F500`).

### 7.2 EE Serial I/O (`EeSio`, `0x1000_F100..=0x1000_F1C0`)
- **Behavior**:
  - Implements the Emotion Engine UART control/status registers (`0x1000_F100` `SIO_LCR`, `0x1000_F110` `SIO_LSR`, `0x1000_F120` `SIO_IER`, `0x1000_F130` `SIO_ISR` with write-1-to-clear on bits `0..=2`, `0x1000_F140` `SIO_FCR`, `0x1000_F150` `SIO_BGR`, `0x1000_F180` `SIO_TXFIFO` / `KPUTCHAR` stdout capture, and `0x1000_F1C0` `SIO_RXFIFO`).
- **Evidence**: Hit on Iteration 2 of LLE BIOS boot (`EE PC = 0x9FC4_2F9C`, `paddr = 0x1000_F100`, `val = 0x0000_0020`), immediately preceding EE serial console initialization and output.
- **Source Classification**: **Primary source** (`ps2tek` `#iomaps` line 212 for `0x1000_F180` `KPUTCHAR`; `ps2sdk` `ee/kernel/include/sio.h` for `0x1000_F100..=0x1000_F1C0`).

### 7.3 IOP Mode & Bootstrap Register (`0x1F80_1450`, `GM_IF`)
- **Behavior**:
  - Backed by `Bus::iop_gm_if: u32` (initialized to `0` on reset = PS2 R3000A mode). Early IOP BIOS reset code (`IOP PC = 0xBFC0_2040`) reads `0xBF80_1450` and tests bit 3 (`andi $t0, $t0, 0x0008`) to branch between native PS2 IOP initialization and PS1 backwards-compatibility mode.
- **Evidence**: Hit on Iteration 3 of LLE BIOS boot (`IOP PC = 0xBFC0_2040`, `lw $t0, 5200($t0)` at `paddr = 0x1F80_1450`).
- **Source Classification**: `TODO(verify)` (`ps2sdk` `common/include/iop_regs.h` names `0x1F80_1450` `GM_IF`; `ps2tek` omits `0x1F80_1450`).

### 7.4 IOP Sub-System Bus Controller (`IopSsbusc`, `0x1F80_1000..=0x1F80_1023`, `0x1F80_1060`, `0x1F80_1400..=0x1F80_1423`)
- **Behavior**:
  - Backed by `IopSsbusc` (`memctrl1: [u32; 9]` at `0x1F80_1000..=0x1F80_1023`, `ram_size: u32` at `0x1F80_1060`, and `ssbus_ext: [u32; 9]` at `0x1F80_1400..=0x1F80_1423`).
- **Evidence**: Hit on Iteration 4 of LLE BIOS boot (`IOP PC = 0xBFC0_20C4`, `paddr = 0x1F80_1010`) where the IOP BIOS iterates through an initialization table configuring SSBUSC memory/device bus delays and base addresses.
- **Source Classification**: `TODO(verify)` (`ps2sdk` `iop/system/ssbusc` and `psx-spx` Memory Control 1/2 document these ranges; `ps2tek` omits the register bitfields).

### 7.5 IOP Expansion Region 2 `POST2` Diagnostic Latch (`0x1F80_2070`)
- **Behavior**:
  - Backed by `Bus::iop_post2: u32` at `0x1F80_2070` (`HW_POST2` power-on self-test diagnostic status latch).
- **Evidence**: Hit on Iteration 5 of LLE BIOS boot (`IOP PC = 0xBFC0_20E8`, `paddr = 0x1F80_2070`, `val = 0x0000_00FC`).
- **Source Classification**: `TODO(verify)` (`ps2tek` omits `0x1F80_2070`).

### 7.6 IOP DMA Priority & Interrupt Control (`IopDmacControl`, `0x1F80_10F0..=0x1F80_10F4`, `0x1F80_1570..=0x1F80_157C`)
- **Behavior**:
  - Implements `DPCR` (`0x1F80_10F0`, reset `0x0777_7777`), `DICR` (`0x1F80_10F4`, bits `24..=30` write-1-to-clear, bit `31` computed as `DICR.15 | (DMACINTEN.0 && DICR.23 && (DICR.24..30 != 0 || DICR2.24..29 != 0))`), `DPCR2` (`0x1F80_1570`), `DICR2` (`0x1F80_1574`, bits `24..=29` write-1-to-clear), `DMACEN` (`0x1F80_1578`, bit `0`), and `DMACINTEN` (`0x1F80_157C`, bits `0..=1`).
- **Evidence**: Hit on Iteration 6 of LLE BIOS boot (`IOP PC = 0xBFC0_20E8`, `paddr = 0x1F80_10F0`, `val = 0x0777_7777`).
- **Source Classification**: **Primary source** (`ps2tek` `https://psi-rockin.github.io/ps2tek/#iopdma`, `local/ps2tek/index.html` lines 4051–4118).

### 7.7 EE RDRAM Memory Controller Hub (`RdramMch`, `0x1000_F400..=0x1000_F4F0`)
- **Behavior**:
  - Extends `RdramMch` beyond `0x1000_F430` (`MCH_RICM`) and `0x1000_F440` (`MCH_DRD`) with a 16-word `regs: [u32; 16]` array covering the remaining 16-byte-aligned MCH control/timing slots in `0x1000_F400..=0x1000_F4F0` (such as `0x1000_F410`).
- **Evidence**: Hit on Iteration 7 of LLE BIOS boot (`EE PC = 0x9FC4_11B4` inside the EE memory initialization routine, `sw $zero, 0($v1)` at `paddr = 0x1000_F410`).
- **Source Classification**: **Primary source** for `0x1000_F430` and `0x1000_F440` (`ps2tek` `#eerdram` lines 700–754); `TODO(verify)` for the remaining 16-byte-aligned slots in `0x1000_F400..=0x1000_F4F0`.

### 7.8 IOP `COP0.SR.IsC` Cache Isolation & `KSEG2` Cache Control (`0xFFFE_0130..=0xFFFE_014F`)
- **Behavior**:
  - When `COP0.SR` bit 16 (`IsC = 0x0001_0000`, Isolate Cache) is set on the R3000A IOP, data loads and stores (`LB`, `LBU`, `LH`, `LHU`, `LW`, `LWL`, `LWR`, `SB`, `SH`, `SW`, `SWL`, `SWR`) target the isolated cache lines/tags and do not read or overwrite `Bus::iop_ram`. Without `IsC` isolation, the IOP BIOS `FlushIcache()` / `FlushDcache()` routines overwrite the first 4 KB of IOP RAM (`0x0000_0000..=0x0000_0FFF`) with zeros during boot.
  - `Bus::iop_translate_addr` preserves `KSEG2` addresses (`vaddr >= 0xC000_0000`) instead of masking with `0x1FFF_FFFF` (which would otherwise alias `0xFFFE_0130` into BIOS ROM at `0x1FFE_0130`), and maps `0xFFFE_0130..=0xFFFE_014F` to `Bus::iop_cache_ctrl: [u32; 8]`.
- **Evidence**: Hit on Iteration 8 of LLE BIOS boot (`IOP PC = 0x0000_0078` prior to `IsC` isolation, and `IOP PC = 0xBFC0_20E8` writing `0xFFFE_0130` / `0xFFFE_0140` / `0xFFFE_0144`). With both in place, the IOP BIOS boots past `145,058` IOP cycles (`1,160,464` EE cycles) into loaded IOP kernel initialization (`PC = 0x0000_3D88`).
- **Source Classification**: **Primary source** for R3000A `COP0.SR.IsC` cache isolation and `FFFE0000h (KSEG2) Cache control` (`ps2tek` `#memorymap` line 106 & `#iop` line 5326); `TODO(verify)` for individual bitfields of `0xFFFE_0130..=0xFFFE_014F`.

