# `.gsp` GS Packet Stream Format

`.gsp` is a plain ASCII packet-stream test script consumed by `ps2_gs::packet::{parse_gsp_script, execute_gsp_script}` and the standalone `ps2-gs-runner` CLI (`crates/ps2-gs-runner`).

## Lexical Rules
- Line-based UTF-8 / ASCII text.
- Comments start with `#` or `//` and run to end of line. Blank lines are ignored.
- Unsigned integer literals accept decimal (`42`) or hex with optional underscores (`0x1302_0020_0000_0000`).

## Directives

| Directive | Arguments | Effect |
| --- | --- | --- |
| `priv <reg_or_offset> <u64>` | Privileged display register name (`PMODE`, `SMODE1`, `SMODE2`, `SRFSH`, `SYNCH1`, `SYNCH2`, `SYNCV`, `DISPFB1`, `DISPLAY1`, `DISPFB2`, `DISPLAY2`, `EXTBUF`, `EXTDATA`, `EXTWRITE`, `BGCOLOR`) or byte offset (`0x0000..=0x00E0`), 64-bit value | Writes the GS privileged display register (`gs.write_privileged_display`). |
| `reg <reg_or_addr> <u64>` | Internal GS register name (`PRIM`, `FRAME_1`, `BITBLTBUF`, `TRXPOS`, `TRXREG`, `TRXDIR`, `HWREG`, etc.) or hex index (`0x00..=0x7F`), 64-bit value | Writes internal GS register directly (useful for initial test harness state before GIF packets). |
| `qwc <lo_u64> <hi_u64>` | Two 64-bit little-endian halves forming one 128-bit `Reg128` quadword | Feeds raw quadword (`GIFtag`, `PACKED`, `REGLIST`, or `IMAGE` payload) through `GifStreamDecoder`. |
| `qwc32 <w0> <w1> <w2> <w3>` | Four 32-bit little-endian words forming one 128-bit `Reg128` quadword | Same as `qwc`, split into 32-bit lanes (`lo = w0 \| (w1 << 32)`, `hi = w2 \| (w3 << 32)`). |
| `surface <bp_blocks> <bw> <psm> <width> <height>` | Base pointer (`* 256` bytes), buffer width (`* 64` px), pixel format name (`PSMCT32`, `PSMT8`, etc.) or numeric PSM, width, height | Optional metadata declaring the intended output surface for test harnesses / `ps2-gs-runner`. |

## `ps2-gs-runner` CLI

`ps2-gs-runner` (`crates/ps2-gs-runner`) replays one or more `<INPUTS>...` `.gsp` scripts in three modes: PNG dump (default), standalone PS2 ELF32 emission (`--emit-elf`), or reference PNG comparison (`--compare-png`). `--emit-elf` and `--compare-png` are mutually exclusive and each requires a single input script.

| Flag | Value / Default | Effect |
| --- | --- | --- |
| `--rect <FBP,FBW,PSM,W,H>` | optional | Overrides the `.gsp` `surface` directive (or inferred `FRAME_1`/`SCISSOR_1` rect). `FBP` is in pages (`bp_blocks = FBP * 32`), `FBW` in 64-px units (`FBW`, `W`, `H` non-zero), `PSM` is a format name (`PSMCT32`, `PSMCT24`, `PSMCT16`, `PSMCT16S`, `PSMT8`, `PSMT4`, `PSMZ32`, `PSMZ24`, `PSMZ16`, `PSMZ16S`) or numeric code. |
| `-o, --out <OUT>` | optional | Explicit output file path (PNG in render mode, ELF in `--emit-elf` mode). Cannot be combined with multiple inputs. |
| `--out-dir <OUT_DIR>` | `local/gs_out` | Output directory when `--out` is omitted (`<stem>.png` or `<stem>.elf`). |
| `--rgba` | bool (`false`) | Export 4-channel RGBA PNG instead of opaque RGB in render mode. |
| `--scale <N>` | `1` (`N >= 1`) | Integer nearest-neighbor upscale factor applied to exported PNG dimensions in render mode. |
| `--emit-elf` | bool (`false`) | Emit a standalone R5900 ELF32 (`.elf`) containing PCRTC display setup and the script's GIF quadwords instead of dumping a PNG. Single input only. |
| `--disasm-stub` | bool (`false`) | Print R5900 disassembly of the generated ELF stub (used with `--emit-elf`). |
| `--compare-png <REF_PNG>` | optional | Replay one `.gsp` script and compare its extracted RGB image against an external reference PNG (e.g., a PCSX2 screenshot). Exits non-zero if `max(|dR|,|dG|,|dB|) > --tolerance`. Single input only. |
| `--diff-out <DIFF_PNG>` | optional | Optional diff PNG path in `--compare-png` mode (0.25x darkened reference, yellow `#FFE040` for `±1` deltas, bright magenta `#FF2080` for `>1` deltas). |
| `--tolerance <N>` | `0` (`u8`) | Maximum allowed absolute per-channel difference `max(|dR|,|dG|,|dB|)` in `--compare-png` mode. |
| `--max-mismatches <N>` | `16` | Maximum number of mismatched coordinates printed in `--compare-png` summary output. |
