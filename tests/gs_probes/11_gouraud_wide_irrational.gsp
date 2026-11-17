# Step 2.5 PCSX2 diagnostic probe (outside frozen GS_RENDER_GATE_SUITES until Step 4).
# Wide (W=307) vs narrow (W=19) unclipped Gouraud triangles with prime-denominator gradients
# to separate horizontal span stepping error (Q7/Q9/SIMD drift) from edge/start rounding.
surface 0 5 PSMCT32 320 240

reg PRMODECONT 1
reg FRAME_1 0x00000000_00050000
reg ZBUF_1 0x00000001_00000096
reg XYOFFSET_1 0x00008000_00008000
reg SCISSOR_1 0x00EF0000_013F0000
reg TEST_1 0x00030000

# 1. Clear full 320x240 surface to black with A=0x80
reg PRIM 6
reg RGBAQ 0x3F800000_80000000
reg XYZ2 0x00000000_80008000
reg XYZ2 0x00000000_8F009400

# 2. Top wide Gouraud triangle (PRIM = 0xB = Triangle + IIP):
# v0=(160, 10)  RGB=(255,  16, 128)
# v1=(313, 110) RGB=( 17, 251, 128)
# v2=(  6, 110) RGB=(254,  15, 128)
# W=307 (prime): dR/dx = -237/307, dG/dx = +236/307, dB/dx = 0
reg PRIM 0x000B
reg RGBAQ 0x3F800000_808010FF
reg XYZ2 0x00000000_80A08A00
reg RGBAQ 0x3F800000_8080FB11
reg XYZ2 0x00000000_86E09390
reg RGBAQ 0x3F800000_80800FFE
reg XYZ2 0x00000000_86E08060

# 3. Bottom narrow Gouraud triangle (PRIM = 0xB):
# v0=(160, 130) RGB=(240,  20,  96)
# v1=(169, 230) RGB=( 15, 247, 200)
# v2=(150, 230) RGB=(238,  22,  95)
# W=19 (prime): dR/dx = -223/19, dG/dx = +225/19, dB/dx = +105/19
reg RGBAQ 0x3F800000_806014F0
reg XYZ2 0x00000000_88208A00
reg RGBAQ 0x3F800000_80C8F70F
reg XYZ2 0x00000000_8E608A90
reg RGBAQ 0x3F800000_805F16EE
reg XYZ2 0x00000000_8E608960
