# Phase 1 `ps2autotests` & LLE BIOS Boot Status (`phase1-done`)

`local/ps2autotests/tests/` contains **78 prebuilt test binaries**:
- **55 EE `.elf` suites** (MIPS R5900 Emotion Engine binaries)
- **23 IOP `.irx` suites** (MIPS R3000A I/O Processor relocatable modules)

---

## Phase 1 Summary (`35 / 35` Hardware Gate PASS, LLE BIOS Boot `PARTIAL`)

| Track / Scope Bucket | Total Suites | Status | Target Task / Milestone | Details |
| :--- | ---: | ---: | :--- | :--- |
| **Phase 1 Hardware Gate — EE CPU, COP0, COP1 FPU, MMI SIMD, Memory** | **23 `.elf`** | **`23 / 23 PASS` (100%)** | **Task 4 (Done)** | `cpu/ee` (6), `cpu/ee_cop0` (1), `cpu/ee_fpu` (9), `cpu/ee_simd` (6), `memory/ee` (1). |
| **Phase 1 Hardware Gate — EE DMAC & Scratchpad DMA** | **5 `.elf`** | **`5 / 5 PASS` (100%)** | **Task 5 (Done)** | `dma/dmac/{cpcond,tagintr}`, `dma/spr/{chain,interleave,normal}`. |
| **Phase 1 Hardware Gate — IOP R3000A CPU** | **7 `.irx`** | **`7 / 7 PASS` (100%)** | **Task 6a (Done)** | `cpu/iop/{alu,branch,branchdelay,hilodelay,lsu,lsudelay,muldiv}.irx`. |
| **Phase 1 Hardware Gate Total** | **35 suites** | **`35 / 35 PASS` (100%)** | **Task 6a (Done)** | **`35 / 35` EE + DMAC/SPR + IOP suites passing in `1.408 s` release wall time (`82,167,799` EE cycles + `30,724` IOP cycles = `82,198,523` total cycles, `58.38 Mcycles/s`).** |
| **LLE BIOS Boot (`Task 6b`)** | **Coupled EE+IOP** | **`PARTIAL`** | **Task 6b (Done) / Phase 3** | **IOP kernel init reached `EECONF` (`272,558` IOP cycles); blocked on CDVD S-commands `0x40`/`0x41`/`0x43` and NVM config data; EE parked after the first RDRAM-init message (`2,180,464` EE cycles).** |
| **Kernel Track (Post-LLE BIOS) — EE & IOP Kernel Behavior** | **26 suites** | `0 / 26` (`DEFERRED`) | **Post-LLE BIOS (Phase 3)** | `kernel/ee` (10 `.elf`) + `kernel/iop` (16 `.irx`). Not part of the Phase 1 hardware gate; revisit after full LLE BIOS boot works. |
| **Out-of-Scope Phase 2+ EE Suites (VU0, VU1, VIF1, GS)** | **17 `.elf`** | `0 / 17` (`DEFERRED`) | **Phase 2a** | `cpu/vu0_macro` (2), `vu/` (7), `dma/vif` (6), `gs/` (2). |

---

## LLE BIOS Boot Status (`PARTIAL`) & Phase 3 Resume Point

- **Current Status**: **`PARTIAL`**
  - **EE Progress**: `2,180,464` cycles (`EE PC = 0x9FC4_17B4`). Completed RDRAM MCH initialization (`0x1000_F430` / `0x1000_F440`), configured `EE_SIO` (`0x1000_F100..=0x1000_F180`), printed the first RDRAM-init serial banner, and is scrubbing RDRAM prior to SIF synchronization.
  - **IOP Progress**: `272,558` cycles (`IOP PC = 0x0000_AFA0`). Completed ROM bootstrap (`GM_IF`, `SSBUSC`, `POST2`, `KSEG2` Cache Control, `COP0.SR.IsC` cache isolation) and initialized `SYSMEM`, `LOADCORE`, `EXCEPMAN`, `INTRMAN`, `SSBUSC`, and `DMACMAN` in IOP RAM before stopping in `EECONF._start`.
