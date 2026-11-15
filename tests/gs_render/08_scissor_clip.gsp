# Step 2 GS Render Gate #8: Scissor clipping `[40..=279] x [30..=209]` with `XYOFFSET_1 = (2048, 2048)`.
surface 0 5 PSMCT32 320 240

reg PRMODECONT 1
reg FRAME_1 0x0000_0000_0005_0000
reg ZBUF_1 0x0000_0000_0000_0030
reg TEST_1 0x0003_0000
reg XYOFFSET_1 0x0000_0000_0000_0000
reg SCISSOR_1 0x00EF_0000_013F_0000

# 1. Clear full 320x240 surface to dark plum #1A1622
reg PRIM 6
reg XYZ2 0x0000_0000_0000_0000
reg RGBAQ 0x3F800000_8022161A
reg XYZ2 0x0F00_1400

# 2. Draw crisp 1px outer guide frame around the scissor box at x=39..=280, y=29..=210 using LineStrip (PRIM=2)
reg PRIM 2
reg RGBAQ 0x3F800000_80C0C0C0
reg XYZ2 0x01D0_0270
reg XYZ2 0x01D0_1180
reg XYZ2 0x0D20_1180
reg XYZ2 0x0D20_0270
reg XYZ2 0x01D0_0270

# 3. Engage tight SCISSOR_1 = [40..=279] (0x28..=0x117) x [30..=209] (0x1E..=0xD1)
reg SCISSOR_1 0x00D1_001E_0117_0028
# Set XYOFFSET_1 = (2048, 2048) -> 0x8000 subpixels in both X and Y
reg XYOFFSET_1 0x0000_8000_0000_8000

# Oversized coral banner sprite at screen [-20, 10]..[340, 100] (in offset coords [2028..2388] x [2058..2148])
# Coordinates: x0 = 2028*16=0x7EC0, y0 = 2058*16=0x80A0, x1 = 2388*16=0x9540, y1 = 2148*16=0x8640
reg PRIM 6
reg XYZ2 0x80A0_7EC0
reg RGBAQ 0x3F800000_805868F0
reg XYZ2 0x8640_9540

# Oversized Gouraud triangle crossing ALL FOUR scissor edges:
# v0 = screen(160, 0) -> offset(2208, 2048) = (0x8A00, 0x8000) bright gold
# v1 = screen(318, 238) -> offset(2366, 2286) = (0x93E0, 0x8EE0) cyan
# v2 = screen(2, 238) -> offset(2050, 2286) = (0x8020, 0x8EE0) violet
reg PRIM 0xB
reg RGBAQ 0x3F800000_8030E0FF
reg XYZ2 0x8000_8A00
reg RGBAQ 0x3F800000_80FFE040
reg XYZ2 0x8EE0_93E0
reg RGBAQ 0x3F800000_80FF40C0
reg XYZ2 0x8EE0_8020
