# PCSX2 Black-Box Screenshot Comparison Workflow (Step 2.5)

This document describes how to emit standalone `320x240` test ELFs from `.gsp` fixtures, capture
unfiltered PCSX2 **Software** GS screenshots under `local/pcsx2/`, and compare them against
`ps2-gs-runner` outputs.

> [!IMPORTANT]
> **Legal & Source Rule (`AGENTS.md`)**: PCSX2 (`GPL-3`) is strictly a black-box comparison tool.
> Never inspect PCSX2 source files, copy PCSX2 files into this repository, or commit anything under
> `local/`. Keep all PCSX2 screenshots, ELFs, and diff PNGs inside `local/pcsx2/`.

> [!NOTE]
> **Verification Status (`SEARCH-ONLY`)**: PCSX2 was **not** launched during Step 2.5 harness
> creation. Every PCSX2 menu path, setting name, screenshot option, and hotkey listed below comes
> from public web documentation (`SEARCH-ONLY`) and is **`DRAFT, unverified`** against the local
> binary at `C:\Users\My Laptop\Downloads\PCSX2`.

---

## 1. Emit Standalone Test ELFs & Reference PNGs

All generated files belong under the gitignored `local/pcsx2/` directory:

```powershell
cargo run -p ps2-gs-runner -- tests/gs_render/04_sprite_flat.gsp      --emit-elf -o local/pcsx2/04.elf --disasm-stub
cargo run -p ps2-gs-runner -- tests/gs_render/05_triangle_flat.gsp    --emit-elf -o local/pcsx2/05.elf
cargo run -p ps2-gs-runner -- tests/gs_render/06_triangle_gouraud.gsp --emit-elf -o local/pcsx2/06.elf
cargo run -p ps2-gs-runner -- tests/gs_render/08_scissor_clip.gsp     --emit-elf -o local/pcsx2/08.elf
cargo run -p ps2-gs-runner -- tests/gs_probes/10_subpixel_probe.gsp   --emit-elf -o local/pcsx2/10.elf
```

Optional reference PNG dumps from `ps2-gs-runner` (for visual inspection):

```powershell
cargo run -p ps2-gs-runner -- tests/gs_render/04_sprite_flat.gsp      -o local/pcsx2/04_ours.png
cargo run -p ps2-gs-runner -- tests/gs_render/05_triangle_flat.gsp    -o local/pcsx2/05_ours.png
cargo run -p ps2-gs-runner -- tests/gs_render/06_triangle_gouraud.gsp -o local/pcsx2/06_ours.png
cargo run -p ps2-gs-runner -- tests/gs_render/08_scissor_clip.gsp     -o local/pcsx2/08_ours.png
cargo run -p ps2-gs-runner -- tests/gs_probes/10_subpixel_probe.gsp   -o local/pcsx2/10_ours.png
```

What each generated ELF does (`G-016`):
- Writes `6` GS privileged PCRTC registers (`PMODE = 0x0000_0000_0000_FFA5`,
  `SMODE1 = 0x0000_0007_4080_4504`, `SMODE2 = 0`, `DISPFB1 = 0x0000_0000_0000_0A00`,
  `DISPLAY1 = 0x000E_F9FF_0381_927C`, `BGCOLOR = 0`) for a `320x240` non-interlaced display where
  fixed `MMOD=1, SLBG=1, ALP=0xFF` keeps `A=0x80` pixels undimmed.
- Streams every GS packet/register write through `GIF_FIFO` (`0x1000_6000`), polling `GIF_STAT.FQC < 15`.
- Enters a self-loop (`beq $zero, $zero, -1`) keeping the image static on screen with zero syscalls.

---

## 2. Capturing Unfiltered Screenshots in PCSX2 (`DRAFT, unverified`)

Every menu name, option label, and hotkey in Steps 2–4 below is **`DRAFT, unverified`** (`SEARCH-ONLY` web docs; exact labels in your installed Qt version may differ slightly):

1. Launch PCSX2 from the directory configured in `local/PCSX2_PATH.txt` and ensure a valid PS2 BIOS is configured (`Settings > BIOS` — **`DRAFT, unverified`**).
2. Configure **Software** GS rendering before booting the ELF (`Settings > Graphics` — all item names below **`DRAFT, unverified`**):
   - **Rendering > Renderer**: select **`Software`** (or toggle in-game with **`F9`** — **`DRAFT, unverified`**).
   - **Rendering > Internal Resolution**: **`1x (Native)`** (`Software` mode inherently uses `1x` — **`DRAFT, unverified`**).
   - **Display > Filtering / Bilinear / Sharp Bilinear**: set **`Nearest`** or disable display smoothing/filtering (**`DRAFT, unverified`**) so screenshots do not blur texels.
   - **Display > Aspect Ratio / Cropping**: prefer unstretched / internal-resolution screenshot capture if available; check **`Settings > Hotkeys > Graphics`** (**`DRAFT, unverified`**) for a raw/unscaled **`Save Internal Resolution Screenshot`** binding alongside default **`F8`** (**`DRAFT, unverified`**, legacy docs also cite **`F12`** — **`DRAFT, unverified`**).