- **Exact Resume Point**:
  - **CPU & PC**: `IOP` at `PC = 0x0000_AFA0` (`EE PC = 0x9FC4_17B4`, `ee_cycles = 2,180,464`, `iop_cycles = 272,558`).
  - **Blocker**: `UnimplementedIopRegister(paddr = 0x1F40_2005, width = 1, is_write = false, value = 0)` (`CDVD N command status` read in `EECONF._start`, immediately followed by CDVD S-commands `0x40` `OpenConfig`, `0x41` `ReadConfig`, and `0x43` `CloseConfig` on `0x1F40_2016..=0x1F40_2018`).
- **Deferred to Phase 3 (`ps2-cdvd`, `ps2-spu2`, I/O & Kernel Track)**:
  1. **CDVD S-command FIFO and NVM config (`ps2-cdvd`, `0x1F40_2004..=0x1F40_2018`)**: S-command parameter/result FIFO state machine (`0x1F40_2016`, `0x1F40_2017` bit 6 data-available transition, `0x1F40_2018`) and NVM/EEPROM configuration blocks (`0x40`/`0x41`/`0x43`).
  2. **SIF DMA handshake (`SIF0` / `SIF1`, IOP `0x1D00_0000..=0x1D00_0060` & EE `0x1000_F200..=0x1000_F260`)**: bidirectional transfer between IOP DMA Ch 9 / EE DMA Ch 5 (`SIF0`) and EE DMA Ch 6 / IOP DMA Ch 10 (`SIF1`).
  3. **SIO2 pad and memory card controller (`0x1F80_8200..=0x1F80_8280`, IOP DMA Ch 11/12)**.
  4. **SPU2 (`ps2-spu2`, `0x1F90_0000..=0x1F90_07FF`, IOP DMA Ch 4/7)**.

---

## All 55 EE `.elf` Suites — Per-Suite Status Table

### 1. Phase 1 Hardware Gate: EE CPU, COP0, COP1 FPU, MMI SIMD, Memory, and DMAC/SPR Suites (`28 / 28 PASS`)

Release-mode execution command:
```powershell
cargo build --release -p ps2-autotest
# Runs all 28 in-scope EE & DMAC/SPR suites sequentially and compares stdout against .expected ground truth
```
Total release wall time across all 28 suites: **`1.778 s` (`1778 ms`)** (`82,167,799` emulated EE cycles).

