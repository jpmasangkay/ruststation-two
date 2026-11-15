# PS2 Emulator Roadmap Status (`docs/ROADMAP_STATUS.md`)

## Phase 1 Status: COMPLETE (`tag: phase1-done`)
- **Hardware Gate**: **`35 / 35 PASS`** (`23` EE CPU/COP0/COP1/MMI/MMU + `5` EE DMAC/SPR + `7` IOP R3000A).
- **Workspace Unit Tests**: **`60 / 60 PASS`** (`11` `ps2-common`, `45` `ps2-core`, `4` `ps2-autotest`).
- **LLE BIOS Boot (`Task 6b`)**: **`PARTIAL`** — reaches `EECONF` (`2,180,464` EE cycles, `272,558` IOP cycles); stops at `IOP PC = 0x0000_AFA0` on `0x1F40_2005` (`CDVD` S-commands `0x40`/`0x41`/`0x43` & NVM config, deferred to Phase 3 `ps2-cdvd`).

## Speed Baseline (Release Profile: `lto = "fat"`, `codegen-units = 1`)
- **Phase 1 Gate (`35` suites)**: `82,167,799` EE cycles + `30,724` IOP cycles = **`82,198,523` cycles in `1.408 s` (`58.38 Mcycles/s`)**.

## Phase 2a Gate (`17` New Suites -> `52 / 52` Cumulative Hardware Gate)
- **`cpu/vu0_macro` (2)**: `integer.elf`, `transfer.elf`.
- **`vu/` Micro & EFU (7)**: `games/triace.elf`, `lower/{branch,efu,fdivdelay,integer,random}.elf`, `upper/clip.elf`.
- **`dma/vif` (6)**: `basic.elf`, `intstall.elf`, `mskpath3.elf`, `stcycl.elf`, `stmod.elf`, `unpack.elf`.
- **`gs/` Privileged & GIF (2)**: `label.elf`, `signal.elf`.

## Active `GUESS` Stubs & Inferred Behaviors (`docs/HARDWARE_NOTES.md`)
- **`0x1000_F500` (`EeDmac::d_f500`)**: `GUESS` (`TODO(verify)`) — 32-bit R/W latch (reset `0`); write-only during BIOS boot (`EE PC = 0xBFC0_0838`), omitted in `ps2tek` `#dmacio`.
- **`0x1000_F400..=0x1000_F4F0` (`RdramMch::regs`, non-`F430`/`F440` slots)**: `GUESS` (`TODO(verify)`) — 32-bit R/W slots (reset `0`) for undocumented MCH timing registers (`0x1000_F410`, `0x1000_F420`, etc.); `0x1000_F430` (`MCH_RICM`) and `0x1000_F440` (`MCH_DRD`) are verified from `ps2tek` `#eerdram`.
- **Inferred Test-Vector Behaviors (`fits test vectors; may not generalize`)**:
  - `BC1F`/`BC1T` back-to-back delay-slot chaining (`ee/interpreter.rs`, `cpu/ee_fpu/branchdelay.elf`).
  - `ps2float` `MUL.S`/`MULA.S` low-Booth-group truncation, `MADD.S`/`MSUB.S` product-overflow immediate saturation, and `(q25 + 1) >> 1` rounding in `DIV.S`/`SQRT.S`/`RSQRT.S` (`ps2-common/src/ps2float.rs`).

## Known timing debt (Phase 4)
- **VU timing shortcuts (`bus/mod.rs`, `vu/pipeline.rs`)**:
  - Synchronous VU completion on `MSCAL`/`COP2` stalls (`finish_vu0_micro` / VU run-to-completion when stalled instead of cycle-accurate interleaved EE/VU execution).
  - Instantaneous VU stall draining inside `step_micro_liw` without advancing or stalling the EE clock proportionally.
  - Interleaved EE/VU races, `VU1` cycle-accurate parallel timing, and cycle-accurate stall backpressure are deferred to Phase 4 timing tightening.
