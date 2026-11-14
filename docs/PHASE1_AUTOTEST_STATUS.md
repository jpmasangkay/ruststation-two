# Phase 1 `ps2autotests` Suite Status (Task 3b Baseline)

`local/ps2autotests/tests/` contains **78 prebuilt test suites**:
- **55 EE `.elf` suites** (compiled with `ee-gcc` against `ps2sdk` `libc`/`newlib`).
- **23 IOP `.irx` suites** (MIPS R3000A relocatable IOP modules).

## Summary by Category (Task 3b)

| Category | Count | Task 3b Status | Notes |
| :--- | ---: | :--- | :--- |
| `cpu/ee` (Base R5900 + MMI/COP1 in `libc`) | 6 | `DEFERRED (Task 4)` | All 6 prebuilt `.elf` binaries execute `crt0` and `fioWrite` SIF RPC stdout (`-- TEST BEGIN`), then hit `ee-gcc`-emitted `MMI` (`POR`/`PCPYLD`/`PCPYH`) or `COP1` (`SWC1 $f12..$f18` in `printf` / `_vfprintf_r`). |
| `cpu/ee` (MMI-specific: `mmi`, `mmi0..3`) | 5 | `DEFERRED (Task 4)` | Directly test 128-bit R5900 Multimedia Instructions (`opcode = 0x1C`). |
| `cpu/ee_cop0` (`performance`) | 1 | `DEFERRED (Task 4)` | `COP0.Perf` (`MFPS`/`MTPS`/`MFPC`/`MTPC` & `PCCR` cycle counting) verified in unit tests; prebuilt `.elf` calls `newlib` `memset` (`MMI3 PCPYH`) at `0x0010_2AC4`. |
| `cpu/ee_fpu` (`arithmetic`, `branch`, `compare`, `convert`, `fcr`) | 5 | `DEFERRED (Task 4)` | Require `COP1` (FPU `ps2float`) instructions (`opcode = 0x11`, `LWC1` `0x31`, `SWC1` `0x39`). |
| `cpu/vu0` (`macro`) | 1 | `DEFERRED (Task 4)` | Requires `COP2` (VU0 Macro Mode) instructions (`opcode = 0x12`, `LWC2` `0x32`, `SWC2` `0x3A`). |
| `cpu/iop` (`alu`, `branch`, `branchdelay`, `lsu`, `lsudelay`, `muldiv`) | 6 | `DEFERRED (Task 5 / 7b)` | Require R3000A IOP interpreter (Task 5) and `.irx` loader / IOP `stdio` HLE (Task 7b). |
| `memory/ee` (`default`) | 1 | `DEFERRED (Task 4)` | `vtlb` cached/uncached/UCAB mirrors and scratchpad verified in unit tests; prebuilt `.elf` calls `newlib` `memset` (`MMI3 PCPYH`) at `0x0010_2300`. |
| `dmac` (`commit`, ` normal`, `chain`, `interleave`, `sadr`, `stall`) | 9 | `DEFERRED (Task 5 / 4)` | Require 10-channel DMAC (Task 5) and `MMI`/`COP1` for `libc`. |
| `kernel/ee` (threads, semaphores, alarm, intc, dmac,cache, osversion) | 31 | `DEFERRED (Task 7a / 4)` | Require full EE kernel HLE (`Task 7a`) plus `MMI`/`COP1` for `libc`. |
| `kernel/iop` (threads, semaphores, eventflag, mbox, timer, sysmem, intrman) | 17 | `DEFERRED (Task 5 / 7b)` | Require R3000A IOP interpreter (Task 5) and IOP kernel module HLE (Task 7b). |
| `gs` (`download`) | 1 | `DEFERRED (Phase 2+)` | Out of Phase 1 scope (requires GS local-to-host `VIF1`/`GIF` texture download). |
| `vu` (`assemble`, `lower`, `triace`, `upper`) | 4 | `DEFERRED (Phase 2+)` | Out of Phase 1 scope (require VU0/VU1 micro-mode execution and `mpg`/`vcallms`). |

---

## Detailed Per-Suite Status Table (All 78 Suites)

### 1. EE CPU, COP0, COP1, VU0 Macro, and Memory Suites (18 `.elf` suites)