| # | Suite (`tests/...`) | Category | Status | Emulated Cycles | Notes / Verification |
| ---: | :--- | :--- | :--- | ---: | :--- |
| 1 | `cpu/ee/alu.elf` | EE Base ALU | `PASS` | `4,926,806` | Matches `alu.expected` (100%). |
| 2 | `cpu/ee/branch.elf` | EE Base Branch | `PASS` | `1,299,236` | Matches `branch.expected` (100%). |
| 3 | `cpu/ee/branchdelay.elf` | EE Base Branch Delay | `PASS` | `43,694` | Matches `branchdelay.expected` (100%). |
| 4 | `cpu/ee/lsu.elf` | EE Base Load/Store | `PASS` | `1,182,281` | Matches `lsu.expected` (100%, including `LQ`/`SQ` and unaligned `LDL`/`LDR`/`LD`/`SD`/`LWL`/`LWR`/`SWL`/`SWR`). |
| 5 | `cpu/ee/lsudelay.elf` | EE Base LSU Delay | `PASS` | `29,809` | Matches `lsudelay.expected` (100%). |
| 6 | `cpu/ee/muldiv.elf` | EE Base + MMI Mul/Div | `PASS` | `9,359,190` | Matches `muldiv.expected` (100%, including 3-op `MULT`/`MULTU`, `DIV`/`DIVU`, `MULT1`/`MULTU1`, `DIV1`/`DIVU1`, `MADD`/`MADDU`, `MADD1`/`MADDU1`, `MFHI1`/`MFLO1`/`MTHI1`/`MTLO1`). |
| 7 | `cpu/ee_cop0/performance.elf` | EE COP0 Perf | `PASS` | `129,851` | Matches `performance.expected` (100%, `MFPS`/`MTPS`/`MFPC`/`MTPC` & `PCCR` event counting). |
| 8 | `cpu/ee_fpu/arithmetic.elf` | EE COP1 FPU | `PASS` | `7,899,174` | Matches `arithmetic.expected` (100%, `ADD.S`, `SUB.S`, `ADDA.S`, `SUBA.S`, `ABS.S`, `NEG.S`, `MOV.S`). |
| 9 | `cpu/ee_fpu/branch.elf` | EE COP1 FPU | `PASS` | `94,633` | Matches `branch.expected` (100%, `BC1F`, `BC1T`, `BC1FL`, `BC1TL`). |
| 10 | `cpu/ee_fpu/branchdelay.elf` | EE COP1 FPU | `PASS` | `30,090` | Matches `branchdelay.expected` (100%, branch-in-delay-slot target registration). |
| 11 | `cpu/ee_fpu/compare.elf` | EE COP1 FPU | `PASS` | `3,065,213` | Matches `compare.expected` (100%, `C.F.S`, `C.EQ.S`, `C.LT.S`, `C.LE.S`, `MAX.S`, `MIN.S`). |
| 12 | `cpu/ee_fpu/convert.elf` | EE COP1 FPU | `PASS` | `1,527,669` | Matches `convert.expected` (100%, `CVT.S.W`, `CVT.W.S`). |
| 13 | `cpu/ee_fpu/fcr.elf` | EE COP1 FPU | `PASS` | `430,521` | Matches `fcr.expected` (100%, `FCR0`/`FCR31` register mirroring, writable bitmasks, and sticky flags). |
| 14 | `cpu/ee_fpu/muldiv.elf` | EE COP1 FPU | `PASS` | `13,865,818` | Matches `muldiv.expected` (100%, `MUL.S`, `MULA.S`, `DIV.S`, `MADD.S`, `MADDA.S`, `MSUB.S`, `MSUBA.S`, Booth multiplier truncation, and MAC overflow saturation). |
| 15 | `cpu/ee_fpu/sqrt.elf` | EE COP1 FPU | `PASS` | `1,818,130` | Matches `sqrt.expected` (100%, `SQRT.S` and `RSQRT.S` 24-bit guard-bit rounding and negative-zero handling). |
| 16 | `cpu/ee_fpu/transfer.elf` | EE COP1 FPU | `PASS` | `243,796` | Matches `transfer.expected` (100%, `MFC1`, `MTC1`, `CFC1`, `CTC1`, `LWC1`, `SWC1`). |
| 17 | `cpu/ee_simd/arithmetic.elf` | EE MMI SIMD | `PASS` | `5,010,602` | Matches `arithmetic.expected` (100%, `PADD*`, `PSUB*`, `PADDS*`, `PSUBS*`, `PABSH`, `PABSW`, `PMAX*`, `PMIN*`). |
| 18 | `cpu/ee_simd/compare.elf` | EE MMI SIMD | `PASS` | `1,235,219` | Matches `compare.expected` (100%, `PCGT*`, `PCEQ*`). |
| 19 | `cpu/ee_simd/funnel.elf` | EE MMI SIMD | `PASS` | `315,357` | Matches `funnel.expected` (100%, `MTSAB`, `MTSAH`, `QFSRV`, `PSLL*`, `PSRL*`, `PSRA*`, `PSLLVW`, `PSRLVW`, `PSRAVW`). |
| 20 | `cpu/ee_simd/logic.elf` | EE MMI SIMD | `PASS` | `2,571,940` | Matches `logic.expected` (100%, `PAND`, `POR`, `PXOR`, `PNOR`, `PLZCW`). |
| 21 | `cpu/ee_simd/muldiv.elf` | EE MMI SIMD | `PASS` | `15,159,284` | Matches `muldiv.expected` (100%, `PMULTH`, `PMULTW`, `PMULTUW`, `PDIVW`, `PDIVUW`, `PDIVBW`, `PMADDH`, `PHMADH`, `PMSUBH`, `PHMSBH`, `PMADDW`, `PMADDUW`, `PMSUBW`, `PMFHL.*`, `PMTHL.LW`). |
| 22 | `cpu/ee_simd/shuffle.elf` | EE MMI SIMD | `PASS` | `4,294,859` | Matches `shuffle.expected` (100%, `PCPYH`, `PCPYLD`, `PCPYUD`, `PEXCH`, `PEXCW`, `PEXEH`, `PEXEW`, `PEXT*`, `PINTH`, `PINTEH`, `PPAC*`, `PREVH`, `PROT3W`). |
| 23 | `memory/ee/default.elf` | EE Memory / vtlb | `PASS` | `273,568` | Matches `default.expected` (100%, cached/uncached/UCAB mirrors, scratchpad `0x7000_0000`, and `COP0` TLB remap). |
| 24 | `dma/dmac/cpcond.elf` | DMAC (`D_PCR` / `CPCOND0`) | `PASS` | `37,654` | Matches `cpcond.expected` (100%, `BC0F`/`BC0T` `CPCOND0` evaluation against `D_PCR.CPC0..9` and `D_STAT.CIS0..9`). |
| 25 | `dma/dmac/tagintr.elf` | DMAC (DMA Tag IRQ) | `PASS` | `76,112` | Matches `tagintr.expected` (100%, source-chain `REF`/`END` tag execution, `CHCR.TAG` latching, `CHCR.TIE` + `tag.IRQ` early termination, and `D_STAT.CIS9`). |
| 26 | `dma/spr/chain.elf` | DMAC (`fromSPR` / `toSPR` Chain) | `PASS` | `180,921` | Matches `chain.expected` (100%, `toSPR` source-chain `REFE`, `SADR` `& 0x3FF0` masking, and `QWC = 0` tag zero-transfer behavior). |
| 27 | `dma/spr/interleave.elf` | DMAC (`fromSPR` / `toSPR` Interleave) | `PASS` | `5,680,034` | Matches `interleave.expected` (100%, `D_SQWC` 8-bit `SQWC`/`TQWC` masking and per-slice `SQWC * 16` `MADR` skip). |
| 28 | `dma/spr/normal.elf` | DMAC (`fromSPR` / `toSPR` Normal) | `PASS` | `336,338` | Matches `normal.expected` (100%, `fromSPR`/`toSPR` `SADR` `& 0x3FF0` masking, 16 KB wraparound, and `MADR` bit-31 clearing). |

