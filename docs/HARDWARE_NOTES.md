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
  - **`inferred; fits test vectors, may not generalize`**: Exact `CTC2 $22` cancellation behavior, exact `efu_cycles` vs `efu_cycles - 1` stall boundary (noting `ps2tek` line 2912 states EFU lacks the writeback hazard stage), and EFU numeric evaluation (EFU work is parked on local branch `wip/efu` pending Step 4 completion).

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
| `EFU` `P` latency table (`11..54` cycles) | `efu_latency` table in `vu/pipeline.rs` | **Sourced (`FETCHED`)** (ops parked on `wip/efu`) | `ps2tek` `#vupipelining` |
| `EFU` stall boundary (`efu_cycles` vs `efu_cycles - 1`) & `CTC2 $22` cancel | Stalls full `efu_cycles`; `CTC2 $22` zeroes `fdiv_cycles` | **`inferred; fits test vectors, may not generalize`** | Unverified pending Step 2c EFU resume |
| Delay-slot RAW stalls draining `FDIV`/`CLIP` across branches | `before_liw` drains stall cycles cleanly with `branch_delay_active` | **Sourced (`FETCHED`)** | `fdivdelay.cpp` (`PerformStalledBranch`) |
| Instantaneous stall drain per `step_micro_liw` in `Bus::step_timers` | `1.max(ee_cycles / 2)` LIW steps without slowing EE clock | **`inferred; fits test vectors, may not generalize`** | Untested against cycle-accurate EE/VU race tests |


