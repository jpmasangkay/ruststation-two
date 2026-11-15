# `.gsp` GS Packet Stream Format

`.gsp` is a plain ASCII packet-stream test script consumed by `ps2_gs::packet::{parse_gsp_script, execute_gsp_script}` (and later `ps2-gs-runner` in Phase 2b Step 4).

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
