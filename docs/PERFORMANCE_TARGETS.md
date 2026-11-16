# Performance Targets & Reference Devices (`docs/PERFORMANCE_TARGETS.md`)

> **Current Phase Scope (Phase 2b):** Only the headless interpreters (`EE`, `IOP`, `VU0`, `VU1`), `DMAC`/`VIF`/`GIF`, and the reference single-threaded CPU software GS (`ps2-gs`) exist today. Dynamic recompilers (`ps2-jit`, Phase 3), multi-threaded `MTVU`/`MTGS` + `wgpu` GPU hardware rendering (Phase 4), and `ARM64` JITs (Phase 6) are not yet built. Cross-machine measurements today cover **headless interpreter throughput (`ps2-autotest --bench`) only**.

---

## Target Cadence & Measurement Rules ([PS2_BLUEPRINT.md](PS2_BLUEPRINT.md) §10–11)

- **Full-speed wall-clock targets:**
  - **NTSC:** `59.94 Hz` (`<= 16.68 ms` frame budget; blueprint targets `P99 < 16.67 ms` for Phase 4 full-speed desktop hardware rendering).
  - **PAL:** `50.00 Hz` (`<= 20.00 ms` frame budget).
- **Percentile discipline:** Always report frame-time percentiles (**`P50` median**, **`P99`**, and **`P99.9` worst-spike**) alongside wall-clock speed %, rather than average FPS alone.
- **Deterministic inputs:** Measure interpreter throughput on reproducible `ps2autotests` suites today, and measure future GS/full-game benchmarks on fixed `.gs` dumps or deterministic in-game save states + recorded input traces (never cold disc boots).

---

## Reference devices

| Tier / Role | Device & OS | Performance Goal | Status / Label |
| :--- | :--- | :--- | :--- |
| **Developer machine** | AMD Ryzen 9 8945HX with Radeon Graphics (`16C / 32T`, OS-reported `MaxClockSpeed: 2501 MHz`), `31.8 GiB` RAM (`34,142,187,520` bytes), NVIDIA GeForce RTX 5070 Laptop GPU (`8151 MiB`, driver `616.92`), Microsoft Windows 11 Home (`10.0.26300`, `64-bit`) | **Measured baseline only** (development & regression reference; **not** a minimum spec target) | **Measured** (headless Phase 1+2a interpreter baseline below; JIT/GPU unbuilt) |
| **Tier A** | Asus laptop: Intel Core i7-8750H (`6C / 12T`), NVIDIA GTX 1050 Ti `4 GB`, `8 GB` DDR4, Windows | **Heavy games at native resolution (`1x`) at full speed** (`59.94 Hz` NTSC / `50.00 Hz` PAL) | **Interpreter baseline measured** (`27.68 Mcycles/s`, CPU model/power state unconfirmed; heavy `1x` game target remains **unverified aspiration** until JIT + GPU exist) |
| **Tier B** | Raspberry Pi 5 (`ARM64` Linux SBC; Blueprint §6 notes BCM2712 `4x Cortex-A76 @ 2.4 GHz`) | **Light/medium games; heavy best-effort** (e.g., native `1x`, with `75%` EE cycle rate / minimal blending fallback if needed in Phase 6) | **Unverified target** (Phase 6 `ARM64` JIT) |
| **Tier C** | Raspberry Pi 4 (`ARM64` Linux SBC) | **Stretch** (exploratory / lightweight homebrew only; full-speed commercial games not guaranteed) | **Unverified stretch goal** |

### Tier A Notes (Asus laptop: i7-8750H / GTX 1050 Ti 4 GB / 8 GB DDR4 / Windows)

- **Label:** Treat heavy games at `1x` native resolution at full speed on Tier A as an **unverified aspiration until measured** with actual interpreter, JIT, and GPU benchmarks on that hardware.
- **Laptop thermals:**
  - Always **benchmark plugged in** to AC power and record the active Windows power mode/plan (plus any OEM fan/performance mode).
  - Run multi-iteration passes (`ps2-autotest --bench` runs `3` passes by default) and **record whether the laptop thermally throttles** (e.g., noticeable slowdown from `Run 1` to `Run 3` or sustained CPU clock drop under load).
