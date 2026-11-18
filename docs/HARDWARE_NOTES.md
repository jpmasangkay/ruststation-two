# PS2 Hardware Quirks & Verification Notes (`docs/HARDWARE_NOTES.md`)

This document records non-obvious PlayStation 2 hardware behaviors encountered during Phase 1 implementation, the exact `ps2autotests` suite and line numbers that verify each behavior, and whether the implementation is derived from a primary source (`ps2tek`, `ps2sdk`, `Play!`, or `ps2autotests` test code) or inferred from test vectors only (`"fits test vectors; may not generalize"`).

Every citation is labeled with its verification status from this session:
- **`FETCHED`**: The exact document page or source file was opened and read directly during this session.
- **`SEARCH-ONLY`**: Seen only in search results or from memory without opening the primary document in full.

---

## 1. R5900 Base Scalar ISA & Load/Store (`ee/interpreter.rs`)

### 1.1 32-Bit Scalar Shifts Sign-Extend Bit 31 Even When `shamt == 0`

- **Behavior**: All 32-bit scalar ALU and shift instructions (`ADD`, `ADDU`, `SUB`, `SUBU`, `ADDI`, `ADDIU`, `LUI`, `SLL`, `SRL`, `SRA`, `SLLV`, `SRLV`, `SRAV`) sign-extend bit 31 of the 32-bit result into bits `32..=63` of `GPR[rd]` while preserving bits `64..=127`. For variable shifts (`SRLV`, `SLLV`, `SRAV`) with `(GPR[rs] & 0x1F) == 0`, bit 31 of `GPR[rt]` is still sign-extended into bits `32..=63` (e.g., `SRLV` of `0x0000_0000_FFFF_8000` by `0` produces `0xFFFF_FFFF_FFFF_8000`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/alu.expected` line 713 (`srlv C_S16_MIN, C_S16_MIN: 0000133a 00001339 ffffffff ffff8000`, `FETCHED`) and lines 8–20 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `cpu/ee/alu.cpp` [`FETCHED`]).

### 1.2 Integer Division by Zero and `i32::MIN / -1` (`DIV`, `DIVU`, `DIV1`, `DIVU1`, `PDIVW`, `PDIVUW`, `PDIVBW`)

- **Behavior**:
  - Signed 32-bit division by `0` sets `HI = sext32(dividend)` and `LO = 1` (if `dividend < 0`) or `-1` (if `dividend >= 0`).
  - Signed `i32::MIN / -1` (`0x8000_0000 / -1`) sets `HI = 0` and `LO = sext32(i32::MIN)` (`0xFFFF_FFFF_8000_0000`) without trapping.
  - Unsigned 32-bit division by `0` sets `HI = sext32(dividend)` and `LO = -1` (`0xFFFF_FFFF_FFFF_FFFF`).
  - Scalar `MULT`/`MULTU`/`DIV`/`DIVU` modify only `LO0`/`HI0` (`bits 0..=63`) and preserve `LO1`/`HI1` (`bits 64..=127`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/muldiv.expected` lines 6, 16, 18, 46 (`FETCHED`), and `local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 2–107 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `cpu/ee/muldiv.cpp` [`FETCHED`]).

### 1.3 Unaligned Load Word Right (`LWR`) Conditional Sign-Extension

- **Behavior**: `LWR rt, offset(base)` sign-extends bit 31 into bits `32..=63` of `GPR[rt]` **only** when `(vaddr & 3) == 0` (when all 4 bytes of the 32-bit word are loaded). When `(vaddr & 3) != 0` (`1`, `2`, or `3`), `LWR` replaces only the lowest `4 - (vaddr & 3)` bytes of `GPR[rt]` and leaves bits `24..=63` unmodified.
- **Evidence**: `local/ps2autotests/tests/cpu/ee/lsu.expected` lines 76–80 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#eeinstructions` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `cpu/ee/lsu.cpp` [`FETCHED`]).

### 1.4 Branch Instructions Inside a Branch Delay Slot (`BEQ`/Scalar vs `BC1F`/`BC1T`/`BC1FL`/`BC1TL`)

