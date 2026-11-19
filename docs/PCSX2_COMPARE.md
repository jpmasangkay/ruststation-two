# PCSX2 Black-Box Screenshot Comparison Workflow

This document describes how to emit standalone `320x240` test ELFs from `.gsp` fixtures, capture
unfiltered PCSX2 **Software** GS screenshots under `local/pcsx2/`, and compare them against
`ps2-gs-runner` outputs.

> [!IMPORTANT]
> **Legal & Source Rule (`AGENTS.md`)**: PCSX2 (`GPL-3`) is strictly a black-box comparison tool.
> Never inspect PCSX2 source files, copy PCSX2 files into this repository, or commit anything under
> `local/`. Keep all PCSX2 screenshots, ELFs, and diff PNGs inside `local/pcsx2/`.

> [!NOTE]
> **Verification Status**: Menu tabs and setting labels in §2 below were verified from the owner's
> PCSX2 `v2.8.2` screenshots. Any item not yet verified against the local install (or left as a
> placeholder for the owner to fill in) is explicitly marked as unverified.

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

## 2. Capturing Unfiltered Screenshots in PCSX2 (`v2.8.2`)

1. Launch PCSX2 from the directory configured in `local/PCSX2_PATH.txt` and ensure a valid PS2 BIOS is configured (`Settings > BIOS` — `unverified` exact menu label).
2. Open **`Settings > Graphics`** (tabs: **`Display`**, **`Rendering`**, **`Post-Processing`**, **`Media Capture`**) and configure **Software** GS rendering before booting the ELF:
   - **`Graphics API`**: **`Software Renderer`**.
   - **`Display` tab**:
     - `Deinterlacing`: **`No Deinterlacing`**
     - `Bilinear Filtering`: **`None`**
     - `Anti-Blur`: **off**
     - `Integer Scaling`: **off**
     - `Screen Offsets`: **off**
     - `Show Overscan`: **off**
     - Crop: **`0`**
   - **`Rendering` tab**:
     - `Texture Filtering`: **`Nearest`**
     - `Software Rendering Threads`: **`3`**
     - `Auto Flush`: **on**
     - `Mipmapping`: **off**
   - **`Post-Processing` tab**: all **off**.
   - **`Media Capture` tab**:
     - `Screenshot Capture Setup` -> `Resolution`: **`Internal Resolution (No Aspect Correction)`**
     - Format: **`PNG`**
3. Boot each generated ELF via **`Start File`** (CLI `--elf="<path>"` — `unverified`). Wait ~1 second for the static frame to settle.
4. Capture a screenshot and copy the resulting PNG into `local/pcsx2/<name>_pcsx2.png`:
   - Screenshot hotkey: `<owner to fill in>` (`unverified`).
   - Screenshots saved to: `<owner to fill in>` (`unverified`).

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

### Step 3a Checkpoint Sprites (`76800/76800` exact on PCSX2 `v2.8.2` Software) & Textured-Triangle Probes (`14`–`16`)

Settings (unchanged): PCSX2 `v2.8.2`, `Software` renderer, `Deinterlacing: No Deinterlacing`, `Bilinear Filtering: None`, `Anti-Blur` off, `Auto Flush` on, `Software Rendering Threads: 3`, `Texture Filtering: Nearest`, `Mipmapping` off, no post-processing, screenshots `Internal Resolution (No Aspect Correction)`, PNG.

Emit ELFs and `*_ours.png` for the three Step 3a textured-triangle probes (`tests/gs_probes/14..16_*.gsp`, predictions frozen in `docs/GS_GUESSES.md` commit `98ea99d`):

```powershell
cargo run -p ps2-gs-runner -- --emit-elf tests/gs_probes/14_tex_tri_wide_uv.gsp         -o local/pcsx2/14_tex_tri_wide_uv.elf
cargo run -p ps2-gs-runner -- tests/gs_probes/14_tex_tri_wide_uv.gsp                    -o local/pcsx2/14_tex_tri_wide_uv_ours.png
cargo run -p ps2-gs-runner -- --emit-elf tests/gs_probes/15_tex_tri_stq_perspective.gsp -o local/pcsx2/15_tex_tri_stq_perspective.elf
cargo run -p ps2-gs-runner -- tests/gs_probes/15_tex_tri_stq_perspective.gsp            -o local/pcsx2/15_tex_tri_stq_perspective_ours.png
cargo run -p ps2-gs-runner -- --emit-elf tests/gs_probes/16_tex_tri_narrow.gsp          -o local/pcsx2/16_tex_tri_narrow.elf
cargo run -p ps2-gs-runner -- tests/gs_probes/16_tex_tri_narrow.gsp                     -o local/pcsx2/16_tex_tri_narrow_ours.png
```

