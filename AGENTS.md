# PS2 Emulator (Rust) — Agent Rules

## Source of truth
- `docs/PS2_BLUEPRINT.md` is the engineering blueprint (auto-converted from PDF; tables may be noisy). Read the relevant section before starting any task. Do not load the whole file at once.
- Sections: 3 = architecture/crates, 5 = heavy games, 6 = CPU/VU perf, 7 = GS rendering, 10 = roadmap + phase gates, 12 = starter code.
- If the blueprint conflicts with primary sources (GBATEK-equivalent PS2 docs, PCSX2/Play! source, ps2autotests), the primary source wins. Flag the conflict in the plan.

## Project
- Language: Rust (stable). Dev machine: Windows. Must also build on Linux and macOS.
- Workspace crates (blueprint §3 and §12): `ps2-common`, `ps2-jit`, `ps2-cdvd`, `ps2-gs`, `ps2-spu2`, `ps2-core`, `ps2-gs-runner`, `ps2-autotest`, `ps2-ui-desktop`, `ps2-ui-android`.
- `ps2-core` is platform-independent. No windowing, audio device, or filesystem-UI code in core.
- Acyclic crate dependencies only. Do not add a crate dependency edge without stating why in the plan.

## Work process
- Work one roadmap phase at a time (blueprint §10). Do not start the next phase until the current phase's Definition of Done and performance gate pass.
- Planning mode for every new subsystem. The plan must list: files to create or change, test ROMs or suites to run, and the acceptance check.
- Small, reviewable steps. Each task ends with `cargo fmt`, `cargo clippy -- -D warnings`, and `cargo test` passing.
- Correctness first, then speed. Build the interpreter before the JIT. Keep the interpreter as the reference for differential testing against the JIT.
- Every bug fix gets a regression test (ps2autotests case, unit test, or `.gs` dump replay).

## Code rules
- Avoid `dyn Trait` and heap allocation in hot paths (CPU loop, VU, GIF, DMA). Prefer generics and monomorphization.
- `unsafe` only in `ps2-common` (virtual memory), `ps2-jit`, and fastmem paths. Every `unsafe` block needs a `// SAFETY:` comment.
- Keep hot state `#[repr(C)]` and cache-line aligned where the JIT indexes it by offset.
- No `unwrap()` or `expect()` outside tests and startup code.
- Use `tracing` for logging. No `println!` in core crates.
- Release profile: `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`.
- Never commit toolchain, linker, or environment workarounds. If the build breaks, report the error and ask.

## Legal and content rules
- Never commit BIOS dumps, game images, or memory cards. Keep them in a gitignored `local/` directory.
- Do not copy code from PCSX2 (GPL-3). Read it for behavior and write independent code. Note any ported algorithm and its source in a comment.
- Do not add dependencies with GPL/AGPL licenses to the workspace without asking.

## Verification
- Phase 1: ps2autotests via `ps2-autotest` CLI, headless. Run all 35 Phase 1 hardware gate suites with:
  `cargo run --release -p ps2-autotest -- --gate`
  (prints the 35-suite table and exits nonzero on any failure).
- Phase 2+: `.gs` dump replay via `ps2-gs-runner`, pixel-compare against golden PNGs.
- Performance: measure frame-time percentiles (P99), not only average FPS. Use fixed save states or dumps for repeatable benchmarks.
- Never claim a test passes without running it and showing the output.
