# PS2 Emulator Roadmap Status (`docs/ROADMAP_STATUS.md`)

## Phase 1 Status: COMPLETE (`tag: phase1-done`)
- **Hardware Gate**: `35 / 35 PASS` (`23` EE CPU/COP0/COP1/MMI/MMU + `5` EE DMAC/SPR + `7` IOP R3000A).
- **LLE BIOS Boot (`Task 6b`)**: `PARTIAL` — reaches `EECONF` (`2,180,464` EE, `272,558` IOP cycles); stops at `IOP PC = 0x0000_AFA0` on CDVD `0x1F40_2005` (deferred to Phase 3 `ps2-cdvd`).

## Phase 2a Status: COMPLETE (`tags: phase2a-core`, `phase2a-done`, `51 / 52`, EFU partial)
- **Hardware Gate**: `51 / 52 PASSED`, `1 DEFERRED`, `0 FAILED` (`cpu/vu0_macro` 2, `vu/` 6/7 pass, `dma/vif` 6, `gs/` 2).
- **Deferred Suite (`vu/lower/efu.elf`)**: reason `"partial: 184/208 vectors; see HARDWARE_NOTES.md EFU section"` (`5` ops `16/16`, `8` ops `PARTIAL`, `0` fail).

## Speed Baseline (Release: `lto = "fat"`, `codegen-units = 1`)
- **Phase 1 Gate (`35` suites)**: `82,198,523` cycles in `1.408 s` (`58.38 Mcycles/s`).
- **Phase 2a Gate (`52` suites)**: `114,417,068` EE + `30,724` IOP = `114,447,792` cycles in `2.115 s` (`54.11 Mcycles/s`).

## `GUESS` / `SEARCH-ONLY` / Untested-by-Hardware Areas (`docs/HARDWARE_NOTES.md`)
- **EE/MCH Stubs (`GUESS`)**: `0x1000_F500` (`EeDmac::d_f500`) and non-`F430`/`F440` `RdramMch` slots (`TODO(verify)`).
- **GS Privileged (`SEARCH-ONLY` / `GUESS`)**: `GS_CSR` bits beyond `SIGNAL`/`FINISH` (`HSINT`, `VSINT`, `EDWINT`, `FLUSH`/`RESET`, `NFIELD`/`FIELD`, `FIFO`, `REV`/`ID = 0x551B`), second-`SIGNAL` stall, `GS_IMR`, `GS_BUSDIR` (`BUSDIR=1` error), display register latches.
- **GIF (`untested by hardware suites`)**: `IMT` intermittent preemption & `PATH3` save/restore (`GIF_P3CNT`/`GIF_P3TAG`), `M3R` boundary, `PSE`-gated debug reads, `REGLIST` odd-count discard, `PATH1` priority/masking, `PACKED` edge descriptors.
- **VIF (`untested` / `inferred`)**: `STAT.FQC` 4-QWC pre-decode staging buffer inference, `MPG` (`0x4A`, unit-tested only; `0x46` unresolved), `VIF1_NUM` 9-bit / `CLIP`+`FCSET` rules.
- **CPU/FPU/VU (`inferred` / Deferred)**: `BC1F`/`BC1T` chained delay slots; `ps2float` Booth-1 / `MADD` overflow / `(q25+1)>>1` rounding; `MAC`/`Status` flag pipelines deferred; `EFU` `vu_mul_rtz`, `x^2` stepping, RTZ `pi/4` (`0x3F49_0FDA`), and un-fitted `1..5` ULP / `2^-31` gaps.

## Known Timing Debt (Phase 4)
- Synchronous `finish_vu0_micro` on `MSCAL`/EE `COP2` stalls instead of cycle-interleaved EE/VU execution.
- Instantaneous stall-cycle drain inside `step_micro_liw` without proportional EE clock advance/stall.
- `VU1` cycle-accurate parallel timing and cycle-accurate stall backpressure deferred to Phase 4.

## Phase 2b Scope
- **GS Software Rasterizer (`ps2-gs`)**: Implement the platform-independent GS software renderer.
- **GS Dump Runner (`ps2-gs-runner`)**: Headless `.gs` dump replay and golden PNG pixel comparison.
- **`VU1 XGKICK -> GIF PATH1`**: `VU1` micromode `XGKICK` feeding `GIF PATH1` (`GIF_CNT[20..=29]`, `TOP`/`XTOP`).