Expected PCSX2 screenshot filenames under `local/pcsx2/`:
- `local/pcsx2/14_tex_tri_wide_uv_pcsx2.png` (from `local/pcsx2/14_tex_tri_wide_uv.elf`)
- `local/pcsx2/15_tex_tri_stq_perspective_pcsx2.png` (from `local/pcsx2/15_tex_tri_stq_perspective.elf`)
- `local/pcsx2/16_tex_tri_narrow_pcsx2.png` (from `local/pcsx2/16_tex_tri_narrow.elf`)

Compare commands once screenshots are saved:

```powershell
cargo run -p ps2-gs-runner -- tests/gs_probes/14_tex_tri_wide_uv.gsp         --compare-png local/pcsx2/14_tex_tri_wide_uv_pcsx2.png         --diff-out local/pcsx2/14_tex_tri_wide_uv_diff.png         --max-mismatches 16
cargo run -p ps2-gs-runner -- tests/gs_probes/15_tex_tri_stq_perspective.gsp --compare-png local/pcsx2/15_tex_tri_stq_perspective_pcsx2.png --diff-out local/pcsx2/15_tex_tri_stq_perspective_diff.png --max-mismatches 16
cargo run -p ps2-gs-runner -- tests/gs_probes/16_tex_tri_narrow.gsp          --compare-png local/pcsx2/16_tex_tri_narrow_pcsx2.png          --diff-out local/pcsx2/16_tex_tri_narrow_diff.png          --max-mismatches 16
```

Add `--tolerance 1` to permit `±1` channel LSB differences (`PMODE.ALP` scaling or fixed-point rounding) while still catching `>1` coverage/color errors.

### Output & Diff Color Legend
- **Console output**: matched size, reference PNG dimensions, detected uniform `(sx, sy)` integer upscale and black-border `(crop_x, crop_y)` alignment, exact pixel `%`, `±1` count, `>1` count, max `|ΔR|, |ΔG|, |ΔB|`, `MAE` / `RMSE`, up to `16` sample mismatches (`ours=#RRGGBB pcsx2=#RRGGBB`), and a `shift_hint` if shifting by `(dx, dy)` reduces mismatches.
- **Diff PNG (`--diff-out`)**: `0.25x` darkened reference pixels where exact; **yellow (`#FFE040`)** for `max(|ΔR|, |ΔG|, |ΔB|) == 1`; **bright magenta (`#FF2080`)** for `> 1`.
- **Non-uniform scaling error**: aborts cleanly if PCSX2 captured display-scaled / bilinear-filtered output; verify **`Display` > `Bilinear Filtering: None`**, **`Rendering` > `Texture Filtering: Nearest`**, and **`Media Capture` > `Screenshot Capture Setup` > `Resolution: Internal Resolution (No Aspect Correction)`**.

### How to Read Diagnostic Outcomes Against `docs/GS_GUESSES.md`
1. **Whole-image uniform `-1` LSB (yellow across all colored pixels in `04_sprite_flat`)**: points to `G-016` CRTC `PMODE.ALP = 0xFF` (`255/256`) scaling, not Gouraud interpolation. (~50% brightness indicates `MMOD=0` or `ALP=0x80` regression.)
2. **`08_scissor_clip` `shift_hint`**:
   - If the `SCISSOR_1` rectangle `[40..=279] x [30..=209]` shifted together with the interior sprites/triangle: points to **`G-016`** CRTC `DISPLAY1.DX/DY` border offset, not rasterizer sample centers.
   - If `SCISSOR_1` borders stayed pinned at `(40, 30)..(279, 209)` while sprites/triangles shifted by `(±1, 0)` or `(0, ±1)` inside: points to **`G-009`/`G-010`/`G-011`** sample center (`HalfPixelCenter (+0.5)` vs `IntegerCenter (+0.0)`).
3. **`06_triangle_gouraud` yellow pixels while `04` & `05` are 100% exact**: isolates **`G-012`** fixed-point barycentric RGB interpolation rounding.
4. **`10_subpixel_probe` pixels**: compare against the pre-measurement predictions frozen in `docs/GS_GUESSES.md` (`Probe Predictions` section) for Points `P0..P6` (`G-009`), Lines `L0..L5` (`G-011`), and normal/reversed/subpixel Sprites `S0..S2` (`G-010`).

---

## 4. Two-Attempt Time Box & Fallback Rule

- Cap manual PCSX2 GUI screenshot troubleshooting for any new probe batch at **2 attempts** (for example: an ELF hangs on a future display/CRTC setup, or a PCSX2 update alters internal-resolution capture behavior).
- If unblocked screenshots cannot be captured cleanly within 2 attempts, log the exact observed blocker in `docs/HARDWARE_NOTES.md` and proceed with the remaining Phase 2b work (Step 3b/3c fragment features and Step 4 verification using hand-derived + `ps2tek`/`Play!` tests) without holding up the roadmap; `--emit-elf`, `--compare-png`, and the automated `EE -> GIF -> GS` gate self-check remain active regression checks regardless.