---

### 2. Kernel Track: EE Kernel Interrupt & Thread Suites (`10 .elf` suites — Post-LLE BIOS Track)

These suites test BIOS kernel behavior (`AddIntcHandler`, `AddDmacHandler`, `CreateThread`, `StartThread`, `SleepThread`, etc.) and are outside the 35-suite Phase 1 Hardware Gate.

| # | Suite (`tests/...`) | Category | Status | Track | Reason / Evidence from Execution |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 29 | `kernel/ee/interrupt/dmac_order.elf` | EE Kernel + DMAC IRQ | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00101624, syscall=18 / 0x12 AddDmacHandler)` (`25,840` cycles). |
| 30 | `kernel/ee/interrupt/intc_order.elf` | EE Kernel (`INTC` Queue) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Registers multiple VBLANK handlers via `AddIntcHandler`/`AddIntcHandler2` and waits on a semaphore (`WaitSema`). |
| 31 | `kernel/ee/thread/delete.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00104994, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 32 | `kernel/ee/thread/exit.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00104864, syscall=33 / 0x21 DeleteThread)` (`24,259` cycles). |
| 33 | `kernel/ee/thread/resume.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00104974, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |
| 34 | `kernel/ee/thread/sleep.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00104814, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |
| 35 | `kernel/ee/thread/stat.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x001049C4, syscall=48 / 0x30 ReferThreadStatus)` (`24,539` cycles). |
| 36 | `kernel/ee/thread/suspend.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x001049C4, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 37 | `kernel/ee/thread/terminate.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x00104A54, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 38 | `kernel/ee/thread/wakeup.elf` | EE Kernel (Threads) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | Stops at `UnimplementedSyscall(pc=0x001049A4, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |

---

### 3. Out-of-Scope Phase 2+ EE Suites (`17 .elf` suites — VU0 Macro, VU Micro, VIF1, GS)

| # | Suite (`tests/...`) | Category | Status | Target Phase | Reason / Evidence from Execution |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 39 | `cpu/vu0_macro/integer.elf` | `COP2` / VU0 Macro | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_329C` (`24,254` cycles). VU0 macro mode is out of Phase 1 scope. |
| 40 | `cpu/vu0_macro/transfer.elf` | `COP2` / VU0 Macro | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x48B0_0800` (`QMTC2`) at `PC = 0x0010_07FC` (`26,764` cycles). VU0 macro mode is out of Phase 1 scope. |
| 41 | `dma/vif/basic.elf` | `VIF1` / DMA | `DEFERRED` | Phase 2 | Writes to `VIF1` (`0x1000_3C00`) / `DMA Ch 1` (`UnimplementedDmaTransfer` / `VIF1_STAT`). |
| 42 | `dma/vif/intstall.elf` | `VIF1` / DMA | `DEFERRED` | Phase 2 | Tests `VIF1` interrupt stall bit (`VIF1_STAT.INT`) and `VIF1_FBRST`. |
| 43 | `dma/vif/mskpath3.elf` | `VIF1` / `GIF` PATH3 | `DEFERRED` | Phase 2 | Tests `VIF1` `MSKPATH3` command interaction with `GIF_STAT`. |
| 44 | `dma/vif/stcycl.elf` | `VIF1` (`STCYCL`/`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `STCYCL` + `UNPACK` into VU1 memory. |
| 45 | `dma/vif/stmod.elf` | `VIF1` (`STMOD`/`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `STMOD` addition decompression into VU1 memory. |
| 46 | `dma/vif/unpack.elf` | `VIF1` (`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `UNPACK` formats into VU1 memory. |
| 47 | `gs/label.elf` | `GS` / `GIF` (`LABEL`) | `DEFERRED` | Phase 2 | Sends `GIF` `A+D` packet to `GS_LABEL` via `DMA Ch 2` (`UnimplementedDmaTransfer(ch=2 [GIF])`). |
| 48 | `gs/signal.elf` | `GS` / `GIF` (`SIGNAL`) | `DEFERRED` | Phase 2 | Sends `GIF` `A+D` packet to `GS_SIGNAL` via `DMA Ch 2` (`UnimplementedDmaTransfer(ch=2 [GIF])`). |
| 49 | `vu/games/triace.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_3DBC` (`25,640` cycles). |
| 50 | `vu/lower/branch.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_3E7C` (`25,727` cycles). |
| 51 | `vu/lower/efu.elf` | `VU1` EFU + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_B70C` (`26,782` cycles). |
| 52 | `vu/lower/fdivdelay.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_420C` (`27,305` cycles). |
| 53 | `vu/lower/integer.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_491C` (`30,061` cycles). |
| 54 | `vu/lower/random.elf` | `VU0`/`VU1` RNG + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_4264` (`25,793` cycles). |
| 55 | `vu/upper/clip.elf` | `VU0`/`VU1` Upper + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_4A6C` (`27,219` cycles). |

---

## All 23 IOP `.irx` Suites — Per-Suite Status Table

### 1. Phase 1 Hardware Gate: IOP R3000A CPU Suites (`7 / 7 PASS`)

| # | Suite (`tests/...`) | Category | Status | Emulated IOP Cycles (`x8` EE Cycles) | Notes / Verification |
| ---: | :--- | :--- | :--- | ---: | :--- |
| 1 | `cpu/iop/alu.irx` | IOP R3000A ALU | `PASS` | `10,542` (`84,336` EE) | Matches `alu.expected` (100%, `ADDU`, `ADDIU`, `SUBU`, `AND*`, `OR*`, `XOR*`, `NOR`, `LUI`, `SLT*`, `SLL*`, `SRL*`, `SRA*`). |
| 2 | `cpu/iop/branch.irx` | IOP R3000A Branch | `PASS` | `7,906` (`63,248` EE) | Matches `branch.expected` (100%, `BEQ`, `BNE`, `BLEZ`, `BGTZ`, `BLTZ`, `BGEZ`, `BLTZAL`, `BGEZAL`). |
| 3 | `cpu/iop/branchdelay.irx` | IOP R3000A Branch Delay | `PASS` | `219` (`1,752` EE) | Matches `branchdelay.expected` (100%, `BEQ` in delay slot, `JAL`/`JALR`/`BLTZAL`/`BGEZAL` link order vs delay slot, `JALR rs == rd`). |
| 4 | `cpu/iop/hilodelay.irx` | IOP R3000A HI/LO Delay | `PASS` | `112` (`896` EE) | Matches `hilodelay.expected` (100%, `MFHI`/`MFLO`/`DIV`/`MULT` pipeline behavior). |
| 5 | `cpu/iop/lsu.irx` | IOP R3000A Load/Store | `PASS` | `2,748` (`21,984` EE) | Matches `lsu.expected` (100%, `LB`, `LBU`, `LH`, `LHU`, `LW`, `SB`, `SH`, `SW`, `LD`/`SD` macros, and unaligned `LWL`/`LWR`/`SWL`/`SWR`). |
| 6 | `cpu/iop/lsudelay.irx` | IOP R3000A LSU Delay | `PASS` | `232` (`1,856` EE) | Matches `lsudelay.expected` (100%, 1-instruction load delay slot visibility and delay-slot overwrite cancellation). |
| 7 | `cpu/iop/muldiv.irx` | IOP R3000A Mul/Div | `PASS` | `8,965` (`71,720` EE) | Matches `muldiv.expected` (100%, `DIV`, `DIVU`, `MULT`, `MULTU`, `MFHI`, `MFLO`, `MTHI`, `MTLO`, divide-by-zero, and `i32::MIN / -1`). |

### 2. Kernel Track: IOP Kernel Module Suites (`16 .irx` suites — Post-LLE BIOS Track)

| # | Suite (`tests/...`) | Category | Status | Track | Dependency |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 8 | `kernel/iop/eventflag/delete.irx` | IOP Kernel (`thevent`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thevent` module behavior. |
| 9 | `kernel/iop/eventflag/poll.irx` | IOP Kernel (`thevent`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thevent` module behavior. |
| 10 | `kernel/iop/interrupt/register.irx` | IOP Kernel (`intrman`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `intrman` module behavior. |
| 11 | `kernel/iop/interrupt/release.irx` | IOP Kernel (`intrman`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `intrman` module behavior. |
| 12 | `kernel/iop/libc/sprintf.irx` | IOP Kernel (`sysclib`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `sysclib` module behavior. |
| 13 | `kernel/iop/libc/strtok.irx` | IOP Kernel (`sysclib`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `sysclib` module behavior. |
| 14 | `kernel/iop/messagebox/poll.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thmsgbx` module behavior. |
| 15 | `kernel/iop/messagebox/receive.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thmsgbx` module behavior. |
| 16 | `kernel/iop/messagebox/send.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thmsgbx` module behavior. |
| 17 | `kernel/iop/thread/cancelwakeup.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thbase` module behavior. |
| 18 | `kernel/iop/thread/create.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thbase` module behavior. |
| 19 | `kernel/iop/thread/delete.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thbase` module behavior. |
| 20 | `kernel/iop/thread/releasewait.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thbase` module behavior. |
| 21 | `kernel/iop/thread/stat.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thbase` module behavior. |
| 22 | `kernel/iop/vpool/create.irx` | IOP Kernel (`thvpool`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thvpool` module behavior. |
| 23 | `kernel/iop/vpool/pallocate.irx` | IOP Kernel (`thvpool`) | `DEFERRED` | Kernel Track (Post-LLE BIOS) | IOP `thvpool` module behavior. |
