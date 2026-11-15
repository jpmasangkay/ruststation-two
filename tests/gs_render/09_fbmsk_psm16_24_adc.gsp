# Step 2 GS Render Gate #9: FBMSK, PSMCT24 upper-byte preservation, PSMCT16 RGBA5551, Point, LineStrip & TriangleStrip + ADC.
surface 0 1 PSMCT32 64 32

reg PRMODECONT 1
reg XYOFFSET_1 0
reg SCISSOR_1 0x001F_0000_003F_0000
reg ZBUF_1 0x0000_0001_0000_0030
reg TEST_1 0x0001_0000

# 1. Seed 8x8 block at FBP=0 in PSMCT32 with upper 8H alpha byte = 0xAB and BGR = 0x112233
reg FRAME_1 0x0000_0000_0001_0000
reg TEST_1 0x0003_0000
reg PRIM 6
reg XYZ2 0x0000_0000
reg RGBAQ 0x3F800000_AB112233
reg XYZ2 0x0080_0080

# Switch to PSMCT24 (0x01) with FBMSK masking G (0x0000_FF00), draw sprite (0,0)-(4,4) with R=0x77, G=0x88, B=0x99, A=0xFF:
# Expected VRAM 32-bit word at (1,1): 0xAB preserved, B=0x99, G=0x22 preserved, R=0x77 -> 0xAB992277
reg FRAME_1 0x0000_FF00_0101_0000
reg XYZ2 0x0000_0000
reg RGBAQ 0x3F800000_FF998877
reg XYZ2 0x0040_0040

# 2. Switch to PSMCT16 (0x02) at page FBP=2 (0x0002), FBMSK=0, draw sprite (0,0)-(8,8) with R=0xFF(31), G=0, B=0, A=0x80(1) -> 0x801F
reg FRAME_1 0x0000_0000_0201_0002
reg XYZ2 0x0000_0000
reg RGBAQ 0x3F800000_800000FF
reg XYZ2 0x0080_0080

# Now test bit 63 of FRAME (`FBMSK = 0x8000_00FF`): masks R and alpha bit 15, draws G=0xFF(31), A=0 -> preserves R5=31 & A1=1 -> 0x83FF
reg FRAME_1 0x8000_00FF_0201_0002
reg XYZ2 0x0000_0000
reg RGBAQ 0x3F800000_0000FF00
reg XYZ2 0x0040_0040

# 3. Back to PSMCT32 page 0: Point (12, 4) -> 0x80123456
reg FRAME_1 0x0000_0000_0001_0000
reg PRIM 0
reg RGBAQ 0x3F800000_80123456
reg XYZ2 0x0044_00C2

# 4. TriangleStrip (PRIM=4) at x in [20..36], y in [4..16] with 4th vertex masked by XYZ3 (ADC) and 5th vertex drawn!
reg PRIM 4
reg RGBAQ 0x3F800000_80FFFFFF
reg XYZ2 0x0040_0140
reg XYZ2 0x0040_01C0
reg RGBAQ 0x3F800000_8000FF00
reg XYZ2 0x00C0_0140
# v3 (28, 12) uses XYZ3 (ADC -> skips tri (28,4)-(20,12)-(28,12), keeps [v1=(28,4), v2=(20,12)]? No, keeps [v2=(20,12), v3=(28,12)])!
reg XYZ3 0x00C0_01C0
# v4 (28, 20) uses XYZ2 -> draws (v2=(20,12), v3=(28,12), v4=(28,20)) in blue!
reg RGBAQ 0x3F800000_80FF4020
reg XYZ2 0x0140_01C0
