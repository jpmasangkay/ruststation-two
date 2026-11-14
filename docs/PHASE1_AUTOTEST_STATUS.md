# Phase 1 `ps2autotests` Suite Status (Task 4 Update)

`local/ps2autotests/tests/` contains **78 prebuilt test binaries**:
- **55 EE `.elf` suites** (MIPS R5900 Emotion Engine binaries)
- **23 IOP `.irx` suites** (MIPS R3000A I/O Processor relocatable modules)

---

## Proposed Revised Phase 1 Gate

The blueprint's original `"52/55"` placeholder is replaced with an exact inventory and pass-list verified against `local/ps2autotests/tests/`:

| Scope Bucket | Total Suites | Passing Now (Task 4) | Target Task | Details |
| :--- | ---: | ---: | :--- | :--- |
| **In-Scope EE: CPU, COP0, COP1 FPU, MMI SIMD, Memory** | **23 `.elf`** | **`23 / 23 PASS` (100%)** | **Task 4 (Done)** | `cpu/ee` (6), `cpu/ee_cop0` (1), `cpu/ee_fpu` (9), `cpu/ee_simd` (6), `memory/ee` (1). Release run wall time: **`1.456 s`** (`75,856,740` EE cycles). |
| **In-Scope EE: DMAC & Scratchpad DMA** | **5 `.elf`** | `0 / 5` (`DEFERRED`) | **Task 5** | `dma/dmac/{cpcond,tagintr}`, `dma/spr/{chain,interleave,normal}`. |
| **In-Scope EE: Kernel Interrupt & Thread HLE** | **10 `.elf`** | `0 / 10` (`DEFERRED`) | **Task 7a** | `kernel/ee/interrupt/{dmac_order,intc_order}`, `kernel/ee/thread/{delete,exit,resume,sleep,stat,suspend,terminate,wakeup}`. |
| **In-Scope Phase 1 EE Subtotal** | **38 `.elf`** | **`23 / 38 PASS`** | **Tasks 4, 5, 7a** | All 38 Phase 1 EE `.elf` suites must pass by the end of Task 7a. |
| **In-Scope IOP: R3000A CPU & IOP Kernel/libc** | **23 `.irx`** | `0 / 23` (`DEFERRED`) | **Tasks 5 & 7b** | `cpu/iop` (7) and `kernel/iop` (16). All 23 `.irx` suites must pass by the end of Task 7b. |
| **Total In-Scope Phase 1 Suites (EE + IOP)** | **61 suites** | **`23 / 61 PASS`** | **End of Phase 1** | **Proposed Phase 1 Gate: `38 / 38` EE `.elf` + `23 / 23` IOP `.irx` (`61 / 61 PASS`)**. |
| **Out-of-Scope Phase 2+ EE Suites (VU0, VU1, VIF1, GS)** | **17 `.elf`** | `0 / 17` (`DEFERRED`) | **Phase 2+** | `cpu/vu0_macro` (2), `vu/` (7), `dma/vif` (6), `gs/` (2). |

---

## All 55 EE `.elf` Suites — Per-Suite Status Table

### 1. In-Scope EE CPU, COP0, COP1 FPU, MMI SIMD, and Memory Suites (`23 / 23 PASS`)

Release-mode execution command:
```powershell
cargo build --release -p ps2-autotest
# Runs all 23 suites sequentially and compares stdout against .expected ground truth
```
Total release wall time across all 23 suites: **`1.456 s` (`1456 ms`)** (`75,856,740` emulated EE cycles).

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

---

### 2. In-Scope Phase 1 EE Suites Deferred to Task 5 (DMAC) and Task 7a (EE Kernel HLE) (`15 .elf` suites)