- **Behavior**:
  - When a taken scalar branch/jump (`BEQ`, `J`, `JALR`, etc.) or a taken `COP1` Branch Likely (`BC1FL`, `BC1TL`) executes inside the delay slot of an already-taken branch, it overrides the pending branch target (`next_pc`), jumping to the inner branch's target immediately after the delay slot instruction (`Branch To Second Target - Work Inside Delay Slot`).
  - In `cpu/ee_fpu/branchdelay.elf` (`test_bc1t`), two back-to-back non-likely `BC1T` instructions following `C.EQ.S` execute the first branch's target instruction (`target1`, which sets `$t0 = 1`) as the delay slot of the second `BC1T` before jumping to the second `BC1T`'s target (`target4`, printing `bc1t -> bc1t: Branch To Second Target - Work Inside Delay Slot`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee/branchdelay.expected` lines 1–20 (`FETCHED`) and `local/ps2autotests/tests/cpu/ee_fpu/branchdelay.expected` lines 1–5 (`FETCHED`).
- **Source Classification**:
  - Scalar and `BC1FL`/`BC1TL` delay-slot target override: **Primary source** (`ps2autotests` `cpu/ee/branchdelay.cpp` & `cpu/ee_fpu/branchdelay.cpp`, `FETCHED`).
  - Non-likely `BC1F`/`BC1T` delay-slot chaining (`StepOutcome::Cop1BranchTaken`): **Inferred from test vectors (`fits test vectors; may not generalize`)** — on real R5900 silicon this may alternatively be caused by a 2-instruction `C.<cond>.S -> BC1` pipeline hazard on `FCR31.C`.

---

## 2. R5900 `COP0` System Control & Performance Counters (`ee/cop0.rs`, `ee/interpreter.rs`)

### 2.1 `COP0.Perf` (`Reg 25`) Specifier Decoding (`MFPS`, `MTPS`, `MFPC`, `MTPC`) and `PCCR` Mask

- **Behavior**:
  - `MTPS rt, reg` (`opcode=0x10, rs=0x04, rd=25, bit 0=0, reg=(instr >> 1) & 0x1F`) writes `PCCR = GPR[rt] & 0x800F_FBFE` **only when `reg == 0`**; any non-zero `reg` (`1..=31`) is ignored.
  - `MFPS rt, reg` (`opcode=0x10, rs=0x00, rd=25, bit 0=0`) returns `sext32(PCCR)` for **all** `reg` values (`0..=31`).
  - `MTPC` / `MFPC` (`bit 0=1`) select `PCR0` for **even** `reg` (`(reg & 1) == 0`) and `PCR1` for **odd** `reg` (`(reg & 1) == 1`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_cop0/performance.cpp` & `performance.expected` lines 1–24 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/ee_cop0/performance.cpp`, `FETCHED`).

---

## 3. R5900 `COP1` FPU & `ps2float` (`ps2-common/src/ps2float.rs`, `ee/cop1.rs`)

### 3.1 `ADD.S` / `SUB.S` Single Guard Bit and Absence of Sticky Bit

- **Behavior**: R5900 `ADD.S` and `SUB.S` align mantissas using a single guard bit (`25`-bit intermediate significand) with no round or sticky bit, and round toward zero after normalization. When `exp_diff >= 25`, the smaller operand shifts completely out to zero (e.g., `1.0 - 2^-26 = 1.0` `0x3F80_0000` instead of IEEE-754 `0x3F7F_FFFF`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/arithmetic.expected` lines 1–160 (`FETCHED`).
- **Source Classification**: **Primary source** (`Play!` BSD-2 `Source/ee/FpAddTruncate.cpp` [`FETCHED`] & `ps2autotests` `cpu/ee_fpu/arithmetic.cpp` [`FETCHED`]).

### 3.2 `MUL.S` / `MULA.S` Radix-4 Booth Multiplier Low-Group Truncation

- **Behavior**: When the second multiplier operand `ft` (`t`) has significand bits `sig2[1:0] >= 2` (which encodes a negative Radix-4 Booth digit `-2` or `-1` in the lowest 2-bit Booth group), the R5900 multiplier omits the `+1` two's-complement hot-one of that lowest partial product (`prod = sig1 * sig2 - sig1` before shifting right by `23 + carry`). Consequently, multiplication is not commutative in the lowest bit for operands with `sig[1:0] >= 2`: `MUL.S(1.0, 0x3FFF_FFFF) = 0x3FFF_FFFE`, whereas `MUL.S(0x3FFF_FFFF, 1.0) = 0x3FFF_FFFF`.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 308, 311, 314–317 (`FETCHED`).
- **Source Classification**: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.3 `MADD.S` / `MADDA.S` / `MSUB.S` / `MSUBA.S` Product-Overflow Immediate Saturation

- **Behavior**: When the intermediate product `fs * ft` in a multiply-accumulate instruction overflows biased exponent `255` (`0xFF`), the hardware MAC unit immediately saturates the entire operation result to `±0x7FFF_FFFF` (with the sign of `±(fs * ft)`) and sets `FCR31.O | FCR31.SO`, without adding or subtracting `ACC` (e.g., `MADD.S(acc = -MAX, fs = +MAX, ft = +MAX) = +MAX = 0x7FFF_FFFF`).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/muldiv.expected` lines 57, 60, 63, 118, 121, 124, 178, 181, 191, 193, 204 (`FETCHED`).
- **Source Classification**: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.4 `DIV.S`, `SQRT.S`, and `RSQRT.S` Rounding and Exception Flags

- **Behavior**:
  - `DIV.S` and `SQRT.S` only update `I` (Invalid) and `D` (Divide-by-zero) cause/sticky flags in `FCR31`; they never set `O` (Overflow) or `U` (Underflow) even when the quotient saturates to `±0x7FFF_FFFF` or flushes to `±0.0`.
  - `SQRT.S(-0.0)` and `SQRT.S(-denormal)` return `+0.0` (`0x0000_0000`, clearing bit 31) while setting `FCR31.I | FCR31.SI`.
  - `DIV.S` and `SQRT.S` compute a 25-bit significand (`1` guard bit) and round via `(q25 + 1) >> 1`.
  - `RSQRT.S(fs, ft)` computes a 24-bit rounded reciprocal significand `1 / sqrt(ft)` and multiplies `fs` by that 24-bit reciprocal before truncating to 24 bits.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` line 48 (`FETCHED`), `muldiv.expected` line 29 (`FETCHED`), and `sqrt.expected` lines 10–65 (`FETCHED`).
- **Source Classification**:
  - `DIV.S`/`SQRT.S` flag behavior and `SQRT.S(-0.0) -> +0.0`: **Primary source** (`ps2tek` `#eefpu` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `fcr.cpp` / `sqrt.cpp` [`FETCHED`]).
  - Exact 25-bit guard-bit rounding formula `(x25 + 1) >> 1` for `DIV.S`/`SQRT.S`/`RSQRT.S`: **Inferred from test vectors (`fits test vectors; may not generalize`)**.

### 3.5 `FCR0` / `FCR31` Control Register Mirroring, Masking, and Cause Flag Clearing

- **Behavior**:
  - `CFC1 rt, fs` reads `FCR0` (`0x0000_2E30`) for `fs = 0..=15` and `FCR31` for `fs = 16..=31`.
  - `CTC1 rt, fs` modifies `FCR31` only when `fs == 31`, applying writable mask `0x0083_C078` and hardwired bits `0x0100_0001`; writes with `fs != 31` are ignored.
  - `ABS.S`, `NEG.S`, `MAX.S`, and `MIN.S` clear the `I, D, O, U` cause bits in `FCR31` without flushing denormal operands; `MOV.S`, `CVT.W.S`, `CVT.S.W`, and `C.<cond>.S` leave `I, D, O, U` unchanged.
- **Evidence**: `local/ps2autotests/tests/cpu/ee_fpu/fcr.expected` lines 3–29 (`FETCHED`), `transfer.expected` line 20 (`FETCHED`), and `arithmetic.expected` (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#eefpu` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `cpu/ee_fpu/{fcr,transfer,arithmetic}.cpp` [`FETCHED`]).

---

## 4. R5900 Multimedia Instructions (`ee/mmi.rs`)

### 4.1 `PHMSBH` Upper-Word Bitwise Inversion in `LO` / `HI`

- **Behavior**: `PHMSBH rd, rs, rt` (`MMI2` `shamt = 0x15`) computes `r[i] = p[2*i + 1] - p[2*i]` for each pair of 16-bit signed products (`i = 0..3`). In `LO` and `HI`, the lower 32-bit word of each 64-bit half receives `r[i]`, while the upper 32-bit word receives the bitwise NOT `!(p[2*i + 1] as u32)` (whereas `PHMADH` stores `p[2*i + 1] as u32` uninverted).
- **Evidence**: `local/ps2autotests/tests/cpu/ee_simd/muldiv.expected` lines 145–171 (`phmsbh` followed by `pmfhl.uw`, `FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/ee_simd/muldiv.cpp` & `muldiv.expected`, `FETCHED`).

---

## 5. EE 10-Channel DMAC & Scratchpad DMA (`bus/dmac.rs`)

### 5.1 `D8_SADR` / `D9_SADR` Masking (`0x3FF0`) and `D8_MADR` / `D9_MADR` Bit-31 Masking

- **Behavior**:
  - `SADR` on `fromSPR` (`D8`) and `toSPR` (`D9`) is masked with `0x3FF0` on register writes and wraps modulo `0x4000` (`(sadr + 16) & 0x3FF0`) during transfers.
  - `MADR` on `fromSPR` (`D8`) and `toSPR` (`D9`) clears bit 31 (`0x8000_0000`, the Scratchpad selection bit valid on other channels) as well as bits `0..=3` (`val & 0x7FFF_FFF0`).
- **Evidence**: `local/ps2autotests/tests/dma/spr/normal.expected` lines 2–36 (`FETCHED`) and `local/ps2autotests/tests/dma/spr/chain.expected` lines 2–25 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#dmacchannels` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `dma/spr/{normal,chain}.cpp` [`FETCHED`]).

### 5.2 `D_SQWC` 8-Bit Field Widths (`SQWC` / `TQWC`) and Interleave `MADR` Advancement

- **Behavior**:
  - In `D_SQWC` (`0x1000_E030`), `SQWC` (`bits 0..=7`) and `TQWC` (`bits 16..=23`) are 8-bit fields (`& 0xFF`).
  - In Interleave Mode (`CHCR.MOD == 2`), each slice transfers `TQWC` quadwords (advancing both `MADR` and `SADR` by `TQWC * 16` and decrementing `QWC` by `TQWC`) and then skips `SQWC` quadwords on `MADR` (`MADR += SQWC * 16`), including after the final slice when `QWC` reaches `0`.
- **Evidence**: `local/ps2autotests/tests/dma/spr/interleave.expected` lines 40–628 (`skip = 256` -> `0`, `skip = 257` -> `1`, `transfer = 257` -> `1`, and `MADR` delta warnings, `FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#dmacio` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `dma/spr/interleave.cpp` [`FETCHED`]).

### 5.3 Source Chain `TADR` Update on `END` / `RET` vs `REFE` / `REF` and `CHCR.TIE` Tag Interrupts

- **Behavior**:
  - In Source Chain Mode (`CHCR.MOD == 1`), `REFE` (`ID = 0`) and `REF` (`ID = 3`) set `MADR = tag.ADDR` and advance `TADR += 16`, whereas `END` (`ID = 7`) and `RET` (`ID = 6` when `CHCR.ASP == 0`) set `MADR = TADR + 16` and leave `TADR` pointing at the `END`/`RET` tag.
  - When `CHCR.TIE == 1` (bit 7) and the active DMATag has `IRQ == 1` (bit 31 of low word), the channel terminates (`CHCR.STR = 0`, `D_STAT.CISn = 1`) immediately after transferring the tag's `QWC` payload.
- **Evidence**: `local/ps2autotests/tests/dma/dmac/tagintr.expected` lines 2–9 (`FETCHED`) and `local/ps2autotests/tests/dma/spr/chain.expected` lines 4, 9, 15 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2tek` `#dmactags` [`local/ps2tek/index.html`, `FETCHED`] & `ps2autotests` `dma/dmac/tagintr.cpp` [`FETCHED`]).

### 5.4 DMA Transfer Data Movement at Start with Cycle-Driven Deferred Completion

- **Behavior**:
  - When a channel transfer is triggered (`CHCR.STR = 1` in `EeDmac::start_channel_transfer`), the payload data copy (`RDRAM <-> Scratchpad`) and channel pointer/counter updates (`MADR`, `SADR`, `TADR`, `QWC`, `CHCR.TAG`, `CHCR.ASP`) occur immediately at transfer start, while `CHCR.STR` stays `1` and `D_STAT.CIS[ch]` stays `0`.
  - A `HardwareEventKind::DmacChannelComplete(ch)` event is scheduled on `bus.scheduler` for `((qwc_transferred + tags_read).max(1) * 2)` EE cycles in the future (`1 BUSCLK = 2 EE cycles` per QWC/tag, `ps2tek` `#dmac`, `FETCHED`).
  - `bus.scheduler` is advanced strictly by executed EE CPU cycles (`EeCpuState::step` and `step_hle` call `bus.step_timers(1)` after every executed EE instruction, which calls `self.scheduler.advance(1)` and pops due events). Polling `CHCR` (`EeDmac::read32(&self, paddr)`) is a pure read and never skips time forward.
  - When `DmacChannelComplete(ch)` becomes due and is popped in `bus.step_timers`, `EeDmac::complete_channel(ch)` clears `CHCR.STR` to `0` and sets `D_STAT.CIS[ch]` to `1`.
- **Evidence**: `ps2tek` (`https://psi-rockin.github.io/ps2tek/#dmac`, `local/ps2tek/index.html`, `FETCHED`), `local/ps2autotests/tests/dma/dmasend.cpp` lines 31–34 & 60–63 (`while (--i > 0 && chan->chcr.Ongoing())`, `FETCHED`), and `dma/dmac/cpcond.expected` / `tagintr.expected` (`FETCHED`).
- **Source Classification**: **Primary source** for `1 QWC / BUSCLK` (`2 EE cycles / QWC`) rate (`ps2tek` `#dmac`, `FETCHED`); **Phase 1 architectural simplification** for moving the payload bytes upfront at transfer start while deferring `CHCR.STR = 0` and `D_STAT.CIS[ch] = 1` to the cycle-accurate completion event.

---

## 6. R3000A IOP CPU, Load Delay Slot, and `.irx` Relocations (`crates/ps2-core/src/iop/`)

### 6.1 R3000A 1-Instruction Load Delay Slot & Delay-Slot Overwrite Cancellation

- **Behavior**:
  - Unlike the R5900 EE (which has hardware interlocks on loads), the R3000A IOP (`MIPS I`) has a 1-instruction architectural load delay slot for `LB`, `LBU`, `LH`, `LHU`, `LW`, `LWL`, `LWR`, and `MFC0`.
  - The instruction immediately following a load (`N + 1`, the load delay slot) reads the **old** value of the target register `rt`.
  - If the load delay slot instruction `N + 1` itself writes to `rt` (either via an ALU/link write such as `ori $t0, $0, 0` or by queuing a new load targeting `rt`), the pending load from instruction `N` is **cancelled** so that `N + 1`'s write wins.
  - When `LWL` and `LWR` are executed back-to-back targeting the same register `rt`, the second instruction merges against the in-flight `PendingLoad` value from the first instruction rather than the stale architectural register value.
- **Evidence**: `local/ps2autotests/tests/cpu/iop/lsudelay.c` and `lsudelay.expected` lines 2–10 (`lb 0: 00000000`, `lw 0: 00000000`, `lb 1: 11223344`, `lw 1: 11223344`, `ld 1: 13371337 00000000`, `lw then branch: 2`, `FETCHED`), and `local/ps2autotests/tests/cpu/iop/lsu.expected` lines 56–68 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/lsudelay.c` & `cpu/iop/lsu.c`, `FETCHED`).

### 6.2 R3000A Branch-and-Link Register Write Order vs. Branch Delay Slot

- **Behavior**:
  - `JAL`, `JALR`, `BLTZAL`, and `BGEZAL` write the link register (`$ra` or `rd`) during the branch instruction **before** the branch delay slot instruction executes, so if the delay slot modifies the link register (`li $ra, 2`), the delay slot's write overwrites the return address.
  - For `JALR rd, rs` where `rd == rs`, `rs` is read as the branch target before `rd` is overwritten with `inst_pc + 8`.
  - `BLTZAL` (`REGIMM` `rt = 0x10`) and `BGEZAL` (`REGIMM` `rt = 0x11`) write `$ra = inst_pc + 8` unconditionally, even when the branch condition is false.
- **Evidence**: `local/ps2autotests/tests/cpu/iop/branchdelay.expected` lines 2–10 (`FETCHED`) and `local/ps2autotests/tests/cpu/iop/branch.expected` lines 57–83 & 144–170 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/{branch,branchdelay}.c`, `FETCHED`).

### 6.3 R3000A `DIV` / `DIVU` Corner Cases and Non-Stalling `MFHI` / `MFLO`

- **Behavior**:
  - Signed `DIV` by zero (`rt == 0`): `HI = rs`; `LO = 0xFFFF_FFFF` (`-1`) if `(rs as i32) >= 0`, else `0x0000_0001` (`+1`).
  - Signed `DIV` overflow (`0x8000_0000 / -1`): `HI = 0x0000_0000`, `LO = 0x8000_0000`.
  - Unsigned `DIVU` by zero (`rt == 0`): `HI = rs`, `LO = 0xFFFF_FFFF`.
  - `MFHI` and `MFLO` have no architectural delay slot (`hilodelay.expected` lines 2–5, `FETCHED`).
- **Evidence**: `local/ps2autotests/tests/cpu/iop/muldiv.expected` lines 2–58 (`FETCHED`) and `local/ps2autotests/tests/cpu/iop/hilodelay.expected` lines 2–5 (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/{muldiv,hilodelay}.c`, `FETCHED`).

### 6.4 IOP `.irx` (`ET_SCE_IOPRELEXEC = 0xFF80`) Pre-Linked Addends and `.iop.import` Stubs

- **Behavior**:
  - In IOP `.irx` binaries (`e_type = 0xFF80`), the `PT_LOAD` segment is linked at virtual address `0x0000_0000` with internal symbol values already encoded into the instruction/data words at `r_offset`; relocating the module to `base_addr` (`0x0001_0000`) adds `base_addr` to each `SHT_REL` target (`R_MIPS_32`, `R_MIPS_26`, and paired `R_MIPS_HI16` / `R_MIPS_LO16`).
  - Import stub tables in `.text` start with a 20-byte header (`0x41E0_0000`, `0`, `version`, 8-byte module name) followed by 8-byte function stubs (`0x03E0_0008` `jr $ra`; `0x2400_xxxx` `addiu $zero, $zero, import_index`).
- **Evidence**: `local/ps2autotests/tests/cpu/iop/*.irx` ELF headers, `SHT_REL` sections, and `.iop.import` tables (`FETCHED`).
- **Source Classification**: **Primary source** (`ps2autotests` `cpu/iop/*.irx` binaries, `FETCHED`).

---

## 7. LLE BIOS Boot Register Stubs & IOP Subsystems (`Task 6b` Rounds 1 & 2)

### 7.1 EE DMAC `0x1000_F500` Undocumented Control Latch (`EeDmac::d_f500`)

- **Behavior**:
  - Backed by a 32-bit R/W latch (`EeDmac::d_f500`, reset `0`) at physical address `0x1000_F500` in the `0x1000_F5xx` DMAC disable/enable register block (`D_ENABLEW` `0x1000_F520`, `D_ENABLER` `0x1000_F590`).
- **Evidence**: Hit on Round 1 Iteration 1 of coupled LLE BIOS boot (`EE PC = 0xBFC0_0838`, `sw $v0, 0($v1)` with `$v1 = 0xB000_F500`, `val = 0x0000_0001`). The BIOS only writes to `0x1000_F500` here and does not branch on reading it.
- **Source Classification**: `TODO(verify)` / `GUESS` for readback behavior (`ps2tek` `#dmacio` [`local/ps2tek/index.html`, `FETCHED`] documents `0x1000_F520` and `0x1000_F590` in the `0x1000_F5xx` block, but omits `0x1000_F500`).

### 7.2 EE Serial I/O (`EeSio`, `0x1000_F100..=0x1000_F1C0`)

- **Behavior**:
  - Implements the Emotion Engine UART control/status registers (`0x1000_F100` `SIO_LCR`, `0x1000_F110` `SIO_LSR`, `0x1000_F120` `SIO_IER`, `0x1000_F130` `SIO_ISR` with write-1-to-clear on bits `0..=2`, `0x1000_F140` `SIO_FCR`, `0x1000_F150` `SIO_BGR`, `0x1000_F180` `SIO_TXFIFO` / `KPUTCHAR` stdout capture, and `0x1000_F1C0` `SIO_RXFIFO`).
- **Evidence**: Hit on Round 1 Iteration 2 of LLE BIOS boot (`EE PC = 0x9FC4_2F9C`, `paddr = 0x1000_F100`, `val = 0x0000_0020`), immediately preceding EE serial console output.
- **Source Classification**: **Primary source** (`ps2tek` `#iomaps` [`local/ps2tek/index.html` line 212, `FETCHED`] for `0x1000_F180` `KPUTCHAR`; `ps2sdk` `ee/kernel/include/sio.h` & `ee/kernel/src/sio.c` [`FETCHED`]).

### 7.3 IOP Mode & Bootstrap Register (`0x1F80_1450`, `GM_IF`)

- **Behavior**:
  - Backed by `Bus::iop_gm_if: u32` (initialized to `0` on reset = PS2 R3000A mode). Early IOP BIOS reset code (`IOP PC = 0xBFC0_2040`) and the `intrman` module read `0xBF80_1450` and test bit 3 (`0x0008`) to branch between native PS2 IOP initialization (`bit 3 == 0`) and PS1 backwards-compatibility mode (`bit 3 == 1`).
- **Evidence**: Hit on Round 1 Iteration 3 of LLE BIOS boot (`IOP PC = 0xBFC0_2040`, `lw $t0, 5200($t0)` at `paddr = 0x1F80_1450`).
- **Source Classification**: **Primary source** (`ps2sdk` `common/include/iop_regs.h` line 55 [`FETCHED`], `common/include/iop_mmio_hwport.h` line 213 [`FETCHED`], `iop/system/intrman/src/intrman.c` lines 51–65 [`FETCHED`], and `ps2tek` `#ps1drv` / `#iop` [`local/ps2tek/index.html` lines 5095, 5297, `FETCHED`], noting `1F801570.3` on line 5095 of `ps2tek` is a digit-transposition typo for `1F801450.3`).

### 7.4 IOP Sub-System Bus Controller (`IopSsbusc`, `0x1F80_1000..=0x1F80_1023`, `0x1F80_1060`, `0x1F80_1400..=0x1F80_1423`)

- **Behavior**:
  - Backed by `IopSsbusc` (`memctrl1: [u32; 9]` at `0x1F80_1000..=0x1F80_1023`, `ram_size: u32` at `0x1F80_1060`, and `ssbus_ext: [u32; 9]` at `0x1F80_1400..=0x1F80_1423`).
  - At `IOP PC = 0xBFC0_20C4`, the BIOS performs a read-modify-write on `0x1F80_1010` (`lw $t3, 0($t1); andi $t3, $t3, 0x1000; or $t2, $t2, $t3; sw $t2, 0($t1)`) to preserve bit 12 (`ATYP`: `0 = 8-bit`, `1 = 16-bit`) while updating the BIOS ROM delay/size fields. It does not branch on the value read from `0x1F80_1010`.
- **Evidence**: Hit on Round 1 Iteration 4 of LLE BIOS boot (`IOP PC = 0xBFC0_20C4`, `paddr = 0x1F80_1010`).
- **Source Classification**: **Primary source** (`ps2sdk` `iop/system/ssbusc/include/ssbusc.h` lines 35–42 [`FETCHED`], `iop/system/ssbusc/src/ssbusc.c` [`FETCHED`], and `common/include/iop_mmio_hwport.h` [`FETCHED`]; `psx-spx` Memory Control 1/2 [`SEARCH-ONLY`]).

### 7.5 IOP Expansion Region 2 `POST2` Diagnostic Latch (`0x1F80_2070`)

- **Behavior**:
  - Backed by `Bus::iop_post2: u32` at `0x1F80_2070` (`A_IOP_UNK_2070` / `HW_POST2` power-on self-test diagnostic status latch).
- **Evidence**: Hit on Round 1 Iteration 5 of LLE BIOS boot (`IOP PC = 0xBFC0_20E8`, `paddr = 0x1F80_2070`, `val = 0x0000_00FC`). Write-only during boot; the BIOS does not branch on reading it.
- **Source Classification**: **Primary source** for register address (`ps2sdk` `common/include/iop_mmio_hwport.h` line 231, `FETCHED`); `TODO(verify)` for hardware LED/POST decoding.

### 7.6 IOP DMA Priority, Interrupt Control & Channel Registers (`IopDmacControl`, `0x1F80_1080..=0x1F80_10F4`, `0x1F80_1500..=0x1F80_157C`, `0x1F80_15F0`)

- **Behavior**:
  - Implements per-channel `Dn_MADR`, `Dn_BCR`, `Dn_CHCR`, `Dn_TADR` for channels `0..6` (`0x1F80_1080..=0x1F80_10EC`) and channels `7..12` (`0x1F80_1500..=0x1F80_155C`), auxiliary registers `0x1F80_1560..=0x1F80_1568` (`ch_aux_4_9_a` for `SIF0`, `SIF1`, `SPU`), `DPCR` (`0x1F80_10F0`, reset `0x0777_7777`), `DICR` (`0x1F80_10F4`, bits `24..=30` write-1-to-clear, bit `31` computed as `DICR.15 | (DMACINTEN.0 && DICR.23 && (DICR.24..30 != 0 || DICR2.24..29 != 0))`), `DPCR2` (`0x1F80_1570`), `DICR2` (`0x1F80_1574`, bits `24..=29` write-1-to-clear), `DMACEN` (`0x1F80_1578`, bit `0`), `DMACINTEN` (`0x1F80_157C`, bits `0..=1`), and `DPCR3` (`0x1F80_15F0`).
  - Any write to `Dn_CHCR` with bit 24 (`Start/Busy = 0x0100_0000`) set records an `UnimplementedIopRegister` error until the corresponding peripheral DMA transfer is implemented.
- **Evidence**: Hit on Round 1 Iteration 6 (`IOP PC = 0xBFC0_20E8`, `paddr = 0x1F80_10F0`, `val = 0x0777_7777`) and Round 2 Iteration 2 (`IOP PC = 0x0000_6A84` inside `dmacman._start`, `paddr = 0x1F80_15F0`, `val = 0x0000_0777`).
- **Source Classification**: **Primary source** (`ps2tek` `#iopdma` [`local/ps2tek/index.html` lines 3965–4118, `FETCHED`] and `ps2sdk` `iop/system/dmacman/src/dmacman.c` lines 20–73, 108–739 [`FETCHED`]).

### 7.7 EE RDRAM Memory Controller Hub (`RdramMch`, `0x1000_F400..=0x1000_F4F0`)

- **Behavior**:
  - Extends `RdramMch` beyond `0x1000_F430` (`MCH_RICM`) and `0x1000_F440` (`MCH_DRD`) with a 16-word `regs: [u32; 16]` array covering the remaining 16-byte-aligned MCH control/timing slots in `0x1000_F400..=0x1000_F4F0` (such as `0x1000_F410`).
- **Evidence**: Hit on Round 1 Iteration 7 of LLE BIOS boot (`EE PC = 0x9FC4_11B4` inside `# Initialize memory`, `sw $zero, 0($v1)` at `paddr = 0x1000_F410`).
- **Source Classification**: **Primary source** for `0x1000_F430` and `0x1000_F440` (`ps2tek` `#eerdram` [`local/ps2tek/index.html` lines 700–754, `FETCHED`]); `TODO(verify)` / `GUESS` for the remaining 16-byte-aligned slots in `0x1000_F400..=0x1000_F4F0`.

### 7.8 IOP `COP0.SR.IsC` Cache Isolation & `KSEG2` Cache Control (`0xFFFE_0130..=0xFFFE_014F`)

- **Behavior**:
  - When `COP0.SR` bit 16 (`IsC = 0x0001_0000`, Isolate Cache) is set on the R3000A IOP, data loads and stores (`LB`, `LBU`, `LH`, `LHU`, `LW`, `LWL`, `LWR`, `SB`, `SH`, `SW`, `SWL`, `SWR`) target the isolated cache lines/tags and do not read or overwrite `Bus::iop_ram`. Without `IsC` isolation, the IOP BIOS `FlushIcache()` / `FlushDcache()` routines overwrite the first 4 KB of IOP RAM (`0x0000_0000..=0x0000_0FFF`) with zeros during boot.
  - `Bus::iop_translate_addr` preserves `KSEG2` addresses (`vaddr >= 0xC000_0000`) instead of masking with `0x1FFF_FFFF` (which would otherwise alias `0xFFFE_0130` into BIOS ROM at `0x1FFE_0130`), and maps `0xFFFE_0130..=0xFFFE_014F` to `Bus::iop_cache_ctrl: [u32; 8]`.
- **Evidence**: Hit on Round 1 Iteration 8 of LLE BIOS boot (`IOP PC = 0x0000_0078` prior to `IsC` isolation, and `IOP PC = 0xBFC0_20E8` writing `0xFFFE_0130` / `0xFFFE_0140` / `0xFFFE_0144`).
- **Source Classification**: **Primary source** for R3000A `COP0.SR.IsC` cache isolation and `FFFE0000h (KSEG2) Cache control` (`ps2tek` `#memorymap` line 106 & `#iop` line 5326 [`local/ps2tek/index.html`, `FETCHED`]); `TODO(verify)` for individual bitfields of `0xFFFE_0130..=0xFFFE_014F`.

### 7.9 IOP Interrupt Controller (`IopIntc`, `0x1F80_1070..=0x1F80_1078`) & `COP0.Cause.IP2`

- **Behavior**:
  - `0x1F80_1070` (`I_STAT`): 32-bit interrupt status register with **write-0-to-clear** semantics (`self.stat &= val & 0x03FF_FFFF`).
  - `0x1F80_1074` (`I_MASK`): 32-bit interrupt mask register (`0` = disabled, `1` = enabled).
  - `0x1F80_1078` (`I_CTRL`): global interrupt enable (bit 0). **Reading `I_CTRL` returns bit 0 and clears bit 0 to `0`**, atomically disabling IOP interrupts (`CpuSuspendIntr` / `CpuDisableIntr`).
  - When `I_CTRL.0 != 0 && (I_STAT & I_MASK) != 0`, `IopIntc` asserts **`COP0.Cause.IP2` (bit 10, `0x0400`)**, which triggers an IOP `Int` (`Excode = 0x00`) exception when `COP0.Status.IEc == 1` (bit 0) and `COP0.Status.IM2 == 1` (bit 10).
  - **Primary-source conflict resolved**: `ps2tek` `#iopint` (`local/ps2tek/index.html` line 3960, `FETCHED`) writes `COP0.Cause:8` and `COP0.Status:8`, whereas MIPS I R3000A architecture and `ps2sdk` `iop/system/intrman/src/intrman.c` lines 838–843 & 1216–1220 (`FETCHED`) prove that bits `9:8` (`0x0300`) are software interrupt bits (`IP0`/`IP1`) and `IopIntc` asserts bit `10` (`IP2 = 0x0400`, enabled by `ori $t0, $t0, 0x404` in `syscall_handler_08_CpuEnableIntr`).
- **Evidence**: Hit on Round 2 Iteration 1 of LLE BIOS boot (`IOP PC = 0x0000_3D88` inside `intrman._start`, `paddr = 0x1F80_1074`, `val = 0x0000_0000`).
- **Source Classification**: **Primary source** (`ps2tek` `#iopint` [`local/ps2tek/index.html` lines 3916–3961, `FETCHED`] and `ps2sdk` `iop/system/intrman/src/intrman.c` lines 201–258, 816–875, 1216–1220 [`FETCHED`]).

---

## 8. Task 6b Complete Stub Audit Table (Rounds 1 & 2)

| Iteration | Physical Address Range | Subsystem / Struct | Behavior & Reset Value | Value Returned on Reads | BIOS Branches on Read Value? | Classification & Primary Source Evidence (`FETCHED` / `SEARCH-ONLY` / `GUESS`) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **R1.1** | `0x1000_F500` (EE) | `EeDmac::d_f500` | 32-bit R/W latch; reset `0x0000_0000`. Written with `1` at `EE PC = 0xBFC0_0838`. | Last 32-bit value written (`0` at reset). | **No** (write-only during boot). | **`GUESS` / `TODO(verify)`**: `ps2tek` `#dmacio` (`local/ps2tek/index.html` lines 1075–1095, `FETCHED`) documents `0x1000_F520` (`D_ENABLEW`) and `0x1000_F590` (`D_ENABLER`), but omits `0x1000_F500`. |
| **R1.2** | `0x1000_F100..=0x1000_F1C0` (EE) | `EeSio` (`LCR`, `LSR`, `IER`, `ISR`, `FCR`, `BGR`, `TXFIFO`, `RXFIFO`) | `LCR`/`LSR`/`IER`/`FCR`/`BGR`/`RXFIFO` 32-bit R/W (reset `0`); `SIO_ISR` (`0x1000_F130`) bits `0..=2` W1C; `SIO_TXFIFO` (`0x1000_F180`) appends byte to `stdout_buffer`. | `0x1000_F130` (`SIO_ISR`) returns `0` (`TX FIFO` empty / ready); `0x1000_F180` returns `0`; others return last written value. | **Yes (Steering on `0x1000_F130` `SIO_ISR`)**: EE BIOS `sio_putc` polls `(_lw(SIO_ISR) & 0xF000) == 0x8000` (TX FIFO full) before each byte write to `0x1000_F180`. | **Primary source (`FETCHED`)**: `ps2sdk` `ee/kernel/include/sio.h` & `ee/kernel/src/sio.c` lines 123–133 (`FETCHED`) and `ps2tek` `#iomaps` (`local/ps2tek/index.html` line 212, `FETCHED`). |
| **R1.3** | `0x1F80_1450` (IOP) | `Bus::iop_gm_if` (`GM_IF`) | 32-bit R/W latch; reset `0x0000_0000`. | `0x0000_0000` on reset (bit 3 = `0` -> PS2 mode; bit 31 = `0` -> MIPS R3000A); last written value after writes. | **Yes (Steering)**: `IOP PC = 0xBFC0_2040` and `intrman._start` branch on bit 3 (`0` = PS2 IOP bootstrap, `1` = PS1 mode); `eeconf` checks bit 31. | **Primary source (`FETCHED`)**: `ps2sdk` `iop/system/intrman/src/intrman.c` lines 51–65 (`FETCHED`), `common/include/iop_mmio_hwport.h` line 213 (`FETCHED`), `common/include/iop_regs.h` line 55 (`FETCHED`), and `ps2tek` `#ps1drv` / `#iop` (`local/ps2tek/index.html` lines 5095, 5297, `FETCHED`). |
| **R1.4** | `0x1F80_1000..=0x1F80_1023`, `0x1F80_1060`, `0x1F80_1400..=0x1F80_1423` (IOP) | `IopSsbusc` (`memctrl1`, `ram_size`, `ssbus_ext`) | 32-bit R/W arrays; reset `0x0000_0000`. | Last 32-bit value written (`0` at reset). At `IOP PC = 0xBFC0_20C4`, RMW on `0x1F80_1010` preserves bit 12 (`ATYP`). | **No**: The loop at `0xBFC0_20AC..0xBFC0_20D8` branches on the ROM init table opcodes at `0xBFC1_4200`, never on the value read from `0x1F80_1010`. | **Primary source (`FETCHED`)**: `ps2sdk` `iop/system/ssbusc/include/ssbusc.h` lines 35–42 (`FETCHED`), `iop/system/ssbusc/src/ssbusc.c` (`FETCHED`), and `common/include/iop_mmio_hwport.h` (`FETCHED`). |
| **R1.5** | `0x1F80_2070` (IOP) | `Bus::iop_post2` (`POST2`) | 8/16/32-bit R/W latch; reset `0x0000_0000`. Written with `0xFC` at `IOP PC = 0xBFC0_20E8`. | Last written value (`0` at reset). | **No** (write-only diagnostic POST code during boot). | **Primary source (`FETCHED`)** for address (`ps2sdk` `common/include/iop_mmio_hwport.h` line 231, `FETCHED`); `TODO(verify)` for external POST display decoding. |
| **R1.6** | `0x1F80_10F0..=0x1F80_10F4`, `0x1F80_1570..=0x1F80_157C` (IOP) | `IopDmacControl` (`DPCR`, `DICR`, `DPCR2`, `DICR2`, `DMACEN`, `DMACINTEN`) | `DPCR` reset `0x0777_7777`; others reset `0`. `DICR` bits `24..=30` & `DICR2` bits `24..=29` W1C; `DICR.31` master flag computed from `DICR`/`DICR2`/`DMACINTEN`. | Architectural register state (`DPCR = 0x0777_7777` at reset). | **No** during boot init (written by ROM init table and `dmacman._start`; read-modify-written by `sceEnableDMAChannel`). | **Primary source (`FETCHED`)**: `ps2tek` `#iopdma` (`local/ps2tek/index.html` lines 4051–4118, `FETCHED`) and `ps2sdk` `iop/system/dmacman/src/dmacman.c` (`FETCHED`). |
| **R1.7** | `0x1000_F400..=0x1000_F4F0` (EE, 16B-aligned slots except `F430`/`F440`) | `RdramMch::regs` | 32-bit R/W slots; reset `0x0000_0000`. `0x1000_F430` (`MCH_RICM`) & `0x1000_F440` (`MCH_DRD`) use full RDRAM init state machine from Task 2. | Last written 32-bit value (`0` at reset) for non-`F430`/`F440` slots. | **Yes (Steering on `F430`/`F440` only, verified in Task 2)**; **No** on `0x1000_F410` (written with `0` at `EE PC = 0x9FC4_11B4`). | **Primary source (`FETCHED`)** for `0x1000_F430`/`0x1000_F440` (`ps2tek` `#eerdram` lines 700–754, `FETCHED`); **`GUESS` / `TODO(verify)`** for remaining `0x1000_F4xx` slots. |
| **R1.8** | `0xFFFE_0130..=0xFFFE_014F` (IOP `KSEG2`) & `COP0.SR.IsC` | `Bus::iop_cache_ctrl` & `IopCpuState` | `0xFFFE_0130..=0xFFFE_014F` 32-bit R/W array (reset `0`); `COP0.SR.IsC` (bit 16) isolates cache loads/stores from `iop_ram`. | `0` when reading isolated I/D-cache tags/lines with `SR.IsC == 1`; last written value for `0xFFFE_0130..=0xFFFE_014F`. | **No** (used by `FlushIcache` / `FlushDcache` to clear cache tags without zeroing `iop_ram`). | **Primary source (`FETCHED`)**: `ps2tek` `#memorymap` line 106 & `#iop` line 5326 (`local/ps2tek/index.html`, `FETCHED`). |
| **R2.1** | `0x1F80_1070..=0x1F80_1078` (IOP) | `IopIntc` (`I_STAT`, `I_MASK`, `I_CTRL`) | Reset `0`; `I_STAT` write-0-to-clear; `I_MASK` R/W; `I_CTRL` bit 0 R/W where **read clears bit 0**. Asserts `COP0.Cause.IP2` (bit 10). | `I_STAT`, `I_MASK`, and old `I_CTRL.0` (before clearing `I_CTRL.0` to `0`). | **Yes (Steering)**: `intrman` `CpuSuspendIntr` branches on old `I_CTRL.0`, and IRQ dispatcher branches on `I_STAT & I_MASK`. | **Primary source (`FETCHED`)**: `ps2tek` `#iopint` (`local/ps2tek/index.html` lines 3916–3961, `FETCHED`) and `ps2sdk` `iop/system/intrman/src/intrman.c` lines 201–258, 816–875 (`FETCHED`). |
| **R2.2** | `0x1F80_1080..=0x1F80_10EC`, `0x1F80_1500..=0x1F80_1568`, `0x1F80_15F0` (IOP) | `IopDmacControl` (`channels[0..12]`, `ch_aux_4_9_a`, `dpcr3`) | Reset `0`; `MADR`/`TADR` masked to 24 bits (`0x00FF_FFFF`); `BCR`/`CHCR`/`ch_aux_4_9_a`/`dpcr3` 32-bit R/W. Writing `CHCR.24 = 1` raises `UnimplementedIopRegister`. | Last written architectural value (`CHCR.24 == 0` when stopped). | **Yes**: `dmacman_deinit` checks `(chcr & 0x0100_0000) != 0`; `sceEnableDMAChannel` RMWs `dpcr3`. | **Primary source (`FETCHED`)**: `ps2tek` `#iopdma` (`local/ps2tek/index.html` lines 3965–4050, `FETCHED`) and `ps2sdk` `iop/system/dmacman/src/dmacman.c` lines 20–73, 108–739 (`FETCHED`). |

---

## 9. VU Micro Pipeline Timing Model (Step 2b, commit `adf109c`)

Implemented in `crates/ps2-core/src/vu/pipeline.rs` (`VuPipeline`) and integrated into LIW execution in `crates/ps2-core/src/vu/micro.rs` (`step_micro_liw`). Below is the exact classification of every Step 2b rule into **Sourced (`FETCHED`)**, **`inferred; fits test vectors, may not generalize`**, or **Deferred**.

### 9.1 FMAC Writeback & Per-Lane `VF` RAW Hazard Stalls

- **Implemented behavior**:
  - `VuPipeline::fmac_pipe` tracks a 3-cycle sliding window (`[(u8, u8); 3]` storing `(vf_reg, lane_mask)`) of Upper FMAC `VF` destinations from `upper_fmac_vf_write`:
    - 6-bit `funct6` `0x00..=0x2D` and `0x2F`, plus `OPMSUB` (`0x2E`, effective mask `dest & 0b1110`), plus Upper `Type2`/`Type3` ops writing `ft` (`ITOF*` `0x13C..=0x13F`, `FTOI*` `0x17C..=0x17F`, `ABS` `0x1FD`); excludes `VF00` and `dest == 0`.
    - Slot `[0]` (1 cycle ago) incurs a `3`-cycle stall; slot `[1]` (2 cycles ago) incurs `2` cycles; slot `[2]` (3 cycles ago) incurs `1` cycle.
  - `compute_stall_cycles` gathers all `(reg, lane_mask)` reads from `upper_vf_reads` and (when `!has_i_bit`) `lower_vf_reads` (`MOVE`, `MR32`, `LQI`, `SQI`, `LQD`, `SQD`, `DIV`, `SQRT`, `RSQRT`, `MTIR`, `RINIT`, `RXOR`, `EFU`, and `SQ`), taking the maximum stall where `r_reg == w_reg && (r_mask & w_mask) != 0`.
  - `ACC`, `VF00`, `I`, `Q`, `P`, and `R` do not cause FMAC RAW stalls.
  - `VF` results are committed to `vu.vf` at LIW completion rather than delayed in a 4-stage writeback queue.
- **Classification**:
  - **Sourced (`FETCHED`)**: 4-cycle FMAC latency (`1..=3` stall cycles on dependent `VF` field reads) and exemption of `ACC`, `VF00`, `I`, `Q`, `P`, and `R` — `ps2tek` `#vupipelining` (`local/ps2tek/index.html` lines 2852–2874, `FETCHED`) and verified against `local/ps2autotests/tests/vu/lower/fdivdelay.cpp` (`PerformStalled` / `PerformStalledBranch`, `FETCHED`).
  - **`inferred; fits test vectors, may not generalize`**:
    1. Eager `VF` writeback at LIW issue instead of holding pending `VF` writes in a 4-cycle writeback buffer (matches after-stall RAW reads, but omits WAW-without-RAW ordering and mid-pipeline external `QMFC2` timing).
    2. Tracking only Upper FMAC `VF` writes in `fmac_pipe` (Lower `MOVE`/`MR32`/`LQ`/`LQI`/`LQD` writes are not pushed into an integer/load delay hazard window).

### 9.2 Flag Pipelines (`CLIP`, `MAC`, `Status`)

- **Implemented behavior**:
  - **`CLIP` 4-cycle pipeline**: `clip_latest` holds the most recently computed 24-bit `CLIP` shift register; `prepare_upper_clip` loads `clip_latest` into `vu.clip_flag` immediately before Upper `CLIP` computes `((val << 6) | bits) & 0x00FF_FFFF`, and `after_liw` restores `pre.pre_clip` to `vu.clip_flag` while saving the new shifted value into `clip_latest`.
  - Each VU cycle (`advance_cycle_with_fmac`, including stall cycles) shifts `clip_latest -> clip_queue[0] -> clip_queue[1] -> clip_queue[2] -> vu.clip_flag`, giving a 4-cycle delay before `FCGET`/`FCEQ`/`FCAND`/`FCOR` observe the new `CLIP` bits.
  - `FCSET` (`set_clip_flag`) writes `clip_latest`, all `3` entries of `clip_queue`, and `vu.clip_flag` immediately (if Upper `CLIP` and Lower `FCSET` execute in the same LIW, `clip_latest` keeps `CLIP`'s shifted result while `clip_queue` and `vu.clip_flag` receive the `FCSET` value).
  - **`MAC` & `Status` flag pipelines**: Step 2b does not yet compute or delay-pipeline Upper FMAC `MAC`/`Status` flag updates; only `CTC2` and `FS*`/`FM*` instructions access them directly.
- **Classification**:
  - **Sourced (`FETCHED`)**: `CLIP` 24-bit `(val << 6) | bits` shift layout and 4-cycle visibility delay — `ps2tek` `#vuregisters` (`local/ps2tek/index.html` lines 2792–2802, `FETCHED`) and `local/ps2autotests/tests/vu/upper/clip.cpp` + `clip.expected` (lines 16–31, `FETCHED`).
  - **`inferred; fits test vectors, may not generalize`**: `FCSET` flushing all `3` `clip_queue` slots and the same-cycle `CLIP` + `FCSET` resolution rule (neither case is directly exercised by `clip.cpp`).
  - **Deferred**: `MAC` and `Status` 4-cycle flag pipelines (not required by current passing Phase 2a suites).

### 9.3 `Q` (`FDIV`) and `P` (`EFU`) Latency

- **Implemented behavior**:
  - **`FDIV` (`Q`)**: `DIV` (`0x3BC`) and `SQRT` (`0x3BD`) take `7` VU cycles (`FDIV_LATENCY_DIV_SQRT = 7`); `RSQRT` (`0x3BE`) takes `13` VU cycles (`FDIV_LATENCY_RSQRT = 13`).
  - Upon issue, the computed result is staged into `fdiv_pending_q` and `vu.q` is restored to `pre.pre_q`. Subsequent Upper/Lower reads of `Q` during an active `FDIV` do **not** stall and read the previous `vu.q` value. Each VU cycle decrements `fdiv_cycles` and commits `fdiv_pending_q -> vu.q` on `0`.
  - Issuing `DIV`/`SQRT`/`RSQRT` (`0x3BC..=0x3BE`) or `WAITQ` (`0x3BF`) while `fdiv_cycles > 0` stalls `fdiv_cycles` cycles in `before_liw` before issuing. `CTC2 $22` (`vi22` / `Q`) clears `fdiv_cycles = 0`.
  - **`EFU` (`P`)**: `efu_pending_p` / `efu_cycles` model the EFU busy timer using `efu_latency` (`ESADD=11`, `ERSADD=18`, `ELENG=18`, `ERLENG=24`, `EATANxy/EATANxz=54`, `ESUM=12`, `ESQRT=12`, `ERSQRT=18`, `ERCPR=12`, `ESIN=29`, `EATAN=54`, `EEXP=44`), stalling on `WAITP` (`0x7BF`) or another EFU op.
- **Classification**:
  - **Sourced (`FETCHED`)**: `Q` latencies (`7` for `DIV`/`SQRT`, `13` for `RSQRT`), non-stalling reads returning old `Q`, and `FDIV`/`WAITQ` busy stalls — `ps2tek` `#vupipelining` (`local/ps2tek/index.html` lines 2887–2904, `FETCHED`) and `local/ps2autotests/tests/vu/lower/fdivdelay.cpp` + `fdivdelay.expected` (`FETCHED`). `EFU` latency numbers (`11..54`) — `ps2tek` `#vupipelining` (`local/ps2tek/index.html` lines 2915–2927, `FETCHED`).
  - **`inferred; fits test vectors, may not generalize`**: Exact `CTC2 $22` cancellation behavior and exact `efu_cycles` vs `efu_cycles - 1` stall boundary (noting `ps2tek` line 2912 states EFU lacks the writeback hazard stage); EFU numeric evaluation is implemented in `crates/ps2-core/src/vu/efu.rs` (`184/208`, see §12).

### 9.4 Stall / Branch Interaction

- **Implemented behavior**:
  - In `step_micro_liw`, any pending `branch_delay_active` target is latched into `vu.tpc` at the start of the delay-slot LIW's successor; before executing `(upper, lower)`, `before_liw(vu, upper, lower, has_i_bit)` computes `stall = compute_stall_cycles(...)` and runs `advance_cycle_with_fmac` / `advance_stall_cycle` for all `stall` cycles, draining `fmac_pipe`, `clip_queue`, `fdiv_cycles`, and `efu_cycles` in order.
  - Consequently, a branch delay-slot LIW that triggers an FMAC RAW stall (for example `IBLTZ` + `MADD` followed by delay slot `CLIP` in `fdivdelay.cpp` `PerformStalledBranch`) drains those `3` stall cycles out of `fdiv_cycles` and `clip_queue` while executing the delay slot and still branching cleanly to `branch_delay_target`.
  - Externally, `Bus::step_timers(ee_cycles)` calls VU micro stepping `1.max(ee_cycles / 2)` times per scheduler slice: an EE step with `ee_cycles == 1` executes one VU LIW (including any internal VU stall cycles drained instantaneously inside `step_micro_liw`), unless EE executes `CFC2.I`/`CTC2.I`/`QMFC2.I`/`QMTC2.I`, which drains VU0 via `finish_vu0_micro`.
- **Classification**:
  - **Sourced (`FETCHED`)**: Delay-slot FMAC RAW stalls draining in-flight `FDIV`/`CLIP` pipelines cleanly across taken branches — `local/ps2autotests/tests/vu/lower/fdivdelay.cpp` (`PerformStalledBranch`) + `fdivdelay.expected` (`FETCHED`).
  - **`inferred; fits test vectors, may not generalize`**: Draining all VU stall cycles instantaneously inside one `step_micro_liw` invocation rather than co-scheduling EE vs stalled VU cycle-by-cycle in `Bus::step_timers` (consistent with `1.max(ee_cycles / 2)` and synchronous `finish_vu0_micro`, unverified against cycle-exact EE/VU race tests).

### 9.5 Step 2b Rule Summary Table

| Rule Area | Implemented Mechanism | Classification | Evidence |
| :--- | :--- | :--- | :--- |
| FMAC latency (`4` cycles) & lane-masked RAW stalls (`1..3` cycles) | `fmac_pipe` 3-slot window checked against `upper_vf_reads`/`lower_vf_reads`; `ACC`/`VF00`/`I`/`Q`/`P`/`R` exempt | **Sourced (`FETCHED`)** | `ps2tek` `#vupipelining`; `fdivdelay.cpp` |
| Eager `VF` writeback & Upper-only `fmac_pipe` tracking | `VF` written at LIW issue; Lower `MOVE`/`MR32`/`LQ*` not tracked in `fmac_pipe` | **`inferred; fits test vectors, may not generalize`** | Matches RAW stall tests; untested on WAW / external `QMFC2` |
| `CLIP` 4-cycle delay & 6-bit shift register | `clip_latest -> clip_queue[0..2] -> vu.clip_flag` | **Sourced (`FETCHED`)** | `ps2tek` `#vuregisters`; `clip.cpp` & `.expected` |
| `FCSET` queue flush & `CLIP`+`FCSET` same-cycle rule | `FCSET` overwrites `clip_queue[0..2]` & `vu.clip_flag` | **`inferred; fits test vectors, may not generalize`** | Not directly tested in `clip.cpp` |
| `MAC` & `Status` 4-cycle flag pipelines | Direct access via `CTC2`/`FS*`/`FM*`; FMAC flag updates omitted | **Deferred** | Not exercised by current passing Phase 2a suites |
| `Q` (`DIV`/`SQRT`=`7`, `RSQRT`=`13`), stale-`Q` reads, `WAITQ`/FDIV stalls | `fdiv_pending_q` committed on `fdiv_cycles == 0` | **Sourced (`FETCHED`)** | `ps2tek` `#vupipelining`; `fdivdelay.cpp` |
| `EFU` `P` latency table (`11..54` cycles) | `efu_latency` table in `vu/pipeline.rs` | **Sourced (`FETCHED`)** (ops in `vu/efu.rs`, `184/208`; see §12) | `ps2tek` `#vupipelining` |
| `EFU` stall boundary (`efu_cycles` vs `efu_cycles - 1`) & `CTC2 $22` cancel | Stalls full `efu_cycles`; `CTC2 $22` zeroes `fdiv_cycles` | **`inferred; fits test vectors, may not generalize`** | Unverified pending Step 2c EFU resume |
| Delay-slot RAW stalls draining `FDIV`/`CLIP` across branches | `before_liw` drains stall cycles cleanly with `branch_delay_active` | **Sourced (`FETCHED`)** | `fdivdelay.cpp` (`PerformStalledBranch`) |
| Instantaneous stall drain per `step_micro_liw` in `Bus::step_timers` | `1.max(ee_cycles / 2)` LIW steps without slowing EE clock | **`inferred; fits test vectors, may not generalize`** | Untested against cycle-accurate EE/VU race tests |

---

## 10. `VIF0`/`VIF1` Registers & Known Resolutions (Step 3)

### 10.1 Verified Register Map (`FETCHED`)

Primary sources verified directly (`FETCHED`):
- `local/ps2tek/index.html` `#vifioregisters` (`lines 2395..=2501`), `#vifcommands` (`lines 2505..=2590`), `#gifio` (`lines 1265..=1355`)
- `local/ps2autotests/tests/dma/vif/vifregs.h` (`lines 1..=125`)

| `VIF0` Addr | `VIF1` Addr | Name | Access | Verified Bit Layout (`FETCHED`) |
| :--- | :--- | :--- | :--- | :--- |
| `0x1000_3800` | `0x1000_3C00` | `VIFn_STAT` | `R` | `0..=1`: `VPS` (`0=Idle`, `1=DataWait`, `2=Decoding`, `3=Transferring`); `2`: `VEW`; `3`: `VGW` (`VIF1`); `6`: `MRK`; `7`: `DBF` (`VIF1`); `8`: `VSS`; `9`: `VFS`; `10`: `VIS`; `11`: `INT`; `12`: `ER0`; `13`: `ER1`; `23`: `FDR` (`VIF1`, `0=Mem->VIF`, `1=VIF->Mem`); `24..=28`: `FQC` (max `8` `VIF0`, `16` `VIF1`) |
| `0x1000_3810` | `0x1000_3C10` | `VIFn_FBRST` | `W` | `0`: `RST`; `1`: `FBK`; `2`: `STP`; `3`: `STC` (clears `VSS`, `VFS`, `VIS`, `INT`, `ER0`, `ER1`) |
| `0x1000_3820` | `0x1000_3C20` | `VIFn_ERR` | `R/W` | `0`: `MII`; `1`: `ME0`; `2`: `ME1` |
| `0x1000_3830` | `0x1000_3C30` | `VIFn_MARK` | `R/W` | `0..=15`: `MARK` (writes clear `STAT.MRK`) |
| `0x1000_3840` | `0x1000_3C40` | `VIFn_CYCLE` | `R` | `0..=7`: `CL`; `8..=15`: `WL` |
| `0x1000_3850` | `0x1000_3C50` | `VIFn_MODE` | `R` | `0..=1`: `MODE` (`0=Normal`, `1=Offset`, `2=Difference`, `3=Write-through to ROW (writes both VU data memory and ROW on 00 Data lanes per stmod.expected)`) |
| `0x1000_3860` | `0x1000_3C60` | `VIFn_NUM` | `R` | `0..=7` (`VIF0`) / `0..=8` (`VIF1`, up to `256` per `unpack.expected` line 54 vs `ps2tek` `0..=7`): untransferred `MPG`/`UNPACK` unit count |
| `0x1000_3870` | `0x1000_3C70` | `VIFn_MASK` | `R` | `0..=31`: 16 x 2-bit lane write mask fields (`0=Data`, `1=ROW`, `2=COL`, `3=WriteProtect`) |
| `0x1000_3880` | `0x1000_3C80` | `VIFn_CODE` | `R` | `0..=15`: `IMMEDIATE`; `16..=23`: `NUM`; `24..=31`: `CMD` |
| `0x1000_3890` | `0x1000_3C90` | `VIFn_ITOPS` | `R` | `0..=9`: `ITOPS` |
| — | `0x1000_3CA0` | `VIF1_BASE` | `R` | `0..=9`: `BASE` (`VIF1` only) |
| — | `0x1000_3CB0` | `VIF1_OFST` | `R` | `0..=9`: `OFST` (`VIF1` only) |
| — | `0x1000_3CC0` | `VIF1_TOPS` | `R` | `0..=9`: `TOPS` (`VIF1` only: `BASE` when `DBF=0`, `BASE + OFST` when `DBF=1`) |
| `0x1000_38D0` | `0x1000_3CD0` | `VIFn_ITOP` | `R` | `0..=9`: `ITOP` (latched on `MSCAL`/`MSCALF`/`MSCNT`) |
| — | `0x1000_3CE0` | `VIF1_TOP` | `R` | `0..=9`: `TOP` (`VIF1` only, latched & `DBF` toggled on `MSCAL`/`MSCALF`/`MSCNT`) |
| `0x1000_3900..=3930` | `0x1000_3D00..=3D30` | `VIFn_R0..R3` | `R` | `16`-byte stride; 4 x 32-bit row fill values |
| `0x1000_3940..=3970` | `0x1000_3D40..=3D70` | `VIFn_C0..C3` | `R` | `16`-byte stride; 4 x 32-bit column fill values |
| `0x1000_4000..=4FFF` | `0x1000_5000..=5FFF` | `VIFn_FIFO` | `W` (`R` if `FDR=1`) | Direct FIFO aperture (`8` QWC max for `VIF0`, `16` QWC max for `VIF1`) |

### 10.2 `MPG` (`0x4A`) vs Unresolved `0x46` (`emit_vifcode.cpp`)

- **`MPG` (`0x4A`, `ps2tek` `#vifcommands`, `FETCHED`)**: Uploads `NUM * 8` bytes (`2048` bytes if `NUM == 0`, i.e., `NUM * 2` 32-bit words) starting at byte address `(IMMEDIATE * 8)` in VU micro memory (`4 KB` for `VU0`, `16 KB` for `VU1`), wrapping cleanly at VU micro memory size. Wait-for-VU (`VEW`) stalls before starting if VU is executing.
- **No `ps2autotests` hardware suite exercises `MPG`**: Neither `basic.cpp`, `intstall.cpp`, `mskpath3.cpp`, `stcycl.cpp`, `stmod.cpp`, nor `unpack.cpp` tests `MPG`. Behavior beyond `ps2tek` (`0x4A`) is verified only by our own unit tests (including micro memory address wrapping and odd-length uploads).
- **`0x46` in `local/ps2autotests/tests/dma/vif/emit_vifcode.cpp` (`line 26`, `CMD_MPG = 0x46 << 24`)**: **`UNRESOLVED` — not guessed or aliased to `0x4A`**. `emit_vifcode.cpp` defines `CMD_MPG = 0x46 << 24` whereas `ps2tek` documents `4Ah MPG`. No test in `local/ps2autotests` executes `Packet::MPG`, and no real-hardware capture in the repo clarifies `0x46` (no behavior guessed).

### 10.3 `ps2autotests` Hardware Nuances (`FETCHED` vs Inferred)

- **`VIF1_NUM` 9-bit `256` vs `VIF0_NUM` 8-bit `0` (`FETCHED`, `unpack.expected` lines 27, 54)**: On `No data UNPACK(NUM: 256)`, `VIF0` reports `NUM = 0` (`8`-bit counter) while `VIF1` reports `NUM = 256` (`9`-bit counter).
- **`MARK (I)` & `VIS` Interrupt Stall (`FETCHED`, `intstall.expected` lines 4, 6, 8)**: Non-`MARK` commands with bit 31 (`I`) set (`ERR.MII == 0`) set `STAT.INT` and arm `VIS` so the next decoded non-`MARK` command sets `STAT.VIS` (`STAT = 0x0000_0c02`, `VPS = 2`) before executing its payload. `MARK (I)` sets `STAT.MRK` and `STAT.INT` (`STAT = 0x0000_0840`) **without** arming `VIS` on subsequent commands.
- **`STAT.FQC == 0` after 3-QWC `intstall` DMA (`inferred; fits test vectors, may not generalize`)**: On real PS2 silicon (`intstall.expected`), a 3-QWC DMA transfer containing `NOP(I)` -> `STMASK` finishes DMA (`STR = 0`), stops decoding before QWC 1..2, yet reads `STAT.FQC == 0`. Modeled with an internal 4-QWC (`16`-word) pre-decode staging buffer fed by the DMAC-facing `8`/`16`-QWC FIFO (`FQC` counts QWC still held in that DMAC-facing FIFO).
- **`STCYCL` VU Data Memory Wrap (`FETCHED`, `stcycl.expected` lines 106, 114..126, 369)**: `UNPACK` destination quadword address wraps modulo `256` QWC (`4 KB`, `& 0xFF`) on `VIF0` and modulo `1024` QWC (`16 KB`, `& 0x3FF`) on `VIF1`.
- **`GIF` Path-2 Stub (`0x1000_3000..=0x1000_30A0`, `FETCHED` `#gifio`)**: Step 3 adds a minimal `GIF` stub storing received `VIF1 DIRECT`/`DIRECTHL` (`0x50`/`0x51`) packets and tracking `GIF_STAT.M3P` (bit 1, controlled by `VIF1 MSKPATH3`). Replaced by full `GifUnit` in Step 4 (§11).

## 11. Phase 2a Step 4: GIF, DMAC D2 (`PATH3`), and GS Privileged / `SIGNAL` / `LABEL`

### 11.1 `SIGNAL` (`0x60`) & `LABEL` (`0x62`) Rule and Step-by-Step Trace

- **Rule (`FETCHED` `ps2tek` `#gsregisterlist` + `ps2autotests` `tests/gs/{signal,label}.cpp` + `SEARCH-ONLY`)**:
  - Internal GS register `0x60` (`SIGNAL`) and `0x62` (`LABEL`) carry `ID` in bits `0..=31` and `IDMSK` in bits `32..=63` (`GS::SIGNAL(id, idmsk)` / `GS::LABEL(id, idmsk)`).
  - **`SIGNAL` (`0x60`) stall rule [`SEARCH-ONLY` — Sony GS manual via search snippets; supported by `local/ps2autotests/tests/gs/signal.cpp` lines 8 and 28 comment `"//Signal must be cleared to unstall GIF"`]**:
    - If `GS_CSR.SIGNAL` (bit 0) is currently `1`, GS/GIF **stalls** before executing the incoming `SIGNAL` write until the EE clears `GS_CSR.SIGNAL` by writing `1` to `GS_CSR[0]` (`0x1200_1000`, W1C). No registers update while stalled.
    - When `GS_CSR.SIGNAL == 0`:
      - `SIGLBLID[31:0]` (`SIGID`) `= (SIGLBLID[31:0] & !IDMSK) | (ID & IDMSK)`
      - `SIGLBLID[63:32]` (`LBLID`) is preserved unchanged.
      - `GS_CSR.SIGNAL` (bit 0) becomes `1` (raising `IntcFlags::GS` if `GS_IMR[8]` is `0`).
  - **`LABEL` (`0x62`) rule (`FETCHED` `ps2autotests` `tests/gs/label.cpp`)**:
    - Does not inspect or set `GS_CSR.SIGNAL`.
    - `SIGLBLID[63:32]` (`LBLID`) `= (SIGLBLID[63:32] & !IDMSK) | (ID & IDMSK)`
    - `SIGLBLID[31:0]` (`SIGID`) is preserved unchanged.

- **Trace of `local/ps2autotests/tests/gs/signal.cpp` against `signal.expected`**:
  1. `signal.expected` line 1 (`signal.cpp:48`): `-- TEST BEGIN`
     - Predicted state before tests: initial boot `CSR[SIGNAL] = 0`, `SIGLBLID = 0`.
  2. `testSetBits()` (`signal.cpp:7-25`):
     - Line 9 writes `*GS::CSR = 1` -> `CSR.SIGNAL = 0` (unstalled).
     - Line 10 writes `*GS::SIGLBLID = 0x76543210_00000000` -> `LBLID = 0x76543210`, `SIGID = 0x00000000`.
     - Lines 12-21 `DMA::SendSimple(DMA::D2, ...)` sends `GIFtag(NLOOP=1, EOP=1, FLG=0 [PACKED], NREG=1, REGS=0xE [A+D])` + `A+D(reg=0x60 [SIGNAL], data=0x000000F0_000000F0)` (`ID=0x000000F0`, `IDMSK=0x000000F0`).
     - Since `CSR.SIGNAL == 0`, rule yields `SIGID = (0x00000000 & !0xF0) | (0xF0 & 0xF0) = 0x000000F0`, `LBLID = 0x76543210`, `CSR.SIGNAL = 1`.
     - Predicted print (`signal.cpp:23-24`): `CSR & CSR_SIGNAL = 0000000000000001`, `SIGLBLID = 76543210000000f0`.
     - Matches `signal.expected` line 2 verbatim: `Set signal bits -> CSR[SIGNAL] = 0000000000000001, SIGLBLID: 76543210000000f0`.
  3. `testClearBits()` (`signal.cpp:27-45`):
     - Line 29 writes `*GS::CSR = 1` -> clears `CSR.SIGNAL` (`1 -> 0`) so GIF does not stall on this second `SIGNAL`.
     - Line 30 writes `*GS::SIGLBLID = 0x33333333_33333333` -> `LBLID = 0x33333333`, `SIGID = 0x33333333`.
     - Lines 32-41 `DMA::SendSimple(DMA::D2, ...)` sends `A+D(reg=0x60 [SIGNAL], data=0x000000F0_00000000)` (`ID=0x00000000`, `IDMSK=0x000000F0`).
     - Since `CSR.SIGNAL == 0`, rule yields `SIGID = (0x33333333 & !0xF0) | (0x00 & 0xF0) = 0x33333303`, `LBLID = 0x33333333`, `CSR.SIGNAL = 1`.
     - Predicted print (`signal.cpp:43-44`): `CSR & CSR_SIGNAL = 0000000000000001`, `SIGLBLID = 3333333333333303`.
     - Matches `signal.expected` line 3 verbatim: `Clear signal bits -> CSR[SIGNAL] = 0000000000000001, SIGLBLID: 3333333333333303`.
  4. `signal.expected` line 4 (`signal.cpp:53`): `-- TEST END`
     - Final state: `CSR[SIGNAL] = 1`, `SIGLBLID = 0x3333333333333303`. Every line follows cleanly from the rule.

- **Trace of `local/ps2autotests/tests/gs/label.cpp` against `label.expected`**:
  1. `label.expected` line 1 (`label.cpp:42`): `-- TEST BEGIN`
  2. `testSetBits()` (`label.cpp:7-22`): writes `SIGLBLID = 0x00000000_76543210`, DMA `D2` sends `A+D(reg=0x62 [LABEL], data=0x000000F0_000000F0)`. Rule yields `LBLID = (0 & !0xF0) | 0xF0 = 0x000000F0`, `SIGID = 0x76543210`. Matches `label.expected` line 2: `Set label bits -> SIGLBLID: 000000f076543210`.
  3. `testClearBits()` (`label.cpp:24-39`): writes `SIGLBLID = 0x33333333_33333333`, DMA `D2` sends `A+D(reg=0x62 [LABEL], data=0x000000F0_00000000)`. Rule yields `LBLID = (0x33333333 & !0xF0) | 0 = 0x33333303`, `SIGID = 0x33333333`. Matches `label.expected` line 3: `Clear label bits -> SIGLBLID: 3333330333333333`.
  4. `label.expected` line 4 (`label.cpp:47`): `-- TEST END`.

### 11.2 GS Privileged Registers (`0x1200_0000..=0x1200_1FFF`) — `SEARCH-ONLY` / `GUESS` Inventory

- **`GS_CSR` (`0x1200_1000`) beyond `SIGNAL`/`FINISH`**:
  - `SIGNAL` (bit 0, W1C) & `SIGLBLID` (`0x1200_1080`): covered above (`FETCHED` `ps2autotests` + `SEARCH-ONLY` stall).
  - `FINISH` (`0x61` internal reg -> `GS_CSR[1]`, W1C): register address `0x61` is `FETCHED` (`ps2tek` `#gsregisterlist`); setting `GS_CSR.FINISH` (bit 1) + IRQ raising is **`SEARCH-ONLY`**.
  - `HSINT` (bit 2, W1C), `VSINT` (bit 3, W1C), `EDWINT` (bit 4, W1C): **`SEARCH-ONLY`** (`ps2sdk` `libgs.h` / Sony GS docs).
  - `FLUSH` (bit 8) & `RESET` (bit 9): **`SEARCH-ONLY` / `GUESS`** (`TODO(verify)` exact effect of `GS_CSR.FLUSH` vs `RESET` on internal GS/GIF state beyond resetting GS privileged interrupts).
  - `NFIELD` (bit 12), `FIELD` (bit 13, toggled on VBlank), `FIFO` status bits `14..=15` (`01` = empty): **`SEARCH-ONLY`** (`ps2sdk` `libgs.h`).
  - `REV` (`bits 16..=23 = 0x1B`) and `ID` (`bits 24..=31 = 0x55`): **`SEARCH-ONLY` / `GUESS`** (console revision dependent; hardcoded `0x551B << 16`).
- **`GS_IMR` (`0x1200_1010`)**: **`SEARCH-ONLY`** — bits `8..=12` (`SIGMSK`, `FINISHMSK`, `HSMSK`, `VSMSK`, `EDWMSK`), active-high (`1` = masked, `0` = enabled) gating `IntcFlags::GS` (`INTC` bit 0). Resets to `0x7F00` (`bits 8..=14` masked); `TODO(verify)` reset bits `13..=14` vs `ps2sdk`/hardware.
- **`GS_BUSDIR` (`0x1200_1040`)**: **`SEARCH-ONLY`** — bit 0 selects direction (`0 = Host->Local` / `EE->GS`, `1 = Local->Host` / `GS->EE`), reflected into `GIF_STAT.DIR` (bit 12). When `GS_BUSDIR == 1`, `ps2tek` is silent on VRAM readback FIFO contents and DMA handshake -> explicit unimplemented error (`TODO(verify)`).
- **Other GS privileged display registers (`PMODE`, `SMODE1`, `SMODE2`, `SRFSH`, `SYNCH1`, `SYNCH2`, `SYNCV`, `DISPFB1`, `DISPLAY1`, `DISPFB2`, `DISPLAY2`, `EXTBUF`, `EXTDATA`, `EXTWRITE`, `BGCOLOR`)**: latch-only storage in `GsPrivilegedStub` (**`GUESS`** / minimal latches for BIOS/SDK writes).

### 11.3 GIF & DMAC `D2` (`PATH3`) Behaviors Beyond `gs/label` & `gs/signal` (`untested by hardware suites`)

All behaviors below are implemented strictly from `local/ps2tek/index.html` (`#gif`, `#gifio`, `#giftags`, `#gifdataformats`, `#gifpath3masking`, `#dmac`, `#dmacio`) and marked **`untested by hardware suites`**:

- **`GIF_CTRL` (`0x1000_3000`, W-only)**: bit 0 `RST` resets GIF paths, queues, tags, and internal state (clears `PSE` and `IP3` as well — `TODO(verify)` whether `RST` clears `M3P` or leaves `VIF1 MSKPATH3` bit untouched; left `M3P` preserved across `GIF_CTRL.RST` unless `VIF1` writes `MSKPATH3` or resets). Bit 3 `PSE` pauses GIF transfers when `1` (`GIF_STAT.PSE`).
- **`GIF_MODE` (`0x1000_3010`, W-only)**:
  - Bit 0 `M3R`: masks `PATH3` when `1`. Per `ps2tek` `#gifpath3masking`, when `PATH3` DMA/transfer is already active, masking takes effect after the ongoing `PATH3` transfer ends (`pending_m3r` latched until `PATH3` packet/DMA finishes — `TODO(verify)` exact boundary between packet `EOP` vs `D2` DMA completion).
  - Bit 2 `IMT`: intermittent mode (`PATH3` may yield to `PATH1`/`PATH2`). Note `ps2tek` itself writes `"(by intermittent mode?)"`; **`TODO(verify)`** exact preemption point (preempted at QWC boundaries between `PATH3` GIFtag `NLOOP` steps, saving `GIF_P3CNT` & `GIF_P3TAG` and setting `GIF_STAT.IP3 = 1`).
- **`GIF_STAT` (`0x1000_3020`, R-only)**: `0=M3R`, `1=M3P`, `2=IMT`, `3=PSE`, `5=IP3`, `6=P3Q`, `7=P2Q`, `8=P1Q`, `9=OPH`, `10..=11=APATH` (`0=Idle,1=PATH1,2=PATH2,3=PATH3`), `12=DIR` (`GS_BUSDIR & 1`), `24..=28=FQC` (`0..=16` QWCs in PATH3 16-QWC FIFO).
- **`PSE`-gated debug registers (`0x1000_3040..=0x1000_30A0`, R-only)**: `GIF_TAG0..3`, `GIF_CNT`, `GIF_P3CNT`, `GIF_P3TAG`. Per `ps2tek` `#gifio`, readable **only while `GIF_CTRL.PSE == 1`**, returning `0` otherwise. **`TODO(verify)`**: `GIF_CNT[20..=29]` (`VU` address for `PATH1` `XGKICK`) stays `0` until Phase 2b `XGKICK`.
- **Priority & Arbitration (`PATH1 > PATH2 > PATH3`)**: `PATH1` (`P1Q` / priority queue slot; `XGKICK` feeder deferred to Phase 2b), `PATH2` (`VIF1 DIRECT`/`DIRECTHL`), `PATH3` (`DMAC D2` / `0x1000_6000`). **`TODO(verify)`**: whether `PATH1` has any mask bit outside `PSE` (`ps2tek` `#gifio` lists none).
- **GIFtag parsing & formats**:
  - `NLOOP` (`bits 0..=14`), `EOP` (`bit 15`), `PRE` (`bit 46`), `PRIM` (`bits 47..=57`), `FLG` (`bits 58..=59`: `0=PACKED, 1=REGLIST, 2|3=IMAGE`), `NREG` (`bits 60..=63`, `0 => 16`), `REGS` (`bits 64..=127`, 4 bits per reg).
  - `NLOOP == 0`: ignores all fields other than `EOP` (does not initialize GS `Q = 1.0f` or send `PRE`/`PRIM`).
  - `NLOOP > 0`: initializes GS internal `Q = 1.0f` (`0x3F80_0000`) and, if `PRE == 1`, sends `PRIM` to internal GS `0x00` before payload (`FLG != IMAGE` or all non-`NLOOP==0` tags per `ps2tek` `#giftags`).
  - `PACKED` (`FLG=0`): implements all 16 register descriptors (`0x0=PRIM`, `0x1=RGBAQ`, `0x2=ST`, `0x3=UV`, `0x4=XYZF2`, `0x5=XYZ2`, `0x6=TEX0_1`, `0x7=TEX0_2`, `0x8=CLAMP_1`, `0x9=CLAMP_2`, `0xA=FOG`, `0xC=XYZF3`, `0xD=XYZ3`, `0xE=A+D`, `0xF=NOP`) per `ps2tek` `#gifdataformats` (including `0x2 ST` storing float `Q` from bits `64..95` into internal `Q`, `0x1 RGBAQ` injecting internal `Q` into `RGBAQ[32..63]`, `0x4`/`0x5` ADC from bit 111 / bit 15 of word 3).
  - `REGLIST` (`FLG=1`): 2 x 64-bit writes per QWC; when total writes `NREG * NLOOP` is odd, the upper 64 bits of the last QWC are discarded (`ps2tek` `#gifdataformats`).
  - `IMAGE` (`FLG=2|3`): 2 x 64-bit `HWREG` (`0x54`) writes per QWC.
- **`DMAC D2` (`GIF`, `0x1000_A000..=0x1000_A030`) & `GIF_FIFO` (`0x1000_6000..=0x1000_6FFF`)**:
  - `D2` transfers 1 QWC per `BUSCLK` (`2` EE cycles) on scheduler ticks (`DmacTick`) in `Normal` and `Chain` (`DIR=1`, EE->GIF) modes, pausing automatically when the 16-QWC PATH3 FIFO is full (preserving backpressure even when PATH3 is masked via `M3R`/`M3P`).
  - `DMAC D2` with `CHCR.DIR == 0` (to memory) or `CHCR.MOD == 2` (`Interleave`), or writes/reads to `GIF_FIFO` while `GS_BUSDIR == 1`, return an explicit unimplemented error (`TODO(verify)`).
- **Scheduler-driven bounded GIF execution**: `step_gif` advances at most 1 QWC / unit per `GifTick` (`2` EE cycles per BUSCLK beat); register writes never synchronously drain a multi-QWC packet or DMA, and register reads never mutate or advance GIF state.

---

## 12. VU1 Elementary Function Unit (`EFU`, `crates/ps2-core/src/vu/efu.rs`)

Implemented for `VU1` (`efu.elf`: **`184 / 208`** vectors matching across all 13 instructions; `vu/lower/efu.elf` remains `DEFERRED` in `DEFERRED_GATE_SUITES` with reason `"partial: 184/208 vectors; see HARDWARE_NOTES.md EFU section"` until `208/208` match). Pipeline latencies (`11..=54` cycles) and `P` destination behavior come from `local/ps2tek/index.html` (`#vu`, `#vupipelining`, `FETCHED`). Polynomial formulas, evaluation orders, and numeric constants originate from `Play!` (BSD-2-Clause, see `THIRD_PARTY_NOTICES.md`, `Source/ee/MA_VU_Lower.cpp`, `FETCHED`).

Non-sourced deviations discovered against `efu.expected` are explicitly labeled **`inferred; may not generalize`**:
- `vu_mul_rtz` (truncating 24x24 multiply without EE COP1's Radix-4 Booth `-1` subtraction) paired with `x^2` odd-power stepping (`eval_odd_series_vu_rtz`) in `ESIN`, `EATAN`, `EATANxy`, and `EATANxz`.
- `EATAN_PI_OVER_4 = 0x3F49_0FDA` (mathematical `pi / 4` truncated toward zero per `ps2tek` float rules, rather than `Play!`'s IEEE-RNE `0x3F49_0FDB`).
- Real EFU hardware exhibits ~`±0.8` ULP non-monotonic approximation error in `ERCPR`/`ESQRT`/`ERSQRT` (LUT/Newton-Raphson) and ~`1..5` ULP internal accumulator width differences in `EEXP`, `ESIN`, and `EATAN` (`t = -1`). Where no general sourced rule explained remaining mismatches within 5 cycles, tuning was stopped and marked `PARTIAL`.

| Instruction | Opcode | Status | Matched | Source / Attribution | Diverging Vectors (`efu.expected`) & What Was Tried |
|---|---|---|---|---|---|
| `ESADD` | `0x73C` | `match` | `16 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) + `ps2float` | None. Cycle 1 (`x^2 + (y^2 + z^2)` in `ps2float`) passed `16/16`. |
| `ESUM` | `0x77E` | `match` | `16 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) + `ps2float` | None. Cycle 1 (`x + (y + (z + w))` in `ps2float`) passed `16/16`. |
| `ERCPR` | `0x7BE` | `partial` | `14 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) + `ps2float::div` (`ps2tek`, `FETCHED`) | `CVF_GARBAGE2` (`a03c98eb` vs `a03c98ea`), `CVF_3PI_OVER2` (`3e594cb0` vs `3e594caf`). Cycle 1 `ps2float::div(1.0, x)`: hardware `3PI_OVER2` (+0.797 ULP error from exact) vs `ps2float` (+0.203) shows undocumented LUT/approximation rounding; stopped. |
| `ESQRT` | `0x7BC` | `partial` | `15 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) + `ps2float::sqrt` (`ps2tek`, `FETCHED`) | `CVF_PI` (`3fe2dfc4` vs `3fe2dfc5`). Cycle 1 `ps2float::sqrt`: `sqrt(PI)` rounds down (-0.76 ULP) while `3PI_OVER2` (+0.08 ULP) and `MAX_MANTISSA` (+0.15 ULP) round up cleanly; undocumented hardware `sqrt` LUT/approx; stopped. |
| `ERSQRT` | `0x7BD` | `partial` | `14 / 16` | `ps2tek` `ps2float::rsqrt` (`FETCHED`); `Play!` `MA_VU_Lower.cpp` (`FETCHED`) | `CVF_PI_OVER2` (`3f4c4229` vs `3f4c422a`), `CVF_PI` (`3f106ebb` vs `3f106eba`). Cycle 1 `ps2float::rsqrt(1.0, x)` (`14/16`); Cycle 2 `ercpr(esqrt(x))` (`14/16`, identical 2 misses). Kept Cycle 1 and stopped. |
| `ELENG` | `0x73E` | `partial` | `15 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) (`esqrt(esadd(lanes))`) | `CVF_INCREASING` (`406f7750` vs `406f7751`). Cycle 1 `esqrt(esadd)`: exact `sqrt(14)` is +0.54 ULP above `406f7750`; same non-monotonic EFU `sqrt` approx as `ESQRT`; stopped. |
| `ERLENG` | `0x73F` | `partial` | `13 / 16` | `ps2tek` (`ersqrt(esadd)`, `FETCHED`); `Play!` `MA_VU_Lower.cpp` (`FETCHED`) | `CVF_PI_OVER2` (`3ebc2fda` vs `3ebc2fdb`), `CVF_PI` (`3e3c2fda` vs `3e3c2fdb`), `CVF_3PI_OVER2` (`3dfaea7a` vs `3dfaea79`). Cycle 1 `ersqrt(esadd)` (`13/16`); Cycle 2 `ercpr(eleng)` (`13/16`, identical ±1 ULP diffs). Kept Cycle 1 and stopped. |
| `ERSADD` | `0x73D` | `match` | `16 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) (`ercpr(esadd(lanes))`) | None. Cycle 1 (`ercpr(esadd(lanes))`) passed `16/16`. |
| `EEXP` | `0x7FE` | `partial` | `10 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) `EEXP_TERMS` (`c_1..c_6`) + `ps2float` | `CVF_MAX_MANTISSA`/`CVF_DECREASING` (`3e0a9554` vs `3e0a9553`), `CVF_ONE` (`3ebc5abf` vs `3ebc5abc`), `CVF_NEGONE` (`402df0aa` vs `402df0a5`), `CVF_INCREASING` (`3d4bed7f` vs `3d4bed7a`), `CVF_PI` (`3d310121` vs `3d31011c`). Cycle 1 `Play!` `ApplySumSeries` + `p*(p*(p*p))` + `ps2float::div` (`10/16`); Cycle 2 Horner (`8/16`); Cycle 3 `(1 / base)^4` (`8/16`). Kept Cycle 1; stopped. |
| `ESIN` | `0x7FC` | `partial` | `12 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) `ESIN_TERMS`; `vu_mul_rtz` + `x^2` odd-power stepping (`inferred; may not generalize`) | `CVF_MAX_MANTISSA` (`3f68c816` vs `3f68c814`), `CVF_ONE` (`3f576aa4` vs `3f576aa3`), `CVF_NEGONE` (`bf576aa4` vs `bf576aa3`), `CVF_DECREASING` (`3f68c815` vs `3f68c814`). Cycle 1 `Play!` + `ps2float` (`8/16`); Cycle 2 `vu_mul_rtz` (`11/16`); Cycle 3 reverse sum (`7/16`); Cycle 4 0-guard-bit add (`9/16`); Cycle 5 `vu_mul_rtz` + `x^2` stepping + `ps2float::add` (`12/16`, matches `CVF_PI` `3b565de0`). Stopped at 5 cycles. |
| `EATAN` | `0x7FD` | `partial` | `11 / 16` | `(z-1)/(z+1)`: `mathematical identity; consistent with ONE -> 3f490fda`; `Play!` `EATAN_TERMS` (`FETCHED`); `0x3F49_0FDA` / `vu_mul_rtz` / `t^2` stepping: `inferred; may not generalize` | `CVF_ZERO`/`CVF_NEGZERO`/`CVF_MIN_EXP`/`CVF_GARBAGE1` (`33e60000` vs `34000000`), `CVF_3PI_OVER2` (`3fae4be7` vs `3fae4be8`). Cycle 1 `Play!` `0x3F49_0FDB` (`4/16`); Cycle 2 `0x3F49_0FDA` RTZ (`11/16`); Cycle 3 `vu_mul_rtz` + `t^2` stepping (`11/16`); Cycle 4 `vu_div_rtz` (`12/16`, broke `EATANxz` `CVF_DECREASING` -> reverted to Cycle 3). `0x33e6_0000` reflects sub-`2^-24` (`2^-31`) EFU internal precision (`inferred; may not generalize`); stopped. |
| `EATANxy` | `0x77C` | `match` | `16 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) `(y-x)/(y+x)` + `EATAN_TERMS`; `0x3F49_0FDA` / `vu_mul_rtz` / `t^2` stepping: `inferred; may not generalize` | None. Reached `16/16` at Cycle 2; preserved under Cycle 3 (`vu_mul_rtz` + `t^2` stepping); reverted Cycle 4 `vu_div_rtz` to keep `EATANxz` at `16/16`. |
| `EATANxz` | `0x77D` | `match` | `16 / 16` | `Play!` `MA_VU_Lower.cpp` (`FETCHED`) `(z-x)/(z+x)` + `EATAN_TERMS`; `0x3F49_0FDA` / `vu_mul_rtz` / `t^2` stepping: `inferred; may not generalize` | None. Reached `16/16` at Cycle 2; preserved under Cycle 3; Cycle 4 `vu_div_rtz` broke `CVF_DECREASING` (`3eed6339` vs `3eed633a`) -> reverted to Cycle 3 (`16/16`). |

---

## 13. Graphics Synthesizer (`crates/ps2-gs`) — Phase 2b Step 1 (Registers, 4 MB VRAM Swizzling & Transfers)

Implemented in `crates/ps2-gs` (depends only on `ps2-common`, `bitflags`, `thiserror`, `tracing`; `#![forbid(unsafe_code)]`) and wired into `crates/ps2-core::bus::GsPrivilegedStub::core` while keeping `SIGNAL`/`FINISH`/`LABEL`/`CSR`/`IMR`/`SIGLBLID` unchanged in `GsPrivilegedStub`. See `docs/GS_GUESSES.md` for all `GUESS` / `SEARCH-ONLY` items (`G-001`..`G-006`) and `docs/GS_PACKET_FORMAT.md` for `.gsp` scripts.

### 13.1 `FETCHED` Sources Used in Step 1

- **`local/ps2tek/index.html`** (`#gs`, `#gsprimitives`, `#gsvertexattributes`, `#gsframeandzbuffers`, `#gstransfers`, `#gstextures`, `#gsfog`, `#gsalphablending`, `#gstestsandpixelcontrol`, `#giftags`, `#gifdataformats`, `FETCHED`): internal register indices (`0x00..=0x62`), GIFtag/`PACKED`/`REGLIST`/`IMAGE` unpacking, `% 2048` transfer coordinate wrap, equal-bpp requirement for `TRXDIR=2`, and `TRXPOS.DIR` `0..=3`.
- **`Play!`** (`jpd002/Play-`, BSD-2-Clause, commit `1f27232772e484bfb79364b751b84f41f4c87481`, date read `2026-11-15`, `FETCHED` — see `THIRD_PARTY_NOTICES.md`):
  - `Source/gs/GsPixelFormats.h`, `Source/gs/GsPixelFormats.cpp`: page dimensions and block/column/word swizzle tables (`PSMCT32/24`, `PSMCT16/16S`, `PSMZ32/24`, `PSMZ16/16S`, `PSMT8`, `PSMT4`, `PSMT8H`, `PSMT4HL`, `PSMT4HH`).
  - `Source/gs/GSHandler.h`, `Source/gs/GSHandler.cpp`, `Source/gs/GSH_OpenGL/GSH_OpenGL.cpp`, `Source/gs/GSH_OpenGL/GSH_OpenGL_Texture.cpp`: register field layouts, `PSMT4HL`/`PSMT4HH` nibble offsets, `TEX2`->`TEX0` mask, `ALPHA.FIX`, `FOG.F`, and `PSMCT24` upper-byte preservation.
- **`ps2dev/ps2sdk`** (`AFL-2.0`, commit `b04e8973c8a9839d0653d87a8d30cc67fd3c3f05`, date read `2026-11-15`, `FETCHED` — see `THIRD_PARTY_NOTICES.md`):
  - `common/include/gs_gp.h`, `common/include/gs_privileged.h`, `common/include/gs_psm.h`, `ee/libgs/include/libgs.h`: cross-checked GP register bit layouts and PCRTC privileged display registers (`PMODE`, `SMODE2`, `DISPFB1/2`, `DISPLAY1/2`, `BGCOLOR`).

### 13.2 Cross-Source Discrepancies Resolved (`FETCHED`)

1. **`FOG` (`0x0A`) bit position**: `ps2tek` `#gsfog` says bits `0..=7`, whereas `ps2tek` `XYZF2`, `Play! GSHandler.cpp`, and `ps2sdk gs_gp.h` / `libgs.h` place 8-bit `F` in bits **`56..=63`**. Implemented bits `56..=63`.
2. **`ALPHA_1/2` (`0x42/0x43`) `FIX` field**: `ps2tek` `#gsalphablending` says bits `8..=15`, whereas `Play! GSHandler.h` and `ps2sdk gs_gp.h` / `libgs.h` place `FIX` in bits **`32..=39`**. Implemented bits `32..=39`.
3. **`DIMX` (`0x44`) element width**: `ps2sdk common/include/gs_gp.h` masks each matrix entry with `& 0x3` (2 bits, typo), whereas `ps2sdk ee/libgs/include/libgs.h` defines 16 signed **3-bit** entries (`-4..=+3`) at bit offsets `0, 4, 8, ..., 60`. Implemented 3-bit two's-complement entries (`(raw3 << 5) >> 5`).
4. **`PSMT4HL` (`0x24`) vs `PSMT4HH` (`0x2C`)**: `ps2sdk common/include/gs_psm.h` comment swaps descriptions (`4HL` `"28 to 32"`, `4HH` `"24 to 27"`), whereas `Play! GSHandler.cpp` & `GSH_OpenGL_Texture.cpp` confirm `PSMT4HL = bits 24..=27` and `PSMT4HH = bits 28..=31`. Followed `Play!`.

---

## 14. Graphics Synthesizer (`crates/ps2-gs`) — Phase 2b Step 2 (Non-Textured Rasterizer & `ps2-gs-runner`)

### 14.1 Single-Source Coverage (`crates/ps2-gs/src/coverage.rs`)

Pixel center, top-left fill, sprite boundary, point snap, and Bresenham line-endpoint rules live in a single function (`coverage::is_pixel_covered`), switched globally via one constant (`coverage::ACTIVE_COVERAGE: CoverageConvention`, defaulting to `CoverageConvention::IntegerCenter` with `IntegerCenterHalfOpenLine` and `HalfPixelCenterHalfOpenLine` available for Step 4 A/B comparison without touching `raster.rs`).

Verbatim statements from Hans-Kristian Arntzen (`themaister`), *"PlayStation 2 GS emulation – The Final Frontier of Vulkan Compute Emulation"* (2024-07-03, `https://themaister.net/blog/2024/07/03/playstation-2-gs-emulation-the-final-frontier-of-vulkan-compute-emulation/`, `FETCHED`):
- `"Triangles are top-left raster, just like modern GPUs."`
- `"Pixel center is on integer coordinate, just like D3D9."`
- `"Lines use Bresenham's algorithm"`
- `"Points snap to nearest pixel. Unsure which rounding is used though ..."`
- `"Sprites are simple quads with two coordinates."`

Unconfirmed sub-pixel rounding, sprite corner normalization, Bresenham endpoint inclusion, attribute interpolation truncation, `ZTE=0` handling, and Z-format bit masks are catalogued in `docs/GS_GUESSES.md` (`G-009`..`G-015`).

### 14.2 Primitives, Depth, Framebuffers & Dumper (`crates/ps2-gs/src/raster.rs`, `crates/ps2-gs-runner`)

- **Vertex assembly & shading** (`Play! Source/gs/GSH_OpenGL/GSH_OpenGL.cpp`, BSD-2-Clause, `FETCHED`): `PRIM` writes clear the vertex queue while `PRMODE`/`PRMODECONT` updates do not (`GsRegs::effective_prim()`); `XYZ2`/`XYZF2` trigger a drawing kick when the primitive's vertex count is met while `XYZ3`/`XYZF3` (`ADC`) advance strip/fan queues without drawing; flat shading (`IIP=0`) takes the final vertex color (`v1` for lines, `v2` for triangles); `Sprite` takes both flat color and depth `Z` from `v1`.
- **Coordinates, clipping & pixel tests** (`local/ps2tek/index.html`, `FETCHED`): `XYOFFSET_1/2` 12.4 fixed-point offsets are subtracted from `XYZ`/`XYZF`; `SCISSOR_1/2` clips inclusively to `[scax0..=scax1, scay0..=scay1]`; `TEST_1/2` implements `ZTE` and `ZTST` (`0=NEVER`, `1=ALWAYS`, `2=GEQUAL`, `3=GREATER`) alongside `ZBUF_1/2` (`ZMSK`, `PSMZ32`/`24`/`16`/`16S`).
- **Framebuffer write masking & formats** (`local/ps2tek/index.html`, `FETCHED`; `Play! GSH_OpenGL_Texture.cpp`, `FETCHED`): `FRAME.FBMSK` is applied in 32-bit RGBA space prior to format conversion (`PSMCT32`, `PSMCT24` preserving upper 8 bits `24..=31`, `PSMCT16`/`16S` packing `R[0..4]`, `G[5..9]`, `B[10..14]`, `A[15]` with mask bit 63 controlling alpha bit 15). Nearest texturing (`TME`) is implemented in Step 3a (§16); fog/alpha-blending/AA/alpha-test/dithering (`FGE`/`ABE`/`AA1`/`ATE`/`DATE`/`DTHE`/`COLCLAMP`/`FBA`) remain deferred to Step 3b/3c.
- **Standalone dumper (`crates/ps2-gs-runner`)**: executes `.gsp` packet streams headless via `ps2-gs` and exports PNG images (defaulting to `local/gs_out/`), supporting `--rect FBP,FBW,PSM,W,H` or `.gsp` `surface` directives (`9/9` frozen Step 2 suites; `16/16` with Step 3a in `=== GS RENDER GATE ===`).

---

## 15. Observed in PCSX2 (not hardware)

> [!NOTE]
> The measurements below are black-box comparisons against PCSX2, recorded for differential diagnosis prior to hardware/GS-dump verification. **PCSX2 is an emulator, not PS2 hardware**: matching PCSX2 raises confidence in conventional interpretations, while discrepancies remain open hypotheses until corroborated across additional probes or real PS2 hardware (`docs/GS_GUESSES.md`).

- **Reference setup (2026-11-17)**: PCSX2 `v2.8.2`, GS renderer `Software`, `Deinterlacing: No Deinterlacing`, `Bilinear Filtering: None`, `Anti-Blur` off, no post-processing, internal-resolution `320x240` screenshots booted via `Start File` from `local/pcsx2/{04,05,06,08,10}.elf`. Full diff report saved in `local/pcsx2/results.txt` (untracked).
- **Display & exact-match scenes (`04`, `05`, `06`)**:
  - `04_sprite_flat.gsp`, `05_triangle_flat.gsp`, `06_triangle_gouraud.gsp`: **`76800/76800` (`100.00%`) exact pixel match** (`pm1=0`, `gt1=0`, `max_delta=(0,0,0)`).
  - Confirms our no-syscall ELF stub (`local/pcsx2/*.elf`) + PCRTC setup (`G-016`: `PMODE=0xFFA5`, `SMODE1=0x7_4080_4504`, `SMODE2=0`, `DISPFB1=0xA00`, `DISPLAY1=0xE_F9FF_0381_927C`, `BGCOLOR=0`) yields unscaled `320x240` output with `A=0x80` pixels undimmed.
- **Scene `10_subpixel_probe.gsp` (`76800/76800` after adopting `IntegerCenterHalfOpenLine`; `76794/76800` before)**:
  - **Adopted convention (`G-009`, `G-011`)**: `ACTIVE_COVERAGE = CoverageConvention::IntegerCenterHalfOpenLine` — **adopted: matches PCSX2 v2.8.2 on scene 10; hardware unconfirmed** (`HalfPixelCenterHalfOpenLine` fails `4` Points `P2,P3,P5,P6` and `1` Sprite `S2`; added to `--gate` under `"PCSX2-observed (v2.8.2), not hardware"`).
  - **Points (`7/7` match)**: PCSX2 snapped `P2 (52.5, 20.0) -> (53, 20)` and `P5 (52.0, 40.5) -> (52, 41)` (half-up integer-center rounding, `G-009`).
  - **Sprites (`3/3` match)**: PCSX2 rendered `S0`, reversed-corner `S1` (`[50..57] x [150..157]`), and subpixel `S2` (`[81..84] x [150..153]`, excluding `x=80`, `G-010`).
  - **Lines (`6/6` match under `IntegerCenterHalfOpenLine`; `0/6` under `IntegerCenter`)**: PCSX2 omitted all `6` final endpoints (`(28,80)`, `(50,80)`, `(90,84)`, `(28,114)`, `(54,110)`, `(80,110)`), including reversed horizontal `L1`, steep negative-`dY` `L4`, and zero-length `L5` (`G-011`), while matching `L3` half-step tie-breaks (`(21,111)` etc.). Fixture `08`'s guide `LineStrip` `(39,29)->(280,29)->(280,210)->(39,210)->(39,29)` is a closed loop (`v4 == v0`), so `0` border pixels changed.
- **Scenes `08`, `11`, `12`, `13` (`G-012` Gouraud & Z Interpolation — 1-Round Analysis)**:
  - **Scene `13_gouraud_z_probe.gsp` (`76800/76800`, `100.00%` exact)**: Both small `Z` (`1000..1017` across `W=281`, `dZ/dx=17/281`) and high-bit `Z` (`0x0100_0000..0x0100_0013`, where IEEE `f32` ULP is `2.0`) matched PCSX2 `v2.8.2` Software at every pixel under `ZTST=GEQUAL`.
  - **Fractional-slope Gouraud RGB diffs (`gt1=0`, `max_delta=(1,1,1)` / `(1,1,0)`)**: `08` (`3049/28008` tri px = `96.03%` full frame), `11` (`2453` total = `2439/15197` wide + `14/945` narrow = `96.81%`), `12` (`4518/21537` tri px = `94.12%`). All mismatches lie inside triangles and differ by at most `±1` LSB.
  - **Verified sign rule (`pcsx2 - ours`)**: PCSX2 lags exact rational floor in the direction of horizontal travel (`dC/dx < 0 => +1`, `dC/dx > 0 => -1`, `dC/dx == 0 => 0`):
    - `08`: `R` (`dR/dx=-0.40506 < 0`): `-1: 0, 0: 26034, +1: 1974` (`0` viol); `G` (`+0.50633 > 0`): `-1: 1199, 0: 26808, +1: 1` (`1` viol at `(68,150)` exact `126.997553`); `B` (`0`): `-1: 0, 0: 28008, +1: 0` (`0` viol).
    - `11_wide`: `R` (`-0.77199 < 0`): `-1: 2, 0: 14423, +1: 772` (`2` viol); `G` (`+0.76873 > 0`): `-1: 1785, 0: 13412, +1: 0` (`0` viol); `B` (`0`): `0` viol.
    - `11_narrow`: `R` (`-11.73684 < 0`): `-1: 0, 0: 942, +1: 3` (`0` viol); `G` (`+11.84211 > 0`): `-1: 4, 0: 941, +1: 0` (`0` viol); `B` (`+5.52632 > 0`): `-1: 7, 0: 938, +1: 0` (`0` viol).
    - `12_slanted`: `R` (`-0.76003 < 0`): `-1: 0, 0: 21145, +1: 392` (`0` viol); `G` (`+0.82754 > 0`): `-1: 2075, 0: 19462, +1: 0` (`0` viol); `B` (`-0.88839 < 0`): `-1: 0, 0: 19010, +1: 2527` (`0` viol).
  - **Mismatch rate vs span distance `(px - min_x)`**: grows monotonically along unclipped spans — `11_wide`: `2.43%` (`0..29`), `7.23%`, `11.94%`, `16.74%`, `21.14%`, `25.33%`, `30.49%`, `37.61%`, `39.02%` (`240..269`), `35.50%` (`270..299`), `25.00%` (`1/4` at `300..306`); `11_narrow` (`W<=19`): `1.48%` (`14/945`); `12`: `4.88%` (`0..29`) to `52.74%` (`180..209`) and `100%` (`1/1` at `210`). On `08`, `SCISSOR_1.SCAX0=40` clips the left edge on rows `y=181..209` (`x_tri_left` drops from `40.50` to `24.08`, shifting `min_x` to `40` by up to `~16` px relative to the unclipped triangle edge), giving `1.91%` (`0..29`) to `26.17%` (`210..239`).
  - **Reproducible analysis & candidate model validation (`08`-parameterized, evaluated unchanged on `06, 11, 12`)**: For any triangle with vertices `(x_k, y_k, C_k)` inside `[min_y..=max_y]` and row span `[min_x..=max_x]`, exact gradients are computed via the 2D planar determinant `dC/dx = (dc1*dy02 - dc2*dy01) / area`, `dC/dy = (dc2*dx01 - dc1*dx02) / area` and compared per pixel across models parameterized strictly on Scene `08`:
    - `M0_exact_floor` (retained in `ps2-gs`): `06=0, 08=3049, 11=2453 (2439+14), 12=4518`
    - `M1_exact_start_trunc_Q8`: `floor(C(min_x,y)*2^8) + dx*trunc(dC/dx*2^8)` -> `06=0, 08=8584, 11=5201 (5169+32), 12=5309`
    - `M1_exact_start_trunc_Q9`: `floor(C(min_x,y)*2^9) + dx*trunc(dC/dx*2^9)` -> `06=0, 08=191, 11=76 (69+7), 12=157`
    - `M1_exact_start_trunc_Q10`: `floor(C(min_x,y)*2^10) + dx*trunc(dC/dx*2^10)` -> `06=0, 08=191, 11=1550 (1543+7), 12=2836`
    - `M2_edge_start_trunc_Q9`: left-edge start + `trunc((min_x-x_edge)*dC/dx*2^9)` -> `06=116, 08=198, 11=82 (76+6), 12=180`
    - `M2_edge_start_trunc_Q10`: same in `Q10` -> `06=116, 08=197, 11=1549 (1543+6), 12=2811`
    - `M2b_plane_step_yx_Q9`: integer Y/X accumulation from `v0` in `Q9` -> `06=7080, 08=4381, 11=2689 (2517+172), 12=3202`
    - `M3a_simd4_Q7_min_x` (fits `6960/6960` `min_x%4==0` rows on `08`; may not generalize): `06=0, 08=124, 11=62 (57+5), 12=125`
    - `M3b_simd4_Q7_align4` (`align4 = min_x & !3`; may not generalize): `06=0, 08=192, 11=117 (114+3), 12=239`
    - `M4_f32_iter_min_x`: `C(min_x,y) as f32` with per-px `+= dC/dx as f32` -> `06=0, 08=3043, 11=2453 (2439+14), 12=4518`
    - `M4b_f32_plane_trunc`: `floor(c0 + dC/dx*(x-x0) + dC/dy*(y-y0))` in `f32` -> `06=0, 08=3049, 11=2454 (2440+14), 12=4518`
    - No tested model achieves `0` diffs across `{06, 08, 11, 12}` (`Q7` aligned-row fit leaves `124–192` unaligned diffs on `08` and fails `11`/`12`); exact rational floor is retained in `ps2-gs` with this documented `±1` LSB difference (`G-012`).
  - **Step 3 `UV` & `ST/Q` note**: Truncated fixed-point stepping of non-perspective `UV` (`FST=1`) would lag identically near integer texel boundaries on wide irrational-slope triangles, whereas perspective `ST/Q` (`FST=0`) divides `S/Q, T/Q` in `f32` per pixel; axis-aligned sprites with exact texel steps avoid drift. Test with two wide (`W=281`) contrasting-texel triangles (`dU/dx=13/281` `UV`, `Q=1.0..3.0` `ST/Q`).
- **Step 3a Textured-Sprite Checkpoint Scenes (`3a_psmct32_modulate`, `3a_psmt8_csm1`, `3a_psmt4hl_hh`)**:
  - **PCSX2 `v2.8.2` settings (2026-11-18)**: `Software` renderer, `Deinterlacing: No Deinterlacing`, `Bilinear Filtering: None`, `Anti-Blur` off, `Auto Flush` on, `Software Rendering Threads: 3`, `Texture Filtering: Nearest`, `Mipmapping` off, no post-processing, screenshots `Internal Resolution (No Aspect Correction)`, PNG (`320x240`).
  - **Prediction timing (not blind)**: Commit `af3ffbf` (`2026-11-17T21:24:04+08:00`) added `verify_10..12` in `crates/ps2-autotest/src/gs_gate.rs` prior to the `local/pcsx2/3a_*_pcsx2.png` file timestamps (`LastWriteTime` `2026-11-18 21:19:01..21:21:13`, `CreationTime` `2026-11-18 21:21:27`), but no standalone prediction-table commit in `docs/GS_GUESSES.md` was made before generating `3a_*_ours.png` (`2026-11-17 21:22:21`) or committing the rasterizer. These three comparisons were **not blind**.
  - **Results (`100.00%` exact on all three; added to `--gate` under `"PCSX2-observed (v2.8.2), not hardware"`)**:
    - `3a_psmct32_modulate` (`tests/gs_render/10_tex_psmct32_uv_modulate.gsp`): **`76800/76800` (`100.00%`) exact** (`pm1=0, gt1=0, max_delta=(0,0,0), mae=0.0000, rmse=0.0000`). Tests `PSMCT32` `HOST->LOCAL` swizzle (`G-007`), `Sprite` `PRIM.FST=1` `UV` (`1x` and `2x` exact texel steps, `G-019`), and `TFX=0` (`Modulate`) with `TCC=1` (`G-020`).
    - `3a_psmt8_csm1` (`tests/gs_render/11_tex_psmt8_csm1_swizzle.gsp`): **`76800/76800` (`100.00%`) exact** (`pm1=0, gt1=0, max_delta=(0,0,0), mae=0.0000, rmse=0.0000`). Tests `PSMT8` page/block/column swizzle (`G-007`), `CSM1` 8-bit `PSMCT32` CLUT `0x08 <-> 0x10` entry swizzle across distinct entries `8..=23`, `CLD=1` reload, and `TFX=1` (`Decal`) with `TCC=1` (`G-020`, `G-021`).
    - `3a_psmt4hl_hh` (`tests/gs_render/12_tex_psmt4hl_hh.gsp`): **`76800/76800` (`100.00%`) exact** (`pm1=0, gt1=0, max_delta=(0,0,0), mae=0.0000, rmse=0.0000`). Confirms `G-008` (`PSMT4HL = bits 24..=27` green/blue, `PSMT4HH = bits 28..=31` magenta/yellow, consistent with PCSX2 `v2.8.2` and `Play!`, differing from the `ps2sdk` `gs_psm.h` comment), `CSM1` 4-bit CLUT lookup (`G-007`, `G-021`), and `TEX2_1` format switch with `CLD=0` cache retention (`G-005`, `G-021`).

---

## 16. Graphics Synthesizer (`crates/ps2-gs`) — Phase 2b Step 3a (Nearest Texturing & CLUT)

### 16.1 `FETCHED` Behavior Implemented (`crates/ps2-gs/src/texture.rs`, `raster.rs`)
- **Texture coordinates & `TEX0`/`TEX2` (`local/ps2tek/index.html` `#gstextures`, `FETCHED`; `Play!` `commit 1f27232` `BSD-2`, `FETCHED`)**:
  - `PRIM.FST = 1` (`UV` `12.4` unsigned fixed-point, rounded/stepped by `div_euclid(16)`) and `PRIM.FST = 0` (`ST/Q` perspective coordinates with `U = (S/Q)*TW`, `V = (T/Q)*TH`, `floor()` to integer texel).
  - `Sprite` (`PRIM=6`) preserves swapped vertex corner pairing (`left_v`/`right_v` for `U`/`S`, `top_v`/`bot_v` for `V`/`T`) while using provoking vertex `v1` for color & `Z`.
  - `TEX2_1`/`TEX2_2` updates the non-base bits of `TEX0` (`PSM`, `CBP`, `CPSM`, `CSM`, `CSA`, `CLD`) without overwriting `TBP0`/`TBW`/`TW`/`TH`/`TCC`/`TFX`.
- **`CLAMP` modes (`WMS`/`WMT`) & `TEXA`/`AEM`**:
  - `0 = REPEAT` (`u & (size - 1)`), `1 = CLAMP` (`clamp(0, size - 1)`), `2 = REGION_CLAMP` (`clamp(MIN, MAX) & (size - 1)`), `3 = REGION_REPEAT` (`((u & MIN) | MAX) & (size - 1)`).
  - `PSMCT24` expands alpha with `TEXA.AEM` (`AEM=1 && RGB==0 -> A=0`, else `TA0`). `PSMCT16`/`16S` expands `5->8` via `<< 3` (`G-014` convention) and sets alpha to `0` (`AEM=1 && RGB==0`), `TA0` (`A1=0`), or `TA1` (`A1=1`).
- **Paletted formats & `CLUT` cache (`ClutCache`)**:
  - Supports `PSMCT32`, `PSMCT24`, `PSMCT16`, `PSMCT16S`, `PSMT8`, `PSMT4`, `PSMT8H`, `PSMT4HL`, and `PSMT4HH` (sharing upper bits `[24..27]` / `[28..31]` of 32-bit pixels with framebuffer formats).
  - `CSM1` 8-bit CLUT reads `256` entries applying the hardware `0x08 <-> 0x10` entry index swizzle (`Play! GSHandler.cpp::MakeLinearCLUT`, `FETCHED`); `CSM2` reads linear strip starting at `(TEXCLUT.COU*16, TEXCLUT.COV)` with `TEXCLUT.CBW` in `PSMCT16`. `CLD 0..=5` controls `CBP0`/`CBP1` conditional cache reload on `TEX0`/`TEX2` writes.
- **`TFX` combiner & `TCC`**: Implements `Modulate` (`0`, `(Cv*Ct)>>7` saturating at `255`), `Decal` (`1`, `Ct`), `Highlight` (`2`, `+ Av`), and `Highlight2` (`3`, `+ Av` RGB, `At` when `TCC=1`) with `TCC=0` forcing vertex alpha `Av`.
- **Gate & ELF Verification**: Suites `10..=16` added to `tests/gs_render/`, verified in `crates/ps2-autotest/src/gs_gate.rs` (`GS_RENDER_GATE_SUITES`, `16/16`) and `crates/ps2-autotest/src/gs_elf_gate.rs` (`STEP3A_GS_ELF_SUITES`, `16/16` `EE -> GIF -> GS` round-trip). `G-014` updated (`TME` implemented); new no-source gaps registered as `G-017`..`G-021` (`GUESS`) in `docs/GS_GUESSES.md`.

### 16.2 Step 3a PCSX2 Checkpoint & Planned Textured-Triangle Probe
- **Current Step 3a PCSX2 checkpoint scenes (`tests/gs_render/10..12`, sprites `PRIM=0x0116` only, `1x`/`2x` exact UV steps; no textured triangles)**:
  - `local/pcsx2/3a_psmct32_modulate.{elf,_ours.png}` (`10_tex_psmct32_uv_modulate.gsp`): `PSMCT32` `UV` `1x`/`2x` + `TFX=0 Modulate` (`G-019`, `G-020`).
  - `local/pcsx2/3a_psmt8_csm1.{elf,_ours.png}` (`11_tex_psmt8_csm1_swizzle.gsp`): `PSMT8` + `CSM1` `16x2` CLUT with entries `8..15` (green/cyan) vs `16..23` (orange/red) testing `0x08 <-> 0x10` swizzle (`G-021`).
  - `local/pcsx2/3a_psmt4hl_hh.{elf,_ours.png}` (`12_tex_psmt4hl_hh.gsp`): `PSMT4HL` green / `PSMT4HH` magenta pair via `TEX0_1` + `TEX2_1` (`G-008`, `G-021`).
- **Planned post-decision textured-triangle probe (held until after interpolation decision / Step 3b-3c)**:
  - **What it will test**: two wide prime-width (`W=281`, contrasting 16- or 32-texel multi-color stripe texture) triangles rendered with `TFX=1 (Decal)`:
    1. **Linear `UV` (`FST=1`) triangle** with irrational prime-denominator step `dU/dx = 13/281`: tests whether PCSX2 Software steps `U`/`V` with truncated fixed-point increments that lag near integer texel boundaries in the same direction as Gouraud `RGBA` (`08`/`11`/`12`, `§15`), vs exact rational floor.
    2. **Perspective `ST/Q` (`FST=0`) triangle** with `Q` varying across `1.0..=3.0`: tests per-pixel `S/Q` & `T/Q` perspective division fidelity, non-linear texel boundary shifts along the span (distinct from linear interpolation), and near-zero `Q` fallback stability on triangles (`G-018`, `G-019`).