- **GPU API support to verify (do not assume):**
  - When Phase 4 (`wgpu` / Vulkan / Direct3D 12 GS hardware renderer) is built, **verify actual Vulkan and D3D12 device/driver feature support** on the target Windows GTX 1050 Ti driver (`4 GB` VRAM) rather than assuming feature availability — specifically `DUAL_SOURCE_BLENDING` (`dualSrcBlend`), fallback ordered/interlock features (Vulkan `VK_EXT_fragment_shader_interlock` / D3D12 ROV) for recursive blending/`DATE`/`FBMASK`/`COLCLIP`, and VRAM footprint inside `4 GB`.
- **Memory headroom (`8 GB` DDR4):** Track host memory consumption across fastmem backing, texture cache staging, shader pipelines, and Windows OS overhead.

---

## Measured Baseline — Developer Machine (Current Headless Interpreter)

Measured after final Phase 2b audit formatting using the release profile (`opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`), **Plugged in (AC)** on Windows **`Balanced`** power plan:

- **Command:** `cargo run --release -p ps2-autotest -- --bench --tests-dir local/ps2autotests`
- **Suites executed:** `52` (`35` Phase 1 + `17` Phase 2a; `51/52 PASSED`, `1 DEFERRED` [`vu/lower/efu.elf`], `0 FAILED`)
- **Cycles per run:** `114,417,068` EE + `30,724` IOP = `114,447,792` total (`Phase 1`: `81,148,523` cycles; `Phase 2a`: `33,299,269` cycles)
- **Measured runs (3 passes):**
  - `Run 1`: `2.181 s` (`52.48 Mcycles/s`) — Phase 1: `1.556 s`, Phase 2a: `0.625 s`
  - `Run 2`: `2.097 s` (`54.59 Mcycles/s`) — Phase 1: `1.459 s`, Phase 2a: `0.637 s`
  - `Run 3`: `2.148 s` (`53.28 Mcycles/s`) — Phase 1: `1.526 s`, Phase 2a: `0.622 s`
- **Summary:** **Median `2.148 s` (`53.28 Mcycles/s`)**, Best `2.097 s` (`54.59 Mcycles/s`), Worst `2.181 s` (`52.48 Mcycles/s`)
  - Phase 1 breakdown: median `1.526 s` (`53.17 Mcycles/s`), best `1.459 s` (`55.60 Mcycles/s`)
  - Phase 2a breakdown: median `0.625 s` (`53.31 Mcycles/s`), best `0.622 s` (`53.55 Mcycles/s`)
  - **EE-only real-time ratio (interpreter):** `0.18` (headless interpreter vs `294.912 MHz` EE clock; excludes VU, GS, audio).

---

## Measured Baseline — Tier A Laptop (Headless Interpreter, Preliminary)

Preliminary measurement on the Tier A Windows laptop (**exact CPU model and power state / AC vs battery not yet reported**):

- **Command:** `ps2-autotest --bench` (`3` runs)
- **Correctness:** `51/52 PASSED`, `1 DEFERRED` (`vu/lower/efu.elf`), `0 FAILED` — identical to the developer machine.
- **Timing summary (`3` passes):**
  - **Median:** `4.134 s` (`27.68 Mcycles/s`)
  - **Best:** `4.055 s` (`28.22 Mcycles/s`)
  - **Worst:** `5.355 s` (`21.37 Mcycles/s`)
- **Cross-machine comparison & EE-only real-time arithmetic:**
  - **Throughput ratio vs developer machine:** **`0.52`**
  - **EE-only real-time ratio (interpreter):** **`0.094`** (developer machine: **`0.18`**)
  - **Speedup needed for `1.0x` EE-only real time:** **about `10.7x`** (arithmetic only; excludes VU, GS, and audio; makes no prediction of what a JIT would achieve).
- **Notes & pending follow-up:**
  - **Run 3 slowdown:** `Run 3` was **`24%` slower than `Run 1`** (cause unconfirmed: possible thermal throttling or power-saving state).
  - **Scope caveat:** Headless interpreter test (`ps2autotests`), **not** a game benchmark.
  - **Pending:** Re-measurement plugged in to AC power with `--bench-runs 10` (recording exact CPU model and active Windows power plan).

To collect matching measurements on another Windows machine without installing Rust, follow [BENCH_ON_ANOTHER_MACHINE.md](BENCH_ON_ANOTHER_MACHINE.md).