| # | Suite (`tests/...`) | Category | Status | Target Task | Reason / Evidence from Execution |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 24 | `dma/dmac/cpcond.elf` | DMAC (`D_PCR` / `CPCOND0`) | `DEFERRED` | Task 5 | Polls `BC0F`/`BC0T` against DMAC `D_STAT`/`D_PCR` channel completion bits (`10,000,000` cycle timeout without DMAC engine). |
| 25 | `dma/dmac/tagintr.elf` | DMAC (DMA Tag IRQ) | `DEFERRED` | Task 5 | Starts source-chain DMA transfer with `IRQ` tag bit and waits for DMAC interrupt (`23 lines` printed; DMA transfers not executed yet). |
| 26 | `dma/spr/chain.elf` | DMAC (`fromSPR` / `toSPR` Chain) | `DEFERRED` | Task 5 | Triggers `fromSPR` (ch 8) / `toSPR` (ch 9) chain transfers and polls `CHCR.STR` bit 8 to clear (`10,000,000` cycle timeout). |
| 27 | `dma/spr/interleave.elf` | DMAC (`fromSPR` / `toSPR` Interleave) | `DEFERRED` | Task 5 | Triggers `fromSPR` / `toSPR` interleave transfers (`D_SQWC`) and polls `CHCR.STR` (`10,000,000` cycle timeout). |
| 28 | `dma/spr/normal.elf` | DMAC (`fromSPR` / `toSPR` Normal) | `DEFERRED` | Task 5 | Triggers `fromSPR` / `toSPR` normal transfers (`SADR`/`MADR`/`QWC`) and polls `CHCR.STR` (`10,000,000` cycle timeout). |
| 29 | `kernel/ee/interrupt/dmac_order.elf` | EE Kernel + DMAC IRQ | `DEFERRED` | Task 5 & 7a | Stops at `UnimplementedSyscall(pc=0x00101624, syscall=18 / 0x12 AddDmacHandler)` (`25,840` cycles); also uses `AddDmacHandler2` (`0x77`) and `EnableDmac` (`0x16`). |
| 30 | `kernel/ee/interrupt/intc_order.elf` | EE Kernel (`INTC` Queue) | `DEFERRED` | Task 7a | Registers multiple VBLANK handlers via `AddIntcHandler`/`AddIntcHandler2`, enables VBLANK (`EnableIntc`), and waits on a semaphore (`WaitSema`) signaled by the handlers. |
| 31 | `kernel/ee/thread/delete.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x00104994, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 32 | `kernel/ee/thread/exit.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x00104864, syscall=33 / 0x21 DeleteThread)` (`24,259` cycles). |
| 33 | `kernel/ee/thread/resume.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x00104974, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |
| 34 | `kernel/ee/thread/sleep.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x00104814, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |
| 35 | `kernel/ee/thread/stat.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x001049C4, syscall=48 / 0x30 ReferThreadStatus)` (`24,539` cycles). |
| 36 | `kernel/ee/thread/suspend.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x001049C4, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 37 | `kernel/ee/thread/terminate.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x00104A54, syscall=47 / 0x2F GetThreadId)` (`24,522` cycles). |
| 38 | `kernel/ee/thread/wakeup.elf` | EE Kernel (Threads) | `DEFERRED` | Task 7a | Stops at `UnimplementedSyscall(pc=0x001049A4, syscall=47 / 0x2F GetThreadId)` (`24,515` cycles). |

---

### 3. Out-of-Scope Phase 2+ EE Suites (`17 .elf` suites — VU0 Macro, VU Micro, VIF1, GS)