| Suite (`tests/...`) | Binary | Task 3b Status | Evidence / First Unimplemented Instruction or Dependency |
| :--- | :--- | :--- | :--- |
| `cpu/ee/alu` | `.elf` | `DEFERRED (Task 4)` | Prints `-- TEST BEGIN\n` via `fioWrite` SIF RPC (`21,428` cycles), then hits `MMI3 POR` (`0x7010_94A9`) at `PC = 0x0010_02B0` and `COP1 SWC1` (`0xE7AC_0028`) at `PC = 0x0011_02E8` (`printf`). |
| `cpu/ee/branch` | `.elf` | `DEFERRED (Task 4)` | Prints `-- TEST BEGIN\n` and executes `test_beq` (`21,444` cycles), then hits `COP1 SWC1 $f12, 40($sp)` (`0xE7AC_0028`) at `PC = 0x0011_0E80` (`printf`) and `MMI1 PCPYLD` in `libc`. |
| `cpu/ee/branchdelay` | `.elf` | `DEFERRED (Task 4)` | Executes `crt0` (`13,471` cycles), then hits `MMI1 PCPYLD` (`0x7129_6B89`) at `PC = 0x0010_0C80` (`libc` `strcpy`) and `COP1 SWC1` (`0xE7AC_0028`) at `PC = 0x0010_05F8` (`printf`). |
| `cpu/ee/lsu` | `.elf` | `DEFERRED (Task 4)` | Executes `crt0`, `InitKernel`, `InitTLBFunctions` (`15,872` cycles), then hits `MMI3 PCPYH` (`0x7008_1EE9`) at `PC = 0x0010_6174` (`newlib` `memset`) and `COP1 SWC1` (`_svfpu`). |
| `cpu/ee/lsudelay` | `.elf` | `DEFERRED (Task 4)` | Prints `-- TEST BEGIN\n` and executes `test_lw` (`21,445` cycles), then hits `COP1 SWC1 $f12, 40($sp)` (`0xE7AC_0028`) at `PC = 0x0010_0338` (`printf`). |
| `cpu/ee/muldiv` | `.elf` | `DEFERRED (Task 4)` | Prints `-- TEST BEGIN\n` and executes `div` (`21,819` cycles), then hits `COP1 SWC1` (`0xE7AC_0028`) at `PC = 0x0011_8DF0` (`printf`); also directly tests `MMI` (`div1`, `divu1`, `madd`, `maddu`, `madd1`, `maddu1`, `mult1`, `multu1`, `mfhi1`, `mflo1`, `mthi1`, `mtlo1`, `por`). |
| `cpu/ee/mmi` | `.elf` | `DEFERRED (Task 4)` | Tests R5900 top-level `MMI` instructions (`opcode = 0x1C`). |
| `cpu/ee/mmi0` | `.elf` | `DEFERRED (Task 4)` | Tests R5900 `MMI0` instructions (`opcode = 0x1C, funct = 0x08`). |
| `cpu/ee/mmi1` | `.elf` | `DEFERRED (Task 4)` | Tests R5900 `MMI1` instructions (`opcode = 0x1C, funct = 0x28`). |
| `cpu/ee/mmi2` | `.elf` | `DEFERRED (Task 4)` | Tests R5900 `MMI2` instructions (`opcode = 0x1C, funct = 0x09`). |
| `cpu/ee/mmi3` | `.elf` | `DEFERRED (Task 4)` | Tests R5900 `MMI3` instructions (`opcode = 0x1C, funct = 0x29`). |
| `cpu/ee_cop0/performance` | `.elf` | `DEFERRED (Task 4)` | Executes `crt0` (`15,872` cycles), then hits `MMI3 PCPYH` (`0x7008_1EE9`) at `PC = 0x0010_2AC4` (`newlib` `memset`) and `COP1 SWC1` (`_svfpu`). (`MFPS`/`MTPS`/`MFPC`/`MTPC` & `PCCR` verified by unit test). |
| `cpu/ee_fpu/arithmetic` | `.elf` | `DEFERRED (Task 4)` | Tests `COP1` FPU arithmetic (`ADD.S`, `SUB.S`, `MUL.S`, `DIV.S`, `SQRT.S`, `RSQRT.S`, `MADD.S`, `MSUB.S`, `MADDA.S`, `MSUBA.S`). |
| `cpu/ee_fpu/branch` | `.elf` | `DEFERRED (Task 4)` | Tests `COP1` FPU branches (`BC1F`, `BC1T`, `BC1FL`, `BC1TL`). |
| `cpu/ee_fpu/compare` | `.elf` | `DEFERRED (Task 4)` | Tests `COP1` FPU comparisons (`C.F.S`, `C.EQ.S`, `C.LT.S`, `C.LE.S`, `MAX.S`, `MIN.S`). |
| `cpu/ee_fpu/convert` | `.elf` | `DEFERRED (Task 4)` | Tests `COP1` FPU conversions (`CVT.S.W`, `CVT.W.S`). |
| `cpu/ee_fpu/fcr` | `.elf` | `DEFERRED (Task 4)` | Tests `COP1` control registers (`CFC1`, `CTC1`, `FCR0`, `FCR31`). |
| `cpu/vu0/macro` | `.elf` | `DEFERRED (Task 4)` | Tests `COP2` VU0 macro mode instructions (`opcode = 0x12`, `LQC2`, `SQC2`). |
| `memory/ee/default` | `.elf` | `DEFERRED (Task 4)` | Executes `crt0` (`14,486` cycles), then hits `MMI3 PCPYH` (`0x7008_1EE9`) at `PC = 0x0010_2300` (`newlib` `memset`). |

