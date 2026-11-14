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