| # | Suite (`tests/...`) | Category | Status | Target Phase | Reason / Evidence from Execution |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 39 | `cpu/vu0_macro/integer.elf` | `COP2` / VU0 Macro | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_329C` (`24,254` cycles). VU0 macro mode is out of Phase 1 scope. |
| 40 | `cpu/vu0_macro/transfer.elf` | `COP2` / VU0 Macro | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x48B0_0800` (`QMTC2`) at `PC = 0x0010_07FC` (`26,764` cycles). VU0 macro mode is out of Phase 1 scope. |
| 41 | `dma/vif/basic.elf` | `VIF1` / DMA | `DEFERRED` | Phase 2 | Writes to `VIF1` (`0x1000_3C00`) / `DMA Ch 1` and polls `VIF1_STAT` / `CHCR.STR` (`10,000,000` cycle timeout). |
| 42 | `dma/vif/intstall.elf` | `VIF1` / DMA | `DEFERRED` | Phase 2 | Tests `VIF1` interrupt stall bit (`VIF1_STAT.INT`) and `VIF1_FBRST` (`27,481` cycles). |
| 43 | `dma/vif/mskpath3.elf` | `VIF1` / `GIF` PATH3 | `DEFERRED` | Phase 2 | Tests `VIF1` `MSKPATH3` command interaction with `GIF_STAT` (`10,000,000` cycle timeout). |
| 44 | `dma/vif/stcycl.elf` | `VIF1` (`STCYCL`/`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `STCYCL` + `UNPACK` into VU1 memory (`10,000,000` cycle timeout). |
| 45 | `dma/vif/stmod.elf` | `VIF1` (`STMOD`/`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `STMOD` addition decompression into VU1 memory (`10,000,000` cycle timeout). |
| 46 | `dma/vif/unpack.elf` | `VIF1` (`UNPACK`) | `DEFERRED` | Phase 2 | Tests `VIF1` `UNPACK` formats into VU1 memory (`10,000,000` cycle timeout). |
| 47 | `gs/label.elf` | `GS` / `GIF` (`LABEL`) | `DEFERRED` | Phase 2 | Sends `GIF` `A+D` packet to `GS_LABEL` via `DMA Ch 2` and reads `GS_SIGLBLID` (`55,615` cycles). |
| 48 | `gs/signal.elf` | `GS` / `GIF` (`SIGNAL`) | `DEFERRED` | Phase 2 | Sends `GIF` `A+D` packet to `GS_SIGNAL` via `DMA Ch 2` and tests `GS_CSR.SIGNAL` stall behavior (`71,860` cycles). |
| 49 | `vu/games/triace.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_3DBC` (`25,640` cycles). |
| 50 | `vu/lower/branch.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_3E7C` (`25,727` cycles). |
| 51 | `vu/lower/efu.elf` | `VU1` EFU + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_B70C` (`26,782` cycles). |
| 52 | `vu/lower/fdivdelay.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_420C` (`27,305` cycles). |
| 53 | `vu/lower/integer.elf` | `VU0`/`VU1` Micro + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_491C` (`30,061` cycles). |
| 54 | `vu/lower/random.elf` | `VU0`/`VU1` RNG + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_4264` (`25,793` cycles). |
| 55 | `vu/upper/clip.elf` | `VU0`/`VU1` Upper + `COP2` | `DEFERRED` | Phase 2 | Hits `COP2` opcode `0x4A00_02FF` at `PC = 0x0010_4A6C` (`27,219` cycles). |

---

## All 23 IOP `.irx` Suites — Per-Suite Status Table (Deferred to Tasks 5 & 7b)

| # | Suite (`tests/...`) | Category | Status | Target Task | Dependency |
| ---: | :--- | :--- | :--- | :--- | :--- |
| 1 | `cpu/iop/alu.irx` | IOP R3000A ALU | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 2 | `cpu/iop/branch.irx` | IOP R3000A Branch | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 3 | `cpu/iop/branchdelay.irx` | IOP R3000A Branch Delay | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 4 | `cpu/iop/hilodelay.irx` | IOP R3000A HI/LO Delay | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 5 | `cpu/iop/lsu.irx` | IOP R3000A Load/Store | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 6 | `cpu/iop/lsudelay.irx` | IOP R3000A LSU Delay | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 7 | `cpu/iop/muldiv.irx` | IOP R3000A Mul/Div | `DEFERRED` | Task 5 & 7b | R3000A interpreter (Task 5) + `.irx` loader & IOP `stdio` HLE (Task 7b). |
| 8 | `kernel/iop/eventflag/delete.irx` | IOP Kernel (`thevent`) | `DEFERRED` | Task 5 & 7b | IOP `thevent` module HLE (Task 7b). |
| 9 | `kernel/iop/eventflag/poll.irx` | IOP Kernel (`thevent`) | `DEFERRED` | Task 5 & 7b | IOP `thevent` module HLE (Task 7b). |
| 10 | `kernel/iop/interrupt/register.irx` | IOP Kernel (`intrman`) | `DEFERRED` | Task 5 & 7b | IOP `intrman` module HLE (Task 7b). |
| 11 | `kernel/iop/interrupt/release.irx` | IOP Kernel (`intrman`) | `DEFERRED` | Task 5 & 7b | IOP `intrman` module HLE (Task 7b). |
| 12 | `kernel/iop/libc/sprintf.irx` | IOP Kernel (`sysclib`) | `DEFERRED` | Task 5 & 7b | IOP `sysclib` module HLE (Task 7b). |
| 13 | `kernel/iop/libc/strtok.irx` | IOP Kernel (`sysclib`) | `DEFERRED` | Task 5 & 7b | IOP `sysclib` module HLE (Task 7b). |
| 14 | `kernel/iop/messagebox/poll.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Task 5 & 7b | IOP `thmsgbx` module HLE (Task 7b). |
| 15 | `kernel/iop/messagebox/receive.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Task 5 & 7b | IOP `thmsgbx` module HLE (Task 7b). |
| 16 | `kernel/iop/messagebox/send.irx` | IOP Kernel (`thmsgbx`) | `DEFERRED` | Task 5 & 7b | IOP `thmsgbx` module HLE (Task 7b). |
| 17 | `kernel/iop/thread/cancelwakeup.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Task 5 & 7b | IOP `thbase` module HLE (Task 7b). |
| 18 | `kernel/iop/thread/create.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Task 5 & 7b | IOP `thbase` module HLE (Task 7b). |
| 19 | `kernel/iop/thread/delete.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Task 5 & 7b | IOP `thbase` module HLE (Task 7b). |
| 20 | `kernel/iop/thread/releasewait.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Task 5 & 7b | IOP `thbase` module HLE (Task 7b). |
| 21 | `kernel/iop/thread/stat.irx` | IOP Kernel (`thbase`) | `DEFERRED` | Task 5 & 7b | IOP `thbase` module HLE (Task 7b). |
| 22 | `kernel/iop/vpool/create.irx` | IOP Kernel (`thvpool`) | `DEFERRED` | Task 5 & 7b | IOP `thvpool` module HLE (Task 7b). |
| 23 | `kernel/iop/vpool/pallocate.irx` | IOP Kernel (`thvpool`) | `DEFERRED` | Task 5 & 7b | IOP `thvpool` module HLE (Task 7b). |
