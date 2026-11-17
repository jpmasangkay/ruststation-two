# Step 2.5 PCSX2 diagnostic probe (outside frozen GS_RENDER_GATE_SUITES until Step 4).
# Slanted unclipped Gouraud triangle with all X and Y coordinates distinct and distinct R/G/B
# on all three vertices, exercising non-integer dC/dx and dC/dy across all three channels
# and cycling min_x % 4 along the unclipped left edge (v0->v2 slope dx/dy = 42/204).
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

# 2. Slanted Gouraud triangle (PRIM = 0xB):
# v0=( 47,  19) RGB=(247,  23, 199)
# v1=(283, 137) RGB=( 19, 241,  41)
# v2=( 89, 223) RGB=(131,  97, 251)
# area2 = 43188; non-integer dC/dx and dC/dy in R, G, and B.
reg PRIM 0x000B
reg RGBAQ 0x3F800000_80C717F7
reg XYZ2 0x00000000_813082F0
reg RGBAQ 0x3F800000_8029F113
reg XYZ2 0x00000000_889091B0
reg RGBAQ 0x3F800000_80FB6183
reg XYZ2 0x00000000_8DF08590