### 2. DMAC Suites (9 `.elf` suites)

| Suite (`tests/...`) | Binary | Task 3b Status | Dependency |
| :--- | :--- | :--- | :--- |
| `dmac/commit` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC channel registers + `MMI`/`COP1` (`Task 4`). |
| `dmac/cpcond0` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `D_PCR` / `CPCOND0` wiring + `MMI`/`COP1` (`Task 4`). |
| `dmac/ Decoupled / fromspr_chain` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `fromSPR` (ch 8) source/dest chain mode + `MMI`/`COP1`. |
| `dmac/fromspr_interleave` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `fromSPR` (ch 8) interleave mode + `MMI`/`COP1`. |
| `dmac/fromspr_normal` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `fromSPR` (ch 8) normal mode + `MMI`/`COP1`. |
| `dmac/sadr` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `SADR` scratchpad address register + `MMI`/`COP1`. |
| `dmac/stall` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC stall control (`STADR`) + `MMI`/`COP1`. |
| `dmac/tospr_chain` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `toSPR` (ch 9) chain mode + `MMI`/`COP1`. |
| `dmac/tospr_interleave` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `toSPR` (ch 9) interleave mode + `MMI`/`COP1`. |
| `dmac/tospr_normal` | `.elf` | `DEFERRED (Task 5)` | Requires DMAC `toSPR` (ch 9) normal mode + `MMI`/`COP1`. |

### 3. EE Kernel HLE Suites (22 `.elf` suites)

