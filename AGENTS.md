# PS2 Emulator (Rust) — Agent Rules

## Source of truth & precedence

- Project rules and `docs/PS2_BLUEPRINT.md` always take precedence over global `.gemini` rules on architecture, scheduling, and crate boundaries.
- `docs/PS2_BLUEPRINT.md` is the engineering blueprint (auto-converted from PDF; tables may be noisy). Read only the relevant section before starting any task—do not load the whole file at once.
  - Sections: 3 = architecture/crates, 5 = heavy games, 6 = CPU/VU perf, 7 = GS rendering, 10 = roadmap + phase gates, 12 = starter code.
- Track active status and open assumptions in `docs/ROADMAP_STATUS.md`, `docs/HARDWARE_NOTES.md`, `docs/GS_GUESSES.md`, and `docs/PERFORMANCE_TARGETS.md`.
- If the blueprint conflicts with primary sources (`ps2tek`, `ps2sdk`, `Play!` BSD-2, `ps2autotests`, or black-box hardware/PCSX2 output comparisons), the primary source wins. Flag the conflict in the plan and label every citation `FETCHED`, `SEARCH-ONLY`, or `GUESS`.

## Project

- Language: Rust (stable). Dev machine: Windows. Must also build on Linux and macOS.
- Workspace crates (blueprint §3 and §12): `ps2-common`, `ps2-jit`, `ps2-cdvd`, `ps2-gs`, `ps2-spu2`, `ps2-core`, `ps2-gs-runner`, `ps2-autotest`, `ps2-ui-desktop`, `ps2-ui-android`.
- `ps2-core` and `ps2-gs` core logic are platform-independent. No windowing, audio device, or GUI framework code in core.
- Acyclic crate dependencies only. Do not add a crate dependency edge without stating why in the plan.

## Work process

- Work one roadmap phase at a time (`docs/ROADMAP_STATUS.md`, blueprint §10). Do not start the next phase until the current phase's Definition of Done and gate pass.
- Planning mode for every new subsystem. The plan must list: files to create or change, test ROMs/suites/probes to run, and the acceptance check.
- Small, reviewable steps. Each task ends with `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` passing.
- Correctness first, then speed. Build the interpreter and reference SW GS before JIT and HW GS; keep both as differential testing oracles.
- Register every unconfirmed hardware assumption in `docs/GS_GUESSES.md` or `docs/HARDWARE_NOTES.md`. For new PCSX2 black-box probes, freeze hand-derived predictions in a standalone commit before emitting ELFs or capturing screenshots.
- Every bug fix gets a regression test (`ps2autotests` case, unit test, or `.gsp`/`.gs` replay).

## Code & performance architecture rules (Beat-PCSX2 & Tier A Targets)

- Target Tier A (`i7-8750H`, `GTX 1050 Ti 4 GB`, `8 GB` RAM) for `1x` full speed (`P99 < 16.67 ms` NTSC / `20.00 ms` PAL) and modern desktops for `3x+` locked `60 FPS` without stalling on CPU/GPU sync.
- Avoid `dyn Trait`, heap allocations, and `println!` in hot paths (CPU loop, VU, VIF, GIF, DMA, GS raster). Prefer generics, monomorphization, and `tracing`.
- `unsafe` only in `ps2-common` (virtual memory/fastmem), `ps2-jit`, and explicit SIMD/fastmem paths. Every `unsafe` block requires a `// SAFETY:` comment.
- Keep hot CPU/VU state `#[repr(C, align(64))]` so JIT offsets stay within compact 8/12-bit displacements.
- **Phase 3–4 JIT & Concurrency Invariants:**
  - Use event-driven downcounting cycle batches (`cycle_countdown`) in `ps2-jit`—never tick timers per instruction in compiled code.
  - Design `ps2-jit` for 4 GiB fastmem + fault backpatching (16 KB host-page safe), direct block linking, inline indirect-jump LUTs, whole-microprogram VU1 dead-flag elimination, and native AVX2/NEON SIMD for EE MMI/VU vectors.
  - Treat per-game accuracy toggles (`ps2float` fallback, FPU/VU clamping, timing fixes) as JIT/pipeline specialization constants rather than runtime `if` branches in hot loops.
  - Communicate across `EE -> MTVU -> MTGS -> Audio` via 64-byte-aligned (`#[repr(align(64))]`) lock-free SPSC ring buffers; never acquire a mutex inside the frame loop.
  - Design Phase 4 HW GS around native Vulkan (`ash`) with 8 KB page-granular texture caching, draw buffering/primitive coalescing, Dual-Source Blending, shader-folded Channel Shuffles, and explicit driver-feature verification (never assume ROV/interlock speed on Pascal/Tier A).
- No `unwrap()` or `expect()` outside tests and startup code.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`.
- Never commit toolchain, linker, or environment workarounds. If the build breaks, report the error and ask.

## Legal and content rules

- Never commit BIOS dumps, game images, memory cards, or PCSX2 outputs. Keep them in the gitignored `local/` directory.
- PCSX2 (`GPL-3`) is strictly a **black-box comparison tool only**. Never read its source, never copy any of its files into the repo, and never modify its install. Its path is in `local/PCSX2_PATH.txt`. All PCSX2 ELFs, screenshots, and diffs go under `local/`.
- Clean-room references permitted with attribution in `THIRD_PARTY_NOTICES.md`: `ps2tek`, `ps2sdk` (`AFL-2.0`), `Play!` (`BSD-2-Clause`), and `ps2autotests` (`ISC`).
- Do not add dependencies or bundled data files (such as `GameIndex.yaml`) with GPL/AGPL licenses to the repository without asking; load any external user-supplied compatibility DB at runtime.

## Verification

- **Hardware & GS Gates:** Run `cargo run --release -p ps2-autotest -- --gate` (Phase 1 `35/35` + Phase 2a `51/52, 1 DEFERRED` + Phase 2b GS render/ELF gates; exits nonzero on any failure).
- **GS Replay & Black-Box Compare:** Replay `.gsp`/`.gs` streams via `ps2-gs-runner` (`--emit-elf` and `--compare-png` per `docs/PCSX2_COMPARE.md`). Follow the 2-attempt cap on PCSX2 GUI troubleshooting.
- **Performance:** Measure interpreter baselines with `cargo run --release -p ps2-autotest -- --bench --tests-dir local/ps2autotests`, and measure Phase 3+ game/GS benchmarks using deterministic save states or `.gs` dumps reporting `P50`, `P99`, and `P99.9` frame times (`docs/PERFORMANCE_TARGETS.md`).
- Never claim a test or benchmark passes without running it and showing the output.