3. Boot each generated ELF via **`System > Boot ELF...`** / **`System > Start File...`** (**`DRAFT, unverified`**) or CLI `--elf="<path>"` (**`DRAFT, unverified`**). Wait ~1 second for the static frame to settle.
4. Press the raw screenshot hotkey (**`F8`** / internal-res hotkey — **`DRAFT, unverified`**) and copy the resulting PNG from PCSX2's configured `snaps/` folder into `local/pcsx2/<NN>_pcsx2.png` (`04_pcsx2.png`, `05_pcsx2.png`, `06_pcsx2.png`, `08_pcsx2.png`, `10_pcsx2.png`).

---

## 3. Running `ps2-gs-runner --compare-png` & Interpreting Results

Run the built-in comparator once screenshots exist under `local/pcsx2/`:

```powershell
cargo run -p ps2-gs-runner -- tests/gs_render/04_sprite_flat.gsp      --compare-png local/pcsx2/04_pcsx2.png --diff-out local/pcsx2/04_diff.png
cargo run -p ps2-gs-runner -- tests/gs_render/05_triangle_flat.gsp    --compare-png local/pcsx2/05_pcsx2.png --diff-out local/pcsx2/05_diff.png
cargo run -p ps2-gs-runner -- tests/gs_render/06_triangle_gouraud.gsp --compare-png local/pcsx2/06_pcsx2.png --diff-out local/pcsx2/06_diff.png
cargo run -p ps2-gs-runner -- tests/gs_render/08_scissor_clip.gsp     --compare-png local/pcsx2/08_pcsx2.png --diff-out local/pcsx2/08_diff.png
cargo run -p ps2-gs-runner -- tests/gs_probes/10_subpixel_probe.gsp   --compare-png local/pcsx2/10_pcsx2.png --diff-out local/pcsx2/10_diff.png
```

Add `--tolerance 1` to permit `±1` channel LSB differences (`PMODE.ALP` scaling or fixed-point rounding) while still catching `>1` coverage/color errors.

### Output & Diff Color Legend
- **Console output**: matched size, reference PNG dimensions, detected uniform `(sx, sy)` integer upscale and black-border `(crop_x, crop_y)` alignment, exact pixel `%`, `±1` count, `>1` count, max `|ΔR|, |ΔG|, |ΔB|`, `MAE` / `RMSE`, up to `16` sample mismatches (`ours=#RRGGBB pcsx2=#RRGGBB`), and a `shift_hint` if shifting by `(dx, dy)` reduces mismatches.
- **Diff PNG (`--diff-out`)**: `0.25x` darkened reference pixels where exact; **yellow (`#FFE040`)** for `max(|ΔR|, |ΔG|, |ΔB|) == 1`; **bright magenta (`#FF2080`)** for `> 1`.
- **Non-uniform scaling error**: aborts cleanly if PCSX2 captured display-scaled / bilinear-filtered output; verify **`Nearest`** / internal-resolution screenshot settings (**`DRAFT, unverified`**).

### How to Read Diagnostic Outcomes Against `docs/GS_GUESSES.md`
1. **Whole-image uniform `-1` LSB (yellow across all colored pixels in `04_sprite_flat`)**: points to `G-016` CRTC `PMODE.ALP = 0xFF` (`255/256`) scaling, not Gouraud interpolation. (~50% brightness indicates `MMOD=0` or `ALP=0x80` regression.)
2. **`08_scissor_clip` `shift_hint`**:
   - If the `SCISSOR_1` rectangle `[40..=279] x [30..=209]` shifted together with the interior sprites/triangle: points to **`G-016`** CRTC `DISPLAY1.DX/DY` border offset, not rasterizer sample centers.
   - If `SCISSOR_1` borders stayed pinned at `(40, 30)..(279, 209)` while sprites/triangles shifted by `(±1, 0)` or `(0, ±1)` inside: points to **`G-009`/`G-010`/`G-011`** sample center (`HalfPixelCenter (+0.5)` vs `IntegerCenter (+0.0)`).
3. **`06_triangle_gouraud` yellow pixels while `04` & `05` are 100% exact**: isolates **`G-012`** fixed-point barycentric RGB interpolation rounding.
4. **`10_subpixel_probe` pixels**: compare against the pre-measurement predictions frozen in `docs/GS_GUESSES.md` (`Probe Predictions` section) for Points `P0..P6` (`G-009`), Lines `L0..L5` (`G-011`), and normal/reversed/subpixel Sprites `S0..S2` (`G-010`).

---

## 4. Two-Attempt Time Box & Fallback Rule

- Cap manual PCSX2 GUI screenshot troubleshooting at **2 attempts** (for example: BIOS setup missing, PCSX2 hangs without an explicit `SetGsCrt` syscall, or the installed PCSX2 build forces non-integer window scaling on all screenshots).
- If unblocked screenshots cannot be captured cleanly within 2 attempts, log the exact observed blocker in Step 4 and continue with Step 3a (`ps2-gs` textures/CLUT/alpha/fog/dither using hand-derived + `ps2tek`/`Play!` tests) without holding up the roadmap; `--emit-elf`, `--compare-png`, and the automated `EE -> GIF -> GS` gate self-check remain valuable regression checks regardless.