| Suite (`tests/...`) | Binary | Task 3b Status | Dependency |
| :--- | :--- | :--- | :--- |
| `kernel/ee/alarm/negative` | `.elf` | `DEFERRED (Task 7a)` | Requires EE Kernel `SetAlarm` / `ReleaseAlarm` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/alarm/order` | `.elf` | `DEFERRED (Task 7a)` | Requires EE Kernel `SetAlarm` / `ReleaseAlarm` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/cache/flush` | `.elf` | `DEFERRED (Task 7a)` | Requires `FlushCache` (`0x64`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/dmac/enable` | `.elf` | `DEFERRED (Task 7a)` | Requires `EnableDmac`/`DisableDmac` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/dmac/handler` | `.elf` | `DEFERRED (Task 7a)` | Requires `AddDmacHandler`/`RemoveDmacHandler` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/intc/enable` | `.elf` | `DEFERRED (Task 7a)` | Requires `EnableIntc`/`DisableIntc` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/intc/handler` | `.elf` | `DEFERRED (Task 7a)` | Requires `AddIntcHandler`/`RemoveIntcHandler` HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/intc/queue` | `.elf` | `DEFERRED (Task 7a)` | Requires `AddIntcHandler2` queue ordering (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/osversion` | `.elf` | `DEFERRED (Task 7a)` | Requires OSD config / kernel version HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/create` | `.elf` | `DEFERRED (Task 7a)` | Requires EE semaphore HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/delete` | `.elf` | `DEFERRED (Task 7a)` | Requires EE semaphore HLE (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/poll` | `.elf` | `DEFERRED (Task 7a)` | Requires `PollSema` (`0x45`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/signal` | `.elf` | `DEFERRED (Task 7a)` | Requires `SignalSema` (`0x42`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/status` | `.elf` | `DEFERRED (Task 7a)` | Requires `ReferSemaStatus` (`0x47`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/sema/wait` | `.elf` | `DEFERRED (Task 7a)` | Requires `WaitSema` (`0x44`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/chpri` | `.elf` | `DEFERRED (Task 7a)` | Requires `ChangeThreadPriority` (`0x29`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/create` | `.elf` | `DEFERRED (Task 7a)` | Requires `CreateThread` (`0x20`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/delete` | `.elf` | `DEFERRED (Task 7a)` | Requires `DeleteThread` (`0x21`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/exit` | `.elf` | `DEFERRED (Task 7a)` | Requires `ExitThread` / `ExitDeleteThread` (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/ priority / rotate` | `.elf` | `DEFERRED (Task 7a)` | Requires `RotateThreadReadyQueue` (`0x2B`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/sleep` | `.elf` | `DEFERRED (Task 7a)` | Requires `SleepThread` (`0x32`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/start` | `.elf` | `DEFERRED (Task 7a)` | Requires `StartThread` (`0x22`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/stat` | `.elf` | `DEFERRED (Task 7a)` | Requires `ReferThreadStatus` (`0x30`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/suspend` | `.elf` | `DEFERRED (Task 7a)` | Requires `SuspendThread` / `ResumeThread` (`Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/terminate` | `.elf` | `DEFERRED (Task 7a)` | Requires `TerminateThread` (`0x25`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |
| `kernel/ee/thread/wake` | `.elf` | `DEFERRED (Task 7a)` | Requires `WakeupThread` (`0x33`, `Task 7a`) + `MMI`/`COP1` (`Task 4`). |

### 4. IOP CPU and IOP Kernel Suites (23 `.irx` suites)

| Suite (`tests/...`) | Binary | Task 3b Status | Dependency |
| :--- | :--- | :--- | :--- |
| `cpu/iop/alu` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `cpu/iop/branch` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `cpu/iop/branchdelay` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `cpu/iop/lsu` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `cpu/iop/lsudelay` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `cpu/iop/muldiv` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires R3000A IOP interpreter (`Task 5`) + `.irx` loader (`Task 7b`). |
| `kernel/iop/eventflag/clear` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thbase`/`thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/create` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/delete` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/poll` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/set` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/status` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/eventflag/wait` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thevent` HLE (`Task 7b`). |
| `kernel/iop/intrman/intr` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `intrman` HLE (`Task 7b`). |
| `kernel/iop/mbox/create` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/mbox/delete` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/mbox/poll` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/mbox/receive` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/mbox/send` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/mbox/status` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thmsgbx` HLE (`Task 7b`). |
| `kernel/iop/sema/Attr` / `create` / `delete` / `poll` / `signal` / `status` / `wait` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thsemap` HLE (`Task 7b`). |
| `kernel/iop/sysmem/alloc` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `sysmem` HLE (`Task 7b`). |
| `kernel/iop/thread/*` / `timer/*` | `.irx` | `DEFERRED (Task 5 / 7b)` | Requires IOP `thbase`/`timrman` HLE (`Task 7b`). |

### 5. Out-of-Phase-1 Suites (GS & VU Micro Mode: 5 `.elf` suites)

| Suite (`tests/...`) | Binary | Status | Reason Out of Phase 1 Scope |
| :--- | :--- | :--- | :--- |
| `gs/download` | `.elf` | `DEFERRED (Phase 2+)` | Requires GS VRAM transfer / `VIF1` / `GIF` download pipeline (Phase 2). |
| `vu/assemble` | `.elf` | `DEFERRED (Phase 2+)` | Requires VU0/VU1 micro-mode execution (`vcallms`) (Phase 2+). |
| `vu/lower` | `.elf` | `DEFERRED (Phase 2+)` | Requires VU0/VU1 micro-mode lower-instruction pipeline (Phase 2+). |
| `vu/triace` | `.elf` | `DEFERRED (Phase 2+)` | Requires VU micro-mode pipeline hazard emulation (Phase 2+). |
| `vu/upper` | `.elf` | `DEFERRED (Phase 2+)` | Requires VU0/VU1 micro-mode upper-instruction pipeline (Phase 2+). |
