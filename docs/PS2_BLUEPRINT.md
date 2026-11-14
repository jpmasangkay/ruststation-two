# **Engineering Blueprint for a High-Performance, Cross-Platform PlayStation 2 Emulator in Rust** 

## **1. Executive Summary and Feasibility Assessment** 

### **Brutally Realistic Feasibility Verdict for a Solo Developer** 

The Sony PlayStation 2 (PS2) represents one of the most complex and idiosyncratic hardware architectures in video game history. Unlike fifth-generation consoles or handhelds such as the Game Boy Advance (which relies on a single ARM7TDMI CPU and a fixed 2D tile/bitmap PPU) or the original PlayStation (a single MIPS R3000A CPU, fixed-point GTE coprocessor, and 1 MB VRAM GPU), the PS2 is a heterogeneous, loosely coupled multiprocessor workstation. It comprises ten distinct programmable or semi-autonomous processing and bus units operating across four clock domains: the 294.912 MHz Emotion Engine (EE) with 128-bit multimedia extensions, a non-IEEE 754 floating-point coprocessor (COP1), two VLIW Vector Units (VU0 and VU1) fed by custom packet decompressors (VIF0 and VIF1), a 10-channel 128-bit DMA controller (DMAC) capable of autonomous source and destination chain execution, an MPEG-2 hardware decoder (IPU), a 3-path Graphics Interface (GIF), the fixed-function Graphics Synthesizer (GS) backed by 4 MB of ultra-wide-bus embedded DRAM (eDRAM), and the entire original PlayStation CPU (the 36.864 MHz R3000A IOP) paired with a 48-voice dual-core audio processor (SPU2). 

Reaching PCSX2's current 99.5% commercial compatibility milestone required over two decades of continuous engineering by dozens of specialized contributors. For a solo developer building a PS2 emulator from scratch with the twin mandates of running the heaviest commercial games at a locked 50/60 FPS and providing a modern, cross-platform user interface, the project is feasible only if scoped with surgical discipline. A solo engineer can realistically achieve BIOS boot and pass user-mode CPU and DMA test suites (ps2autotests) within 3 to 4 months, render basic 3D homebrew and early commercial titles via a reference software or basic hardware rasterizer within 9 to 12 months, and bring a dynamic recompiler (JIT) plus a modern GPU hardware renderer online for major commercial games within 18 to 24 months. However, achieving artifact-free, full-speed execution across the 25 heaviest PS2 titles—which rely on mid-frame framebuffer-to-texture channel shuffles, depth-buffer readbacks, self-modifying VU1 microcode, and sub-scanline DMA timing—requires 30 to 42 months of sustained engineering effort. 

To survive this complexity without stalling in multi-year rabbit holes, a solo developer must ruthlessly cut or defer four non-essential subsystems from the initial roadmap. First, the developer must defer a SIMD-optimized multithreaded CPU software rasterizer, relying instead on a clean, single-threaded reference software rasterizer strictly for validation while investing 90% of graphics engineering into a modern GPU hardware renderer (wgpu / Vulkan) and a standalone GS dump runner. Second, the project must cut PlayStation 1 backward compatibility (DECKARD / PS1 mode); while the IOP is a PS1 R3000A CPU and retains the PS1 GTE in silicon, supporting PS1 discs requires emulating legacy PS1 GPU and SPU1 mappings that add zero value to PS2 titles. Third, the architecture must ingest 

PCSX2's open-source GameIndex.yaml database early rather than attempting to solve every broken game engine math bug via cycle-exact pipeline emulation, applying known rounding, clamping, and GS target fixes automatically. Fourth, the developer must cut official Apple App Store (JIT-less) iOS and WebAssembly browser targets, focusing exclusively on platforms that permit native W^X or dual-mapped dynamic code generation and 64-bit virtual memory page-fault handling, namely Windows, Linux, macOS, Android, and sideloaded iOS. 

### **Language Evaluation: Rust vs. C++** 

Selecting Rust as the primary implementation language for a clean-sheet PS2 emulator is technically sound and offers compelling architectural advantages, provided the core engineer avoids fighting the borrow checker inside the dynamic recompiler and virtual memory subsystem. 

|**Dimension**|**Rust (Recommended Default)**|**C++20 /** **C++23** **(Reference** **Standard)**|**Architectural** **Verdict**|
|---|---|---|---|
|**JIT Code** **Generation**|dynasm-rs(proc-macro assembler for x86-64 and AArch64). Lacks a direct equivalent to Xbyak's runtime C++ class hierarchy without custom wrappers.|Xbyak (x86-64) and vixl/oaknut (ARM64) used by PCSX2, Play!, and ARMSX2. Battle-tested for 15+ years.|**Tie**: dynasm-rs emits zero-overhead machine code at compile-time macro expansion, matching C++ emitters in speed.|
|**Multithreading** **(MTVU /** **MTGS)**|Compile-timeSend/Sync guarantees prevent data races across the EE, VU1 (MTVU), and GS (MTGS) worker threads.crossbeam/ custom lock-free SPSC queues.|Manual std::atomic ring buffers. Historical source of subtle race conditions and deadlocks in older PCSX2 plugin eras.|**Rust** **Advantage**: Eliminates memory corruption across asynchronous VIF1/GIF/GS command queues.|



|**Virtual** **Memory &** **fastmem**|RequiresunsafeFFI to mmap/mprotect/sigaction (POSIX) and VirtualAlloc2/AddVectoredExce ptionHandler(Win32).|Same OS APIs accessed directly without unsafe ceremony.|**C++ Slight** **Edge**: Rust requires strict adherence to unsafe aliasing rules (UnsafeCell/ raw pointers) for guest RAM.|
|---|---|---|---|
|**SIMD &** **Floating-Point** **Quirks**|core::arch::x86_64and core::arch::aarch64expose full SSE4.1/AVX2 and NEON intrinsics. Bit-exact soft-float (ps2float) ports cleanly.|Native intrinsics plus mature C89/C++ reference libraries (ps2float.cin parallel-ps2).|**Tie**: Identical generated assembly under LLVM (rustcand clangshare the LLVM backend).|
|**Graphics & UI** **Ecosystem**|First-classwgpuandash (Vulkan) crates; modern immediate/declarative UIs (egui,Slint,iced) andimgui-rs.|Native Vulkan/D3D12 /Metal APIs and native Qt 6 Widgets (used by PCSX2 v2.x and DobieStation).|**C++ Edge for** **Qt, Rust** **Edge for** **Portability**: Qt 6 in C++ is the gold standard for desktop emulator UIs, while Rust excels at unified desktop/mobil e rendering.|



The optimal engineering path is to build the emulator in **Rust** while enforcing a strict two-zone safety boundary. The outer 85% of the codebase—encompassing the user interface, configuration hierarchy, disc and CHD parsing, save states, shader caches, debugger, input handling, audio mixing, and high-level hardware state machines—should use idiomatic, safe Rust. Conversely, the inner 15% hot path—comprising the JIT code cache, the fastmem virtual memory arena, the CPU context structure accessed directly by JIT assembly, and the SIMD VIF/GS swizzlers—must use encapsulated unsafe blocks 

backed by raw pointers (*mut u8) and #[repr(C, align(64))] structures to prevent rustc from inserting bounds checks or aliasing assertions in hot loops. 

### **The 7 Most Important Architectural Decisions** 

The architectural foundation of the emulator rests on seven core decisions that govern both compatibility and heavy-game throughput. First, the core must utilize an **event-driven downcounting scheduler with cycle batching** rather than ticking the EE (294.912 MHz), Bus/DMAC (147.456 MHz), and IOP (36.864 MHz) in single-cycle lockstep. Executing EE JIT blocks in batches of 64 to 256 cycles up to the nearest scheduled hardware event (next_event_cycle) and synchronizing the IOP lazily or in coarse timeslices—except when SIF DMA or shared communication registers are accessed—eliminates billions of redundant context switches per second. Second, the emulator must employ a **custom single-pass/two-pass JIT via dynasm-rs** , explicitly rejecting heavy compiler backends like Cranelift and LLVM. Heavy PS2 games frequently upload new VU1 microprograms and trigger self-modifying code (SMC) on the EE. Empirical results from Rust emulators such as pchan and RustyN64 confirm that Cranelift's compilation overhead causes severe frame-time stutter, whereas a custom emitter built on dynasm-rs compiles basic blocks in microseconds. 

Third, memory translation must rely on a **SIGSEGV-backpatched fastmem virtual memory arena** . By allocating a contiguous 4 GB (or 8 GB) uncommitted virtual address space, mapping physical RAM (32 MB), Scratchpad (16 KB), and BIOS (4 MB) directly into their guest virtual offsets, and intercepting MMIO accesses (0x10000000+), GS privileged writes (0x12000000+), and SMC page writes (mprotect) via an OS page-fault handler (SIGSEGV/SIGBUS on POSIX; VectoredExceptionHandler on Windows) that backpatches the faulting JIT instruction in-place, the emulator achieves single-instruction memory loads and stores. Fourth, the engine must implement a **decoupled 3-thread execution pipeline (EE/IOP + MTVU + MTGS)** mirroring PCSX2's architecture. Thread 1 runs the EE, VU0, DMAC, and IOP; Thread 2 (MTVU) executes VU1 microcode asynchronously via a lock-free ring buffer; and Thread 3 (MTGS) consumes GIF packets (PATH1/PATH2/PATH3), maintains the 4 MB local VRAM mirror and texture cache, and submits GPU command buffers. 

Fifth, floating-point emulation must provide **dual-level float accuracy** , pairing a hardware fast-clamp default with a bit-exact ps2float software fallback. By default, the EE FPU (COP1) and VU0/VU1 JITs emit native host SIMD instructions flanked by branchless bitwise exponent clamping that converts IEEE NaN/±Inf (0x7F800000) into the PS2 maximum float (0x7F7FFFFF), while a bit-exact software float library (ps2float algorithm) remains available per-instruction or per-game when GameIndex.yaml flags a title as requiring exact PS2 truncation or FMA flags. Sixth, the **GS hardware renderer must center on an 8 KB page-granular texture cache and channel-shuffle pre-detector** (512 pages covering 4 MB eDRAM) from day one, utilizing hardware **Dual-Source Blending** for single-pass PS2 alpha blending (((A - B) * C >> 7) + D) and converting 8-bit/16-bit cross-format framebuffer draws into shader swizzles rather than GPU-stalling texture copies. Seventh, the developer should construct a **headless GS dump (.gs) runner before attempting commercial game boot** . A standalone CLI tool (ps2-gs-runner) capable of replaying PCSX2 .gs dumps allows the engineer to test, profile, and debug _Shadow of the Colossus_ , _God of War II_ , and _Metal Gear Solid 3_ rendering bugs in isolation using RenderDoc long before the IOP or CDVD subsystems are even written. 

### **The 5 Biggest Risks to Heavy-Game Performance** 

Five technical hazards pose the greatest threat to maintaining 50/60 FPS in demanding PS2 titles. The primary GPU risk is **GS texture cache thrashing and mid-frame CPU readbacks** : games such as _Gran Turismo 4_ (sun lens flare occlusion checks), _Champions of Norrath_ (dynamic 8-bit palletized lightmaps), and _Valkyrie Profile 2_ (depth readbacks) read rendered framebuffers back to EE RAM or reinterpret 32-bit render targets as 8-bit textures dozens of times per frame. Naive full-target invalidation or synchronous GPU-to-CPU stalls will drop frame rates below 15 FPS. The second major risk is **VU1 microcode recompilation stutter and flag pipeline overhead** . Heavy titles ( _God of War_ , _Shadow of the Colossus_ , _Ratchet & Clank_ ) dynamically stitch and upload hundreds of VU1 microprograms; emulating the 4-stage MAC, Status, and Clipping flag pipelines on every vector instruction without dead-flag elimination bloats JIT code by 3 × to 4 × and starves the host instruction cache. 

A third, platform-specific hazard is the **16 KB host page size mismatch on Apple Silicon and modern Android kernels** . Because the PS2's virtual memory TLB and DMA pages operate on 4 KB boundaries (and 8 KB on the GS) while Apple Silicon (macOS/iOS) and Android 15+ kernels enforce 16 KB virtual memory pages, relying solely on OS page protection (mprotect) for 4 KB self-modifying code or vtlb fastmem triggers false-positive page faults across adjacent 4 KB blocks unless paired with sub-page dirty bitmasks and instruction backpatching. Fourth, **recursive blending barriers (DATE / COLCLIP) on tile-based mobile GPUs** can cripple performance in titles like _Zone of the Enders 2_ , _Metal Gear Solid 3_ , and _Burnout 3_ , which stack hundreds of alpha-blended particles and stencil-like Destination Alpha Tests (DATE). Implementing these via full render-pass breaks and texture copies instead of VK_EXT_rasterization_order_attachment_access or Framebuffer Fetch incurs severe tile-flush bandwidth penalties on Adreno and Mali GPUs. Finally, **EE-VU-GS synchronization stalls can defeat MTVU** : while MTVU runs VU1 on a second host core, games that poll COP2 status registers (CFC2), read VU1 memory directly from the EE, or interleave PATH1 (XGKICK) and PATH3 (GIF DMA) transfers without proper PATH3 masking force the EE thread to block until the MTVU queue drains, turning multithreading into lock-contention overhead. 

## **2. PS2 Hardware Reference and Emulation Mechanics** 

### **Emotion Engine (MIPS R5900): ISA, MMI, COP0, and COP1 Quirks** 

The Emotion Engine (EE) core is a custom 64-bit superscalar MIPS R5900 processor clocked at **294.912 MHz** . While rooted in the MIPS III and MIPS IV instruction sets, Sony and Toshiba extended all 32 General Purpose Registers (GPR[0..31]) to **128 bits wide** to support packed SIMD integer operations. Standard 32-bit and 64-bit MIPS arithmetic instructions (ADD, DADDU, LW, LD) operate exclusively on the lower 64 bits (bits 0..63) while preserving the upper 64 bits (bits 64..127). The upper 64 bits are modified only during 128-bit Quadword Load/Store instructions (LQ and SQ, which silently mask the lowest 4 bits of the effective address to enforce 16-byte alignment) and 128-bit Multimedia Instructions (MMI). Furthermore, unlike standard MIPS processors where MULT and DIV write solely to LO and HI, the R5900 introduces a secondary multiplication/division pipeline (LO1/HI1, stored in the upper 64 bits of the 128-bit LO/HI registers) and permits 3-operand MULT rd, rs, rt instructions to write the low 32-bit result directly into a GPR in a single clock cycle. 

The **MMI instruction set** (opcode 0x1C) treats the 128-bit GPRs as vectors of sixteen 8-bit (PADDB), eight 16-bit (PADDH, PMADDH), or four 32-bit (PADDW) integers, complemented by byte, halfword, and word pack, unpack, funnel-shift (SA register), and transpose instructions (PEXTLB, PPACH, PINTH). In a Rust dynamic recompiler, the 128-bit GPRs map cleanly onto host 128-bit SIMD registers (XMM0..15 on x86-64; V0..31 on ARM64 NEON), whereas scalar 64-bit GPR operations map to 64-bit general-purpose host registers (RAX..R15 / X0..X28). Tracking dirty register lanes in the JIT's register allocator ensures that the upper 64 bits are only spilled or merged when an MMI or LQ/SQ instruction is encountered. 

System control is governed by **COP0** , which manages exception vectors, dual performance counters (PERF), and a **48-entry Memory Management Unit (TLB)** . Each TLB entry maps an even/odd pair of virtual pages ranging in size from 4 KB to 16 MB across the EE's virtual address segments. The user/kernel virtual segment KUSEG (0x00000000–0x7FFFFFFF) is TLB-mapped and supports both cached and uncached views. Kernel segment KSEG0 (0x80000000–0x9FFFFFFF) provides unmapped cached access where the physical address equals the virtual address masked with 0x1FFFFFFF, while KSEG1 (0xA0000000–0xBFFFFFFF) provides unmapped uncached access used for early BIOS boot (0xBFC00000) and MMIO. Additionally, the EE features **16 KB of zero-wait-state Scratchpad SRAM (0x70000000–0x70003FFF)** mapped via a dedicated S (Scratchpad) bit in TLB entry 0, alongside "Uncached Accelerated" (UCAB) burst-write mappings of main RDRAM at 0x30100000+. 

The EE's Floating-Point Unit ( **COP1** ) and the Vector Units implement a proprietary **non-IEEE 754 32-bit floating-point format** that serves as one of the most frequent sources of game logic bugs in PS2 emulators. Unlike IEEE 754, where a biased exponent of 255 (0xFF) denotes Infinity or NaN, the PS2 treats exponent 0xFF as valid finite normalized numbers extending up to ± (2 −2−23) × 2128 ( ≈± 6. 8056469 × 1038 ). When an arithmetic operation overflows exponent 0xFF, the hardware saturates (clamps) the magnitude to 0x7F7FFFFF (positive) or 0xFF7FFFFF (negative) and sets the Overflow (O) flag. Denormal numbers (exponent 0x00) are unconditionally flushed to ± 0. 0 regardless of mantissa bits. Moreover, the PS2 FPU rounds exclusively toward zero (truncation) and lacks IEEE 754's three extra precision bits (guard, round, sticky) on subtraction and multiplication, causing PS2 MUL.S and SUB.S results to diverge by 1 ULP from host x86/ARM hardware even when host rounding is set to FE_TOWARDZERO. Instructions such as MIN.S and MAX.S evaluate signed zeros (+0.0 vs. -0.0) and exponent 0xFF values as raw signed-magnitude integers without trapping on NaN, while the 32-bit ACC accumulator register in fused multiply-add (MADD.S/MSUB.S) applies intermediate overflow and underflow rules between the multiply and addition stages. 

### **VU0 and VU1: VLIW Microcode, VIF0/VIF1, and Synchronization** 

The two Vector Units (VU0 and VU1) are 294.912 MHz dual-issue VLIW processors equipped with thirty-two 128-bit vector float registers (VF00..VF31, where VF00 is hardwired to (𝑥= 0. 0, 𝑦= 0. 0, 𝑧= 0. 0, 𝑤= 1. 0) ) and sixteen 16-bit integer and control registers (VI00..VI15, where VI00 is hardwired to 0). Every 64-bit VLIW instruction bundle executes two operations simultaneously in a single clock cycle. The **Upper Instruction (bits 32..63)** executes 4-lane SIMD floating-point arithmetic (ADD, MUL, MADD, MSUB, MAX, MINI, 

CLIP, OPMULA, OPMSUB) with customizable .xyzw destination write masks and broadcast lane modifiers (.x, .y, .z, .w). Concurrently, the **Lower Instruction (bits 0..31)** drives the Elementary Function Unit (EFU, available on VU1 only for ESIN, EATAN, EEXP), the FDIV unit (DIV, SQRT, RSQRT, which write to the scalar Q register after 7 or 13 cycles), the 16-bit integer ALU (IADD, ISUB, IAND), conditional branches (IBEQ, IBNE, BAL with a 1-instruction delay slot), vector memory loads/stores (LQ, SQ, ILW, ISW), random number generation (RGET/RNEXT), or the XGKICK graphics transfer instruction (VU1 only). 

While **VU0 (4 KB Micro Memory / 4 KB Data Memory)** operates primarily in **Macro Mode** —executing inline COP2 instructions directly within the EE instruction stream and sharing registers with the EE via QMFC2/QMTC2—it can also run standalone VLIW loops in **Micro Mode** when triggered by VIF0 MSCAL or EE VMSCALL. Conversely, **VU1 (16 KB Micro Memory / 16 KB Data Memory)** operates exclusively in **Micro Mode** . Games stream compressed vertex and matrix packets through **VIF1** , which unpacks data (UNPACK supporting S-32, V2-16, V3-32, V4-8, etc., with USN sign extension and FLG row/column addition) into VU1 Data Memory using double-buffered address registers (BASE, OFFSET, TOP, TOPS). VIF1 then fires MSCAL or MSCNT to transform, light, clip, and perspective-divide vertices on VU1 before XGKICK streams the resulting GS primitive packet directly from VU1 Data Memory over **GIF PATH1** . 

A critical hardware subtlety in VU Micro Mode is the **4-stage flag pipeline hazard** . Because the FMAC unit has a 4-cycle latency, modifications made by an Upper instruction to the **MAC flags** (Sign, Zero, Overflow, Underflow per 𝑋, 𝑌, 𝑍, 𝑊 lane), **Status flags** (sticky equivalents), or **Clipping flags** (6-bit frustum test results shifted into a 24-bit register by CLIPw.xyz) do not become visible to Lower instructions (FSAND, FMAND, FCAND, used heavily for frustum culling branches) until **4 cycles later** . In a VU dynamic recompiler (microVU), the compiler must perform a static analysis pass over each basic block to model hardware register stalls—where reading a VF register at cycle 𝑡+ 1 after it was written at cycle  stalls the 𝑡 execution pipeline for 3 cycles while simultaneously advancing the flag pipeline—and maintain a 4-element flag history ring buffer. 

### **GIF, GS (4 MB eDRAM), Pixel Formats, and Privileged Registers** 

The **Graphics Interface (GIF)** arbitrates data into the **Graphics Synthesizer (GS)** across three prioritized hardware paths: **PATH1** (the highest-priority direct 128-bit bus from VU1 triggered by XGKICK), **PATH2** (the medium-priority direct path from VIF1 DIRECT and DIRECTHL commands), and **PATH3** (the lowest-priority DMA Channel 2 path streaming directly from EE RAM or Scratchpad). Because VU1 (PATH1) and EE DMA (PATH3) run concurrently during heavy gameplay, games set the PATH3 Mask via VIF1 (MSKPATH3) or GIF_MODE so that PATH3 texture uploads wait in the FIFO until the active PATH1 geometry draw finishes (EOP End-of-Packet bit in the 128-bit GIFTag). Similarly, the DIRECTHL command stalls PATH2 only when an uncompressed IMAGE mode transfer is actively mid-flight on PATH3. Failing to emulate PATH3 masking and packet interleaving causes severe texture corruption across multi-pass rendering engines. 

The **GS** is a fixed-function rasterizer operating at 147.456 MHz with **4 MB of internal eDRAM** (0x000000–0x3FFFFF, addressed in 32-bit words from 0x00000 to 0xFFFFF). The GS has no native distinction between textures, color framebuffers, or depth buffers; all 4 MB of eDRAM forms a single unified memory space swizzled into **Pages** (8 KB = 2,048 32-bit 

words), **Blocks** (256 bytes = 64 words; 32 blocks per page), and **Columns** (64 bytes = 16 words; 4 columns per block). 

|**Pixel** **Storage** **Format** **(PSM)**|**Bits /** **Pixel**|**Page** **Dimensions** **(W×HW \times** **HW×H Pixels)**|**Block** **Dimensions** **(W×HW \times** **HW×H Pixels)**|**Primary Hardware** **& Game Usage**|
|---|---|---|---|---|
|**PSMCT32 /** **PSMZ32**|32-bit|64 × 32|8 × 8|Standard RGBA8888 framebuffers, 32-bit Z-buffers, and RGBA textures.|
|**PSMCT24 /** **PSMZ24**|24-bit|64 × 32|8 × 8|RGB888 / 24-bit Z packed into bits 0..23of each 32-bit word, leaving bits 24..31untouched for PSMT8H/ PSMT4HL/ PSMT4HHtextures.|
|**PSMCT16 /** **PSMCT16S**|16-bit|64 × 64|16 × 8|RGBA5551 framebuffers and targets; used heavily in channel-shuffle passes.|
|**PSMT8 /** **PSMT8H**|8-bit|128 × 64 (PSMT8) / (8H) 64 × 32|/ 16 × 16 8 × 8|256-color CLUT-indexed textures;PSMT8H stores indices in the top byte of PSMCT24/PSMZ24 pages to save VRAM.|



|**PSMT4 /4HL** **/4HH**|4-bit|128 × 128 (PSMT4) / (4H) 64 × 32|/ 32 × 16 8 × 8|16-color CLUT-indexed textures;4HL/4HH pack two 4-bit textures into the|
|---|---|---|---|---|
|||||upper byte of a 24-bit buffer.|



Within the GS rendering pipeline, state is split across **dual drawing contexts (Context 1 and Context 2)** , where registers such as FRAME_1/2, ZBUF_1/2, TEX0_1/2, ALPHA_1/2, TEST_1/2, FBA_1/2, SCISSOR_1/2, and XYOFFSET_1/2 allow games to maintain two independent pipeline configurations (for instance, Context 1 for opaque 3D world geometry and Context 2 for UI sprites or shadow volumes) selected per-primitive by the CTXT bit in PRIM. Vertex coordinates (XYZ2/XYZ3) are specified as unsigned 16-bit fixed-point numbers in **12.4 format** (12 integer bits 0..4095 and 4 fractional sub-pixel bits 0..15/16). Games typically configure XYOFFSET around (2048. 0, 2048. 0) as a guard band against 2D scissor clipping, which hardware renderers must subtract before mapping vertices to normalized device coordinates. 

Alpha blending in the GS (ALPHA_1/2) evaluates source ( 𝐶𝑠, α𝑠 ) and destination ( 𝐶𝑑, α𝑑 ) pixels using the fixed integer formula: 

<u>(𝐴−𝐵)×𝐶</u> 𝐶𝑜𝑢𝑡 = ~~(~~ 128 ~~)~~<sup>+𝐷</sup> where 𝐴, 𝐵, 𝐷∈{𝐶𝑠, 𝐶𝑑, 0} and 𝐶∈{α𝑠, α𝑑, 𝐹𝐼𝑋} . Dividing by 128 (>> 7) rather than 255 means that unity (opaque) alpha in PS2 textures is 0𝑥80 **(** 128 **)** , not 0𝑥𝐹𝐹 , and alpha values of 129.. 255 intentionally over-brighten ( 1. 0 × to 2. 0 × ) to produce additive bloom and specular highlights. When the color clamp register COLCLIP is 0, overflow or underflow wraps modulo 256, requiring specialized HDR COLCLIP blend emulation on modern GPUs. Because the GS lacks a dedicated stencil buffer, games use the 1-bit (PSMCT16) or most-significant bit (PSMCT32 bit 7) of the destination framebuffer's alpha channel as a stencil mask via the **Destination Alpha Test (DATE=1, DATM=0/1 in TEST_1/2)** , while FBA_1/2 forces bit 7 of written alpha values to 1. When a single draw call both tests and modifies destination alpha across overlapping primitives, standard GPU pipelines fail without per-pixel synchronization barriers. Finally, the **GS Privileged Registers (0x12000000)** are mapped directly into the EE address space to drive the PCRTC display controller (PMODE, SMODE2, DISPFB1, DISPLAY1, DISPFB2, DISPLAY2, BGCOLOR, CSR, IMR, BUSDIR), allowing games to point DISPFB1 and DISPFB2 to two half-height buffers or offset them by half a scanline (SMODE2.INT=1, FFMD=0/1) and blend them (PMODE.ALP) for hardware anti-aliasing or 480i/1080i interlaced scanout. 

### **DMAC, IPU, and SIF** 

The **DMAC** operates at 147.456 MHz and transfers 128-bit quadwords (QWC) across 10 hardware channels: 0: VIF0, 1: VIF1, 2: GIF, 3: fromIPU, 4: toIPU, 5: SIF0, 6: SIF1, 7: SIF2, 8: fromSPR, and 9: toSPR. In **Source Chain Mode** , the DMAC reads 128-bit **DMATags** 

directly from main RAM or Scratchpad (CNT, NEXT, REF, REFS, REFE, CALL, RET, END), maintaining a 2-entry hardware return address stack (ASR0, ASR1 in channel CHCR) and optionally transferring the upper 64 bits of the DMATag (TTE=1) to VIF0/VIF1 as an inline command. The DMAC also supports **MFIFO (Memory FIFO)** mode, wherein Channel 8 (fromSPR) or the EE writes display lists into a circular ring buffer in main RAM (D_RBSR, D_RBOR), and VIF1 or GIF drains it automatically while stalling on CHCR.STR when empty (D_STADR). 

The **IPU (Image Processing Unit)** is an MPEG-1 and MPEG-2 video macroblock decoder accessed via 0x10002000 and DMA Channels 3 (fromIPU) and 4 (toIPU). Games stream compressed bitstreams via toIPU, issue decoding and color-space conversion commands (BCLR, IDEC, BDEC, VDEC, FDEC, SETIQ, SETVQ, CSC, PACK), and read back decoded RGBA32, RGB16, or INDX4 macroblocks via fromIPU to upload to the GS as streaming video textures. Meanwhile, the **SIF (Sub-system Interface)** bridges the EE and IOP via shared FIFO buffers (SIF0 for IOP-to-EE, SIF1 for EE-to-IOP, and SIF2 for bidirectional/PS1 compatibility) alongside communication flags (MSCOM, SMCOM, MSFLG, SMFLG at 0x1000F200). Virtually all operating system communication on the PS2—loading .IRX modules, reading controllers, accessing memory cards, streaming files from DVD, and triggering SPU2 audio—travels over **SIF CMD and SIF RPC** packets on SIF0 and SIF1. 

### **IOP (MIPS R3000A), SPU2 Audio, and Modular I/O** 

The **IOP** is the original PlayStation's MIPS R3000A processor clocked at **36.864 MHz** in PS2 mode ( 1/8𝑡ℎ of the EE clock), paired with **2 MB of EDO RAM** (0x00000000–0x001FFFFF), an extended 14-channel IOP DMAC, six timers (three 16-bit PS1 timers plus three 32-bit PS2 timers), and an internal PS1 GTE coprocessor (COP2, confirmed present in PS2 silicon by ps2autotests Issue #30). Rather than hardcoding peripheral drivers into ROM, the IOP runs a modular microkernel (LOADCORE, THBASE, THSEMA, INTRMAN, SIFMAN, SIFCMD) and dynamically loads relocatable ELF modules (.IRX files) from the BIOS ROM (rom0:) or game disc (cdrom0:). 

Peripheral I/O on the IOP spans four primary controllers. First, the **CDVD controller (0x1F402000)** executes N-commands (asynchronous sector reads, seeks, and DVD dual-layer PTP/OTP layer switching) and S-commands (status queries, RTC clock, Mechacon version, and MagicGate decryption). Second, **SIO2 (0x1F808200)** acts as a programmable batch controller that queues up to 16 transfer descriptors (SEND1, SEND2, SEND3) to poll DualShock 2 gamepads (PADMAN), Multitaps, and 8 MB MagicGate Memory Cards (MCMAN, organized in 512-byte pages plus a 16-byte ECC spare area) within a single interrupt. Third, the **SPU2 (0x1F900000)** integrates two independent PS1-style SPU cores ( **Core 0** and **Core 1** ), each providing 24 hardware ADPCM voices (48 voices total) and sharing **2 MB of SPU2 work RAM** (0x000000–0x1FFFFF). Operating at a strict **48 kHz sample rate** ( 768 IOP clock cycles per audio sample), the SPU2 performs hardware ADSR envelope shaping, Gaussian 4-tap pitch interpolation, digital reverb (EEA/EESA), voice modulation, and dual auto-DMA streaming (ADMA on IOP DMA Channels 4 and 7) for background music and software-mixed multi-channel audio. Fourth, **DEV9 (0x1F803800 / 0x10000000 SPD) and USB (0x1F801600 OHCI)** expose the expansion bay interface for the Network Adapter (SMAP Ethernet MAC) and 40 GB IDE Hard Drive (APA partitioning), alongside dual USB 1.1 ports for headsets, keyboards, steering wheels, and lightguns such as the GunCon 2. 

### **Memory Map, Bus Timing, and Disc Media Formats** 

|**Processor**|**Virtual /** **Physical** **Address Range**|**Size**|**Mapped Hardware /** **Memory Region**|**Access Widths** **& Special** **Semantics**|
|---|---|---|---|---|
|**EE**|0x00000000– 0x01FFFFFF|32 MB|Main RDRAM (Cached via KUSEG/KSEG0, Uncached via KSEG1,UCABvia 0x30100000)|8, 16, 32, 64, 128-bit (LQ/SQ).|
|**EE**|0x10000000– 0x1000FFFF|64 KB|EE I/O Registers (Timers0000, IPU 2000, GIF3000, VIF0 3800, VIF13C00, DMAC8000, INTC F000, SIFF200)|32, 64, 128-bit (FIFO writes at 4000–7000).|
|**EE**|0x11000000– 0x1100FFFF|64 KB|VU0 Micro (0000), VU0 Data (4000), VU1 Micro (8000), VU1 Data (C000)|Mapped directly into EE space; writes stall or corrupt if VU is executing.|
|**EE**|0x12000000– 0x12001FFF|8 KB|GS Privileged Registers (PMODE, SMODE2, DISPFB1/2, DISPLAY1/2,CSR, IMR)|64-bit register layout.|
|**EE**|0x1C000000– 0x1C1FFFFF|2 MB|IOP RAM Mirror (Read/Write access from EE, restricted in later BIOS revisions)|8, 16, 32-bit.|



|**EE / IOP**|0x1FC00000– 0x1FFFFFFF|4 MB|System BIOS ROM (rom0:, plusrom1: DVD Player &erom: on later models)|Read-only.|
|---|---|---|---|---|
|**EE**|0x70000000– 0x70003FFF|16 KB|Scratchpad SRAM (Fast internal memory used for DMA chains and stacks)|8, 16, 32, 64, 128-bit (fromSPR/toSPR DMA).|
|**IOP**|0x00000000– 0x001FFFFF|2 MB|IOP Main RAM|8, 16, 32-bit.|
|**IOP**|0x1F402000/ 0x1F80xxxx/ 0x1F90xxxx|64 KB|CDVD (1F402000), IOP Timers/DMA/SIO2/DE V9 (1F80xxxx), SPU2 (1F900000)|8, 16, 32-bit.|



Commercial PS2 software is distributed on CD-ROM (Mode 2 Form 1/2, 2,352 bytes/sector, represented as .bin/.cue) and DVD-5 or DVD-9 Dual-Layer discs (2,048 bytes/sector, .iso, utilizing either Parallel Track Path PTP or Opposite Track Path OTP for Layer 1). Physical copy protection relies on the radial DVD/CD wobble groove—which is authenticated by the Mechacon microcontroller via CDVD S-commands that the emulator trivially 

satisfies—alongside occasional bad-sector LBA checks or Sony DNAS online authentication. A modern PS2 emulator must natively stream uncompressed **ISO** and **BIN/CUE** images alongside compressed **CSO** (maxcso LZ4/Deflate compressed ISOs) and **CHD** (MAME Compressed Hunks of Data v5, pairing LZMA/Zstd data compression with FLAC audio compression for CD tracks) through a unified, random-access sector cache trait. 

## **3. Recommended Emulator Architecture and Workspace Blueprint** 

### **Decoupled Three-Thread Pipeline and Data-Flow Topology** 

To achieve maximum single-thread and multi-thread throughput while keeping platform-specific UI and OS abstractions cleanly isolated, the emulator is architected around a **decoupled three-thread execution pipeline** backed by a shared **4 GB Fastmem Virtual Memory Arena** (32 MB RDRAM, 16 KB Scratchpad, and vtlb SIGSEGV instruction backpatching). Rather than coupling the emulation core to Qt or SDL global state—a historical pain point in PCSX2 that complicated early Libretro LRPS2 and Android porting—the core communicates across internal thread boundaries via cache-line-aligned 

(#[repr(align(64))]) Single-Producer Single-Consumer (SPSC) lock-free ring buffers and talks to thin platform frontends strictly through zero-cost Rust traits. 

|**Execution** **Domain /** **Thread**|**Hosted** **Hardware** **Subsystems**|**Clock / Trigger** **Cadence**|**Outbound Data** **Buses &** **Inter-Thread Sync** **Queues**|
|---|---|---|---|
|**Thread 1:** **Main System** **& Scheduler** (ps2-core)|**EE R5900 JIT** (294.912 MHz), **COP0 / COP1** **FPU**,**VU0** (Macro/Micro), **10-Ch 128-Bit** **DMAC**(147.456 MHz),**IPU** (MPEG-2),**IOP** **R3000A JIT** (36.864 MHz), **SIO2 / CDVD**, and**SPU2 Audio** (48 kHz).|Event-driven downcounting scheduler in batches of 64–256 EE cycles; IOP runs in 32-cycle (256 EE cycle) timeslices.|Pushes VIF1 commands & VU1 microcode/data writes to**Thread 2** **(MTVU Ring** **Buffer)**; pushes PATH2(DIRECT) andPATH3(GIF DMA) packets to **Thread 3 (MTGS** **Queue)**; pushes 48 kHz stereo PCM to **Audio Ring Buffer**.|
|**Thread 2:** **MTVU** **Worker** (ps2-core::mtv u)|**VIF1** **Decompressor** (UNPACK, BASE/OFFSET/ TOP/TOPS) and **microVU1 VLIW** **JIT**(16 KB Micro /16 KB Data).|Drains the 16 MB lock-free MtvuRingBuffer continuously; wakes immediately onMSCAL /MSCNTkick.|StreamsXGKICK geometry packets directly over**GIF** **PATH1**into**Thread** **3 (MTGS Queue)**; updates atomic vu1_read_ptrso Thread 1 blocks (SyncStats) only on explicitCFC2/ VU1 RAM reads.|



|**Thread 3:** **MTGS & GPU** **Renderer** (ps2-gs)|**GIF 3-Path** **Packet Arbiter** (PATH1/2/3+ MSKPATH3),**4** **MB Swizzled** **eDRAM Mirror**, **8 KB Page** **Texture Cache** (512pages), **Channel Shuffle** **Detector**, and **Vulkan / Metal /** **D3D12 /wgpu** **Backend**.|Drains the MtgsCommandQueue asynchronously; flushes render passes and presents onVSync command (59.94 Hz/ 50.00 Hz).|Writes asynchronous LOCAL_TO_HOST (TRXDIR=1) readback buffers to shared staging memory for Thread 1 (VIF1/GIFFIFO readbacks); compositesegui OSD overlay onto host swapchain.|
|---|---|---|---|
|**Shared** **Foundation:** **Fastmem** **Arena** (ps2-common )|**4 GB** **Uncommitted** **OS Virtual** **Address Space** (mmap/ VirtualAlloc2) mapping32 MB RDRAM,16 KB Scratchpad, and 4 MB BIOS, backed byvtlb4 KB software page tables.|Zero-cycle hardware MMU translation on RAM hits; traps to OS SIGSEGV/VEH handler on MMIO (0x10000000+) or SMC page writes.|Backpatches faulting 8/12-byte JIT load/store slots in **Thread 1**to jump directly tovtlb MMIO/SMC trampolines on subsequent executions.|



### **Cargo Workspace and Module Hierarchy** 

The Cargo workspace is partitioned into ten specialized crates with strict acyclic dependency rules so that the core emulation engine, JIT backend, and GS renderer can be compiled into a desktop binary, an Android .so JNI library, a Libretro core, or a headless CI test runner (ps2autotests and .gs dump player). 

|**Crate**|**Architectural**|**Core Responsibilities & Key**|**External Crate**|
|---|---|---|---|
|**Name**|**Layer**|**Modules**|**Dependencies**|



|**ps2-com** **mon**|Foundation|Fixed-size integers, bitfield macros,ps2floatbit-exact float model, OS virtual memory (mmap/VirtualAlloc2), and SIGSEGV/VEHfault-handler registry.|bitflags,bytemuck, libc,windows-sys, thiserror,tracing|
|---|---|---|---|
|**ps2-jit**|Code Generation|Cross-platform x86-64 and AArch64 code emitter, executable page allocator (W^X/ dual-mapped), register allocator, and SMC block invalidation table.|dynasmrt,dynasm, smallvec, rustc-hash|
|**ps2-cdvd**|Media & I/O|Sector reader and async read-ahead cache for.iso, .bin/.cue,.cso, and.chdv5 images; ISO9660 parser to extract game serial (SLUS-xxxxx) and ELF CRC.|chd,flate2, lz4_flex, memmap2|
|**ps2-gs**|Graphics Engine|GIF packet parser (PATH1/2/3), 4 MB swizzled eDRAM local memory table, 8 KB page-granular Texture Cache, Channel Shuffle detector, reference SW rasterizer, and wgpu/ nativeashVulkan HW backend.|wgpu,ash, gpu-allocator, xxhash-rust (XXH3-64 texture hashing)|
|**ps2-spu2**|Audio Engine|Dual-core 48-voice ADPCM decoder, hardware ADSR envelope generator, Gaussian pitch interpolator, reverb processor, and time-stretching output resampler.|rubatoor custom Sinc/SoundTouch resampler|



|**ps2-core**|Emulation Core|Scheduler(event queue),Ee (R5900 + COP0 + COP1), Vu0/Vu1(microVU+MTVU worker),Vif0/Vif1,Dmac(10 channels + MFIFO),Ipu (MPEG-2),Sif,Iop(R3000A + .IRXHLE hooks),Sio2(Pad + MemCard), andGameDb (GameIndex.yamlloader).|crossbeam-channe l,serde, serde_yml,lz4_flex (save states)|
|---|---|---|---|
|**ps2-gs-ru** **nner**|Tooling / CI|Standalone headless CLI binary that loads.gs,.gs.xz, and.gs.zstdumps, replays them throughps2-gs, dumps output frames as PNG, and computes SSIM/MSE against golden images.|clap,zstd,xz2, image|
|**ps2-autot** **est**|Tooling / CI|Headless runner that boots ps2autotests .elfand.irx binaries, captures stdout via SIF/DECI2TTY hooks, and diffs output against.expected files.|clap,similar (diffing)|
|**ps2-ui-de** **sktop**|Desktop Frontend|Main desktop application (Windows, Linux, macOS) providing game library grid, per-game settings inspector, debugger, and in-game OSD overlay.|slint/cxx-qtor egui,winit, sdl3-sys,cpal,rfd|
|**ps2-ui-an** **droid**|Mobile Frontend|Thin JNI bridge (cdylib) exposingps2-coreandps2-gs (VulkanANativeWindow) to a native Kotlin / Jetpack Compose Android UI and touch overlay.|jni,ndk,ndk-sys, android_logger|



## **4. Existing Emulators and Architectural Tradeoff Analysis** 

### **Landscape of PS2 Emulators (2026 Status)** 

Studying existing open-source PS2 emulators provides a comprehensive reference for solved hardware edge cases. The following table compares the architecture, accuracy, performance, licensing, and current status of every major PS2 emulation project. 

|**Emulato** **r**|**Status** **(2026)**|**Licens** **e**|**Suppo** **rted** **Platfor** **ms**|**CPU / VU** **Architectur** **e**|**GS** **Rende** **rer** **Backe** **nds**|**BIOS** **Requireme** **nt &** **Compatibili** **ty**|
|---|---|---|---|---|---|---|
|**PCSX2** **(v2.x /** **v2.8)**|**Active** (Flagship reference )|GPLv3|Windo ws, Linux, macOS (x86-64 ; Rosett a 2 on Apple Silicon)|iR5900 (x86-64), iR3000A, microVU (mVU0/mV U1),MTVU, vtlb fastmem|Vulkan , D3D12 (defaul t on Nvidia/ AMD in v2.8), D3D11 , Metal, Open GL, SW|**LLE BIOS** **Required**; **~99.5%** **playable** (only ~4 unplayable titles).|
|**Play!** **(jpd002** **v0.70+)**|**Active**|BSD-2 -Claus e|Windo ws, macOS , Linux, Androi d, iOS, Web (Wasm )|Custom C++ intermediat e IR (Jitter) targeting x86-64, AArch64, and Wasm; VU1 JIT|Vulkan , Open GL / GLES, D3D, Metal, SW|**No BIOS** **Required** **(Pure HLE** **Kernel/IOP)** ; ~600+ playable games (~25% library).|



|**ARMSX2** **(v2.8 /** **v2.9)**|**Active** (Primary open-sou rce ARM64 port)|GPLv3|Androi d (ARM6 4), iOS (iPSX2 fork with JIT)|Native ARM64 EE/IOP dynarecs, aVU (ARM64 microVU), SIGSEGV-b ackpatch fastmem|Vulkan 1.2+, Open GL ES|**LLE BIOS** **Required**; High compatibility inheriting PCSX2 GameIndex. yaml.|
|---|---|---|---|---|---|---|
|**AetherS** **X2 /** **NetherS** **X2**|**Disconti** **nued** (Commun ity-patche d APK)|Proprie tary / Closed binary|Androi d (ARM6 4), Linux/ macOS ARM64 (histori cal)|Native ARM64 port of PCSX2 by Tahlreth; NetherSX2 patches GameIndex .yaml& UI|Vulkan , Open GL ES, Softwa re|**LLE BIOS** **Required**; ~95%+ compatibility on Snapdragon /Adreno devices.|
|**parallel-** **ps2 /** **LRPS2**|**Active** (Libretro cores)|GPLv3|Windo ws, Linux (x86-64 & ARM64 ), macOS|PCSX2 x86-64 + native ARM64 EE/IOP + aVU microVU1 transplant + ps2float.c|ParaL LEl-G S (Vulka n compu te shader SW rasteri zer) & PCSX 2 HW render ers|**LLE BIOS** **Required**; High compatibility with cycle-accur ate compute GS option.|



|**DobieSt** **ation**|**Inactive** (Researc h reference )|GPLv3|Windo ws, Linux, macOS|Clean C++ interpreters + WIP x86-64 EE/VU JITs|Open GL Hardw are, Softwa re|**LLE BIOS** **Required**; Boots BIOS and enters in-game on select titles (_FFX_, _SotC_).|
|---|---|---|---|---|---|---|



### **Why PCSX2 Achieves 99.5% Compatibility and High Speed** 

PCSX2's compatibility and performance leadership stems from four pragmatic engineering pillars refined over twenty years. First, its **vtlb virtual memory and backpatched fastmem subsystem** (pcsx2/vtlb.cpp) divides the 4 GB EE address space into 4 KB pages and emits a single host memory instruction (mov reg, [fastmem_base + guest_addr]) for direct RAM and Scratchpad accesses. Whenever a guest address hits a hardware MMIO register or a write-protected code page (mprotect), the OS fault handler (SysPageFaultSignalFilter in LnxHostSys.cpp) catches the SIGSEGV, inspects the faulting host instruction, and either marks the JIT block dirty or backpatches the machine code to invoke a software vtlb handler. Second, **microVU and MTVU** —originally authored by cottonvibes (pcsx2/VU.h, pcsx2/x86/microVU*.inl, pcsx2/MTVU.cpp)—analyze entire VU0 and VU1 microprograms statically, dead-code eliminate unread MAC, Status, and Clipping flag updates, and offload VU1 execution onto a dedicated thread. Third, PCSX2's **GS Texture Cache and Shuffle Heuristics** (pcsx2/GS/Renderers/HW/GSTextureCache.cpp), refined by refractionpcsx2, lightningterror, TellowKrinkle, and TJnotJT, track sub-page valid rectangles, dirty page bitmasks, and format overlaps (Tex in RT), culminating in PCSX2 v2.8's elimination of expensive texture copies and render-pass transitions during channel shuffles. Fourth, PCSX2 relies on **GameIndex.yaml** (bin/resources/GameIndex.yaml, parsed by pcsx2/GameDatabase.cpp), a comprehensive per-game database indexed by disc serial and ELF CRC that automatically configures EE/VU rounding modes, clamping modes, timing hacks, and GS hardware fixes without user intervention. 

### **Play!'s HLE Architecture vs. LLE BIOS: What HLE Costs on Heavy Games** 

Unlike PCSX2, **Play!** (jpd002/Play-) operates without a Sony BIOS dump by implementing High-Level Emulation (HLE) of both the EE OS Kernel (SYSCALL dispatcher for threads, semaphores, TLB management, and GS interrupt handlers) and the IOP modular operating system (Source/iop/, implementing Iop_Bios, Iop_SifMan, Iop_Cdvdman, Iop_McMan, Iop_PadMan, and Iop_LibSd directly in C++). While Play!'s HLE architecture eliminates the legal and logistical friction of obtaining a BIOS dump and enables wide cross-platform deployment, relying exclusively on HLE incurs a severe compatibility penalty on heavy PS2 games. Specifically, first-party and late-era heavy titles ( _God of War_ , _Jak and Daxter_ , _Gran Turismo 4_ , _Shadow of the Colossus_ , _Final Fantasy XII_ ) bundle custom IOP .IRX modules on the game disc that bypass standard LIBSD or CDVDMAN calls, stream custom compressed archives directly through low-level CDVD hardware registers, or execute game-specific audio synthesis and decompression algorithms directly on the IOP R3000A processor. 

Furthermore, many commercial engines patch the EE kernel's internal thread control blocks (TCB) directly or depend on exact asynchronous delay cycles between an EE sceSifCallRpc invocation and the resulting IOP interrupt response. Consequently, the optimal architecture requires a user-supplied **LLE BIOS dump** (4 MB .bin) as the primary execution path for commercial games to guarantee >95% compatibility, while providing a lightweight **HLE BIOS and ELF Loader mode** strictly for booting standalone homebrew .elf files and running ps2autotests in continuous integration without copyrighted assets. 

### **Core Subsystem Comparison Tables (Deliverable 3)** 

**Table 2: CPU and VU Execution Strategies** 

|**Execution** **Strategy**|**Throughpu** **t (vs.** **Interpreter)**|**Compilation** **Latency**|**Implementatio** **n Complexity**|**Ideal Use Case** **in PS2** **Emulator**|
|---|---|---|---|---|
|**Pure** **Interpreter**|1. 0 × (Baseline; ~2–5 FPS on heavy games)|$0\text{ }\mu\text{s}$|Low|Phase 1–2 bring-up, debugging, differential trace reference (MVU_DIFF), and self-modifying single-shot stubs.|
|**Cached** **Interpreter**|2. 5 × –4. 0 ×|$<5\text{ }\mu\text{s}$|Low–Medium|Pre-decoded instruction structs in fixed 4 KB pages; ideal fallback for IOP (R3000A) or fallback platforms.|
|**Custom** **Single/Two-P** **ass JIT** **(dynasm-rs)**|15 × –35 ×|**$10 –** **50\text{** **}\mu\text{s}** **$ / block**|High|**Recommended** **Primary** **Backend**for EE (R5900),VU0, VU1, andIOP|



|||||on x86-64 and ARM64.|
|---|---|---|---|---|
|**Cranelift JIT**|20 × –40 ×|$500 – 5,000\text{ }\mu\text{s}$ / block|Medium|**Not** **Recommended** : Compilation speed is 20 × –100 × too slow for dynamic VU1 microcode and SMC thrashing (pchanremoved Cranelift for this exact reason).|
|**LLVM ORC** **JIT**|25 × –45 ×|$5,000 – 50,000+\text{ }\mu\text{s}$|Very High|**Unusable at** **Runtime**: Heavy binary size (~80+ MB) and massive compilation pauses during gameplay.|
|**Static AOT** **Recompilatio** **n**|35 × –60 ×|Offline (minutes)|Extreme|Per-game native ports (gow-pc-port, ps2-native-reco mp); cannot handle arbitrary commercial discs without a fallback runtime JIT for dynamic overlays.|



**Table 3: GS Rendering Approaches** 

|**Rendering** **Approach**|**Visual** **Accuracy**|**Max** **Upscaling** **Resolution**|**Host** **Hardware** **Bottleneck**|**When to Use /** **Architectural** **Role**|
|---|---|---|---|---|
|**Single-Threade** **d CPU** **Software** **Rasterizer**|100% Reference (Bit-exact eDRAM)|Native ( 1 × ) 640 × 448|Extreme CPU single-core bottleneck (5–15 FPS on heavy games)|**Phase 2** **Validation &** **Golden** **Reference**: Used in ps2-gs-runner to verify register/blendin g math against HW renderer.|
|**Multithreaded** **SIMD CPU** **Software** **Rasterizer**|99.9% Reference|Native 1 ×|Requires 4–8 fast desktop AVX2/NEON cores (GSdx-SW)|Debugging obscure hardware bugs (F9toggle in PCSX2) and rendering 2D/FMV-heavy titles on multi-core desktops.|
|**Vulkan** **Compute** **Shader** **Rasterizer** **(ParaLLEl-GS)**|99.9% (Bit-exact 4 MB VRAM in SSBO)|1 × –2 × (Super-sampl ed)|Heavy GPU compute fill-rate & shared-memor y pressure|Optional accuracy backend on desktop discrete GPUs; eliminates texture-cache and channel-shuffle heuristics entirely.|



**Primary** 95% – **Production** Texture cache **Traditional** 99.5% **Renderer** invalidations, **GPU Hardware** (Requires 1 × –8 × **(** 4𝐾 across barriers, and **Renderer** TC & **/** 8𝐾 **)** Desktop, Apple CPU **(wgpu / Vulkan)** shuffle Silicon, readbacks heuristics) Android, and ARM SBCs. 

#### **Table 4: Graphics APIs for PS2 Hardware Emulation** 

|**Gra** **phi** **cs** **API**|**Tar** **get** **Plat** **for** **ms**|**Dual-Source** **Blending**|**ROV / Fragment** **Interlock / FB Fetch**|**Co** **mp** **ute** **&** **Bin** **dles** **s** **Sup** **port**|**Recom** **mendati** **on for** **New** **Rust** **Emulato** **r**|
|---|---|---|---|---|---|
|**wg** **pu** **(v2** **5 –** **v30** **+)**|Win dow s, Linu x, mac OS, And roid , iOS , We b|**Supported** (Features::DUAL_SO URCE_BLENDING)|No native ROV / FB Fetch exposed in safe WGSL API|Full Co mpu te + Bind less arra ys|**Use for** **Phase** **3–4** **Cross-P** **latform** **Baselin** **e & UI** **Renderi** **ng**.|



|**Nat** **ive** **Vul** **kan** **1.3** **(as** **h)**|Win dow s, Linu x, And roid (Mo lten VK on mac OS)|independentBlend+ dualSrcBlend|VK_EXT_fragment_shade r_interlock(Desktop) & VK_EXT_rasterization_or der_attachment_access (ARM/Qualcomm)|Full Des cript or Inde xing , Pus h Con stan ts, Sub pas ses|**Recom** **mended** **High-Pe** **rforman** **ce** **Backen** **d** **(ps2-gs)** : Essentia l for barrier-fr ee DATE/bl ending and mobile Adreno/ Mali tuning.|
|---|---|---|---|---|---|
|**Dir** **ect** **3D** **12** **(wi** **ndo** **ws** **crat** **e)**|Win dow s 10/ 11, Xbo x|Supported|Rasterizer Order Views (ROV)|Full SM 6.6 bind less|Seconda ry Window s backend (or rely on Vulkan 1.3 / wgpu DX12 backend ).|
|**Met** **al** **(obj** **c2-** **met** **al)**|mac OS, iOS / iPa dO S|Supported (@blend_srcin MSL)|Programmable Blending (Native Tile Framebuffer Fetch + Raster Order Groups)|Full Arg ume nt Buff ers|**Ideal for** **Apple** **Silicon**: Apple's TBDR architect ure makes|



||||||PS2 program mable blending nearly free via tile memory.|
|---|---|---|---|---|---|
|**Op** **en** **GL** **4.6** **(glo** **w)**|Win dow s, Linu x Des ktop|GL_ARB_blend_func _extended|GL_ARB_fragment_shade r_interlock& GL_NV_texture_barrier|Co mpu te + Bind less text ures|**Do Not** **Build** **for New** **Project**: Depreca ted on macOS, poor driver threadin g on mobile (GLES), superse ded by Vulkan.|



**Table 5: UI Frameworks Across Desktop and Mobile** 

|**UI** **Framewo** **rk**|**Rust** **Integratio** **n Maturity**|**Visual** **Polish** **(PCSX2** **Parity)**|**In-Game** **OSD /** **Big** **Picture** **Support**|**Mobile** **(Android)** **Readiness**|**Verdict**|
|---|---|---|---|---|---|



**Best All-in-One Seamles Choice for s** : **Solo Dev** Good Renders **(MVP** Works via (Customizab directly **through egui** Native winit le theme, over the **Phase 5)** : **(eframe +** pure Rust Android, tables, wgpu/Vul One **egui-wgp** (Best-in-cl though dockable kan codebase **u)** ass) lacks native debugger game powers OS look panels) swapchai desktop n at 60+ library, live FPS debugger, and in-game OSD. **Strong Excellent Alternative** (Native-style **for Polished** First-class Can d widgets, Supported **Desktop** Rust + render to **Slint** smooth via Android **Library UI** declarative texture animations, backend paired with .slint DSL overlay low egui for the memory) in-game OSD. Requires **Gold** separate Only choose **Standard** Dear Poor if writing a **Qt 6** High (Exact ImGui ergonomics thin C++ Qt 6 **(cxx-qt or** friction match to OSD on Android executable **C++** across FFI PCSX2 v2.x renderer compared linking **frontend)** boundary and (as to Kotlin libps2_core.a DuckStation PCSX2 via C-ABI. ) does) 

|**iced**|Native pure Rust (Elm architectur e)|High (Clean modern desktop aesthetic)|Harder to embed as an in-game HUD + dense hex debugger|Experiment al on mobile|Inferior to eguifor dense multi-window emulator debuggers (disassembler s, VRAM viewers).|
|---|---|---|---|---|---|
|**Tauri** **(Web UI +** **Rust)**|Mature IPC|High (HTML/CSS /React)|Cannot easily render zero-cop y over game swapchai n without WebView composit or latency|Supported, heavy RAM footprint|**Avoid**: WebView IPC overhead and multi-process GPU surface sharing complicate low-latency frame pacing.|
|**Native** **Kotlin +** **Jetpack** **Compose**|Clean JNI boundary to ps2-core|**Gold** **Standard** **on Android** (Matches NetherSX2 / ARMSX2 UX)|Native Android Overlay + Vulkan HUD|**100%** **Native** **Android**|**Mandatory** **for Phase 6** **Android** **Release**: Pair a Rust core with a thin Kotlin/Compo se frontend.|



## **5. Heavy-Game Deep Dive and Performance Engineering Matrix** 

To build an emulator that runs the heaviest PS2 games at a locked 50/60 FPS, every design decision in the CPU recompiler, threading model, and GS hardware renderer must be reverse-engineered directly from the failure modes and bottlenecks of those specific titles. 

### **Bottleneck Classification and Hardware Tricks of Heavy PS2 Games** 

**The VU1 and EE Synchronization Giants: Shadow of the Colossus and God of War I & II** 

Team Ico's _Shadow of the Colossus_ pushes the EE, VU1, and GS simultaneously to such extremes that real PS2 hardware frequently drops to 15–20 FPS during colossus encounters. The engine executes complex inverse kinematics, dynamic boss-mesh collision detection, and procedural fur and wind simulation across the EE and VU1, followed by multi-pass stencil-like shadow volumes and HDR bloom on the GS. Because the game uses a double-buffered VSync lock and ties physics and grip-stamina timers to the frame loop, any emulated EE stall halves the output framerate to 30 FPS or lower. Overclocking the emulated EE Cycle Rate (+50% to +150% or 180%) allows the guest game engine to complete its frame logic inside a single 16.67 ms VBlank window, unlocking a true 60 FPS presentation on fast desktop CPUs. Conversely, on weak host processors such as a Raspberry Pi 5 or budget ARM handheld, the host CPU cannot keep pace with 294.9 MHz of EE instructions, requiring **EE Cycle Rate Underclocking (60%–75%)** so the emulator maintains 100% wall-clock speed (at a lower internal game framerate) without audio buffer underruns. 

In Santa Monica Studio's _God of War I & II_ , empirical profiling from the static recompilation project gow-pc-port confirms that **VU1 microcode execution and VIF1 packet unpacking constitute the primary CPU bottlenecks** , followed closely by EE memory address translation and GS uniform and barrier submission. _God of War_ also relies heavily on post-processing bloom passes that render to downsampled framebuffers in eDRAM and blend them back over the primary display buffer, triggering Tex in RT (Texture Inside Render Target) hazards that require accurate sub-page target merging. 

#### **The Streaming, Scratchpad, and Mipmap Hazards: Jak and Daxter and Ratchet & Clank** 

Naughty Dog's _Jak_ series (built on the custom GOAL Lisp engine) and Insomniac's _Ratchet & Clank_ series continuously stream code and geometry into EE RAM, utilize the 16 KB Scratchpad (0x70000000) and fromSPR/toSPR DMA channels as a high-speed software cache, execute heavy COP2 macro-mode and VU1 microcode, and rely on **hardware GS mipmapping (TEX1.MMIN)** . On the PS2, mipmap base pointers (MIPTBP1_1/2, MIPTBP2_1/2) are specified manually in GS registers, and games frequently render dynamically into Level 0 or reuse lower mipmap pages for unrelated data. In _Ratchet & Clank_ (such as on Planet Novalis), enabling naive GPU mipmap generation blurs or corrupts terrain textures, whereas disabling mipmapping leaves harsh LOD seams or corrupted texels. In _Jak and Daxter: The Precursor Legacy_ , character eyes are rendered by dynamically generating a paletted texture in VRAM; if the texture cache fails to invalidate overlapping CLUT and render-target pages or miscalculates mipmap LODs, characters render with black hollow eye sockets. Furthermore, _Ratchet & Clank: Going Commando_ (R&C 2) exhibits timing-sensitive EE/VU synchronization during cutscenes (notably the Jamming Array sequence) that crashes if EE-to-VU cycle ratios drift. 

#### **The Framebuffer Readback and Interlaced Resolution Engines: Gran Turismo 3 and 4** 

Polyphony Digital's racing engine runs at 60 fields per second using heavy VU1 car and track shaders (~150 active microVU1 blocks in an in-race profile, where mvu1 consumes ~75% of the MTVU thread with a flat distribution across blocks), high-frequency IPU MPEG-2 replay/intro decoding, and two notorious GS rendering techniques. First, whenever the sun enters the camera frustum, _Gran Turismo 4_ transfers a small region of the rendered framebuffer back from the GS to the EE via VIF1/GIF (BUSDIR=1, TRXDIR=1 Local-to-Host 

transfer) to measure pixel occlusion for dynamic lens flares. On a modern GPU, a synchronous vkCmdCopyImageToBuffer followed by vkWaitForFences stalls the entire host graphics pipeline for 2–5 ms, causing severe frame drops whenever the sun appears unless mitigated via asynchronous readbacks or a lens-flare readback bypass on mobile devices. Second, when 1080i mode is enabled in _Gran Turismo 4_ , the game does not render a 1920 × 1080 framebuffer (which would require 8.29 MB, exceeding the entire 4 MB eDRAM). Instead, it renders a 576 × 960 **interlaced field buffer** and configures the PCRTC DISPLAY magnification registers (MAGH=2, 3 × horizontal stretch) to output 1728 × 960 centered inside a 1920 × 1080 frame, while dynamically switching back to 640 × 448 (480i) for menus and FMVs. If the hardware renderer fails to track dynamic PCRTC SMODE2/DISPLAY register changes or forces progressive deinterlacing hints improperly (as documented in ParaLLEl-GS Issue #130), the image tears, flickers during license tests (Tsukuba B-5 and Motegi B-8), or collapses into half-height artifacts. 

#### **The Channel Shuffle and DATE Post-Processing Engines: Metal Gear Solid 2 & 3, Burnout 3, Black, and Need for Speed** 

Because the GS lacks programmable fragment shaders, Konami, Criterion, and EA Black Box implemented depth-of-field, thermal goggles, motion blur, color grading, and bloom via **Channel Shuffles (Texture Shuffles)** . In a channel shuffle, a game takes a 32-bit PSMCT32 (RGBA8888) framebuffer, binds the exact same eDRAM page address as a 16-bit PSMCT16 or 8-bit PSMT8/PSMT8H texture and/or render target, and draws dozens of 8-pixel-wide or 16-pixel-wide vertical sprite strips across the screen with UV offsets. Because PSMCT32 and PSMCT16 swizzle pixels differently inside each 256-byte block, this operation shifts the Red and Green bytes of a 32-bit pixel into the Blue and Alpha positions (or vice versa), or routes a single color channel through a 256-entry CLUT lookup table as a non-linear tone-mapping curve. Emulating this naively forces the hardware renderer to flush the render pass, copy textures, re-swizzle on the GPU, and execute hundreds of tiny draw calls per frame—making a single channel shuffle more expensive than rendering all 3D geometry in the scene. PCSX2 v2.8 achieved major performance gains in titles such as _Manhunt 2_ , _Metal Gear Solid 3_ , and _Need for Speed: Most Wanted_ by pre-detecting the shuffle pattern (src == rt or page overlap with PSM < 32-bit and vertical strip draws) and folding the channel swizzle and UV offset directly into a single fullscreen shader pass without copying the render target. 

#### **The Snowblind Engine Texture Cache Thrashers and Depth-as-Color RPGs** 

Snowblind Studios' engine—powering _Baldur's Gate: Dark Alliance I & II_ , _Champions of Norrath_ , _Champions: Return to Arms_ , and _Fallout: Brotherhood of Steel_ —combines two severe emulation challenges. First, it renders dynamic shadows and point lights into a framebuffer and immediately samples that memory as thousands of 8-bit (PSMT8) palletized textures across floor tiles. Without **GPU-accelerated 8-bit texture conversion** (converting PSMCT32 render targets to R8_UINT via a shader so CLUT lookups occur on the GPU) and **Fast Texture Invalidation** , the texture cache invalidates and re-uploads thousands of pages per second, causing extreme speed fluctuations. Second, Snowblind games exhibit the "half-screen bug" by configuring FRAME with an oversized buffer width (FBW=20, or 1280 / 1920 pixels) to pack side-by-side buffers into the same horizontal page stride while only displaying half the width via DISPFB, requiring strict decoupling of FRAME.FBW from the active valid rectangle in the texture cache. Similarly, tri-Ace's RPG engine ( _Valkyrie Profile 2:_ 

_Silmeria_ and _Star Ocean: Till the End of Time_ ) binds the 24-bit/32-bit Z-buffer (PSMZ24/PSMZ32) as a color texture (Tex in RT) to compute depth-of-field blur and refraction while rendering color data into the depth buffer, requiring bidirectional GPU depth-to-color conversion shaders and careful VU floating-point clamping (#1475). 

### **Tradeoff Analysis of PCSX2 Speedhacks and GS Hardware Fixes** 

Every accuracy slider and hardware fix exposed in PCSX2 exists to resolve a fundamental mismatch between the PS2's synchronous, unified-memory hardware and modern asynchronous, discrete CPU/GPU architectures. 

|**Setting /** **Hack** **Name**|**Subsyst** **em**|**Technical** **Mechanis** **m**|**Performa** **nce Gain**|**Accuracy** **Cost &** **Known** **Failure** **Modes**|**Recommended** **Default per** **Device Class**|
|---|---|---|---|---|---|
|**EE Cycle** **Rate (50%** **to300%) **|EE Schedul er|Scales the number of EE instruction s executed per HBlank/VB lank clock quantum (scalar * 294.912 MHz).|<100%: $+15\%–3 5\%$ host FPS on weak CPUs; >100%: unlocks 60 FPS in _SotC_/ _ZOE2_.|<100% causes internal game slowdown /frame-ski pping; >100% breaks games with hardcode d busy-wait loops.|**100%**(Desktop / Apple Silicon / Flagship Android);**75%**on Low-Power ARM SBCs.|
|**EE Cycle** **Skipping** **(0 – 3)**|EE / Bus|Fast-forwa rds the EE timer counters when the EE enters idle polling loops or waits on|$+10\%–2 5\%$ on CPU-bou nd titles.|Causes audio stutter, FMV desync, or physics glitches if DMA finishes too early|**0 (Disabled)** globally; enable viaGameDbonly on verified titles.|



|||DMA/VIF interrupts.||relative to EE code.||
|---|---|---|---|---|---|
|**VU Cycle** **Stealing (0** **– 3)**|VU1 / Schedul er|Artificially inflates the cycle count attributed to VU1 runs, stealing cycles from the EE quota.|$+10\%–3 0\%$ on single/dua l-core devices.|Causes missing geometry, flickering textures, and false 60 FPS readings where internal game speed drops (e.g., _GT4_,_Ring_ _of Red_).|**0 (Disabled)**: Deprecated on modern multi-core CPUs whenMTVUis active.|
|**MTVU** **(Multithrea** **ded VU1)**|VU1 / VIF1|Offloads VIF1 unpacking and microVU1 execution to a dedicated worker thread via a lock-free SPSC ring buffer.|**$+20\%–4** **5\%$**on -core ≥3 hosts across VU1-heav y titles.|Slight latency when a game polls CFC2/ VU1 memory directly from the EE (SyncStat sstalls).|**On (Default)** across all 4+ core targets (Desktop, Apple Silicon, Android, Pi 5).|



|**Instant** **VU1**|VU1 / GIF|Executes VU1 microprogr ams with zero simulated cycle delay when MTVUis off or when XGKICK synchroniz es.|$+5\%–10 \%$ by reducing scheduler context switches.|Breaks games that modify VU1 memory viaVIF1 while a microprog ram is mid-flight (e.g., _Syberia 2_ requires --vif1-inst ant-xfer=0 ).|**On (Default)**, automatically disabled via GameIndex.yaml for timing-sensitive titles.|
|---|---|---|---|---|---|
|**mVU Flag** **Hack**|microVU JIT|Assumes Status/MA C/Clip flags are only read at the end of a microprogr am or branch rather than maintainin g all 4 pipeline stages per block.|**$+15\%–2** **5\%$** faster VU JIT execution and smaller code cache footprint.|Graphical vertex explosion s (spiky polygons) in games that interleave flag updates across dynamic branches.|**On (Default)**, overridden per-game by GameIndex.yaml.|



|**FPU / VU** **Clamping** **Mode** (None, Normal, Extra,Full)|COP1 / VU JIT|Normal: clamps NaN/±Inf on arithmetic results; Extra: clamps every instruction; Full: activates ps2float software emulation.|Normalis 3 × –5 × faster thanFull soft-float.|Insufficien t clamping causes collision fall-throug hs, invisible walls, or camera NaN locks.|**Normal** **(Default)**; auto-escalate to Extra/Fullvia GameIndex.yaml.|
|---|---|---|---|---|---|
|**Blending** **Accuracy** (Min, Basic, Med,High, Full)|GS HW Rendere r|Controls when the renderer uses Dual-Sour ce Blending vs. per-draw barriers / ROV / SW blending for HDR COLCLIP, Cdblends, and FBMASK.|Basicis 2 × –4 × **faster**on GPU fill-rate than Full/Ultra at 3 ×+ upscale.|Min/Basic causes dark or overly bright smoke, missing shadows, or incorrect color grading in _MGS3_/ _ZOE2_.|**Basic**(Mobile / ARM SBCs); **Medium/High** (Desktop & Apple Silicon).|



|**GPU** **Palette** **Conversio** **n (8-bit** **Textures)**|GS Texture Cache|Keeps PSMT8/PS MT4 textures in indexed format on the GPU and resolves the CLUT in the fragment/c ompute shader instead of CPU decoding.|Huge CPU/PCI e bandwidth savings in _Champion_ _s of_ _Norrath_ and_MGS_.|Increases fragment shader complexit y and texture cache swizzle checks.|**On**for Snowblind and CLUT-heavy games via GameIndex.yaml.|
|---|---|---|---|---|---|
|**Fast** **Texture** **Invalidatio** **n**|GS Texture Cache|Skips exhaustive sub-page valid-rect scanning when invalidatin g dirty render targets.|**$+30\%–6** **0\%$**in Snowblin d engine titles.|Can leave stale sub-textur es if a game packs multiple micro-targ ets into a single page.|**Off globally**;**On** **automatically** via GameIndex.yaml for Snowblind games.|
|**Auto** **Flush** **(Hardware** **/ Software)**|GS HW Rendere r|Forces a render-pas s barrier whenever a primitive samples from the currently bound render target (Tex in RT).|Costs $10\%–30 \%$ GPU speed when active.|Required to fix missing shadows, bloom, or ghosting in games that read/write the same buffer mid-draw.|Managed per-game via GameIndex.yaml.|



Rasterizes flat 2D SPRITE Avoids primitives 100s of Adds on the GPU CPU-to-G CPU barrier PU Managed **CPU** GS HW (using the flushes upload per-game via **Sprite** Rendere SW during overhead GameIndex.yaml **Render** r rasterizer) complex if applied (CpuSpriteRende and 2D to 3D rBW). uploads UI/font/filt particle the result er sprites. to the HW passes. texture cache. 

### **Concrete Performance Targets and Per-Frame 16.67 ms Time Budget** 

To maintain a locked **60.0 FPS (NTSC)** , the emulator operates under a strict wall-clock ceiling of **16.67 ms per frame** (20.00 ms for 50 Hz PAL). Averaging 16.67 ms across a session is insufficient: to eliminate perceptible micro-stutter, the 𝑃99 **frame time must remain below 16.67 ms** , meaning typical ( 𝑃50 ) critical-path frame processing across Thread 1 (EE/DMAC/IOP), inter-thread MTVU synchronization, and Thread 3 (MTGS GPU submission) must complete within **11.4 to 14.5 ms** on desktop, Apple Silicon, and flagship Android hardware, reserving a 1.4 to 1.8 ms cushion for OS scheduling jitter and VSync presentation. 

### **Heavy-Game Performance and Architectural Matrix (Deliverable 4)** 

The following matrix catalogs 25 of the most demanding PS2 titles and franchises, classifying their primary bottlenecks, hardware tricks, required accuracy features, PCSX2 GameIndex.yaml hacks, minimum and recommended hardware across 1 × (480p), 2 × (720p), and 3 × (1080p) internal resolutions, and the exact architectural decisions each game forces upon the emulator. 

|**Game /** **Franchis** **e**|**Primary** **Bottlene** **ck** **Classific** **ation**|**Hardware** **Tricks** **Used by** **Game** **Engine**|**Required** **Accuracy** **Features &** **Known** **GameIndex** **Hacks**|**Hardwa** **re** **Needed** **(1×1\ti** **mes1× /** **2×2\tim** **es2× /** **3×3\tim**|**Architectur** **al** **Decisions** **Forced on** **Emulator**|
|---|---|---|---|---|---|



|||||**es3× @** **60 FPS)**||
|---|---|---|---|---|---|
|**Shadow** **of the** **Colossus** (SCUS-9 7472)|**EE-Bou** **nd +** **VU1-Bo** **und +** **GS** **Blendin** **g**|Ties physics/grip to framerate; heavy EE job scheduler; complex VU1 fur/IK microcode; GS stencil-alph a shadows & bloom.|MTVUOn; EE Cycle Rate +150%for true 60 FPS on fast PCs, or 60%–75%on weak SBCs to avoid audio drop.|**1x**: Cortex- A76 (75% EE) / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X + GTX 1660S|Forces an ultra-fast EE JIT (fastmem), user-configu rable**EE** **Cycle Rate** **scaler**, and zero-copy MTVUring buffer.|
|**God of** **War I & II** (SCUS-9 7399/ 97481)|**VU1-Bo** **und +** **VIF1** **Unpack** **+ GS** **Tex in** **RT**|Massive dynamic VU1 microcode blocks; heavy VIF1 unpacks; multi-pass downsampl ed bloom in eDRAM (Tex in RT).|MTVUOn; TextureInsideRT = MergeTargets; Half-Pixel Offset (Special) for bloom alignment.|**1x**: SD 865 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1|Forces **native** **SIMD VIF1** **unpackers**, loop-preser ving VU1 block compilation, and sub-page Tex in RT tracking.|



|||||**3x**: Core i5-1140 0 / RTX 3050||
|---|---|---|---|---|---|
|**Ico** (SCUS-9 7113)|**GS** **Scanout** **+ EE/VU** **Sync**|240p progressive scanout (SMODE2.I NT=0), soft-shado w volume alpha blending, fog tables, and water refraction readbacks.|Accurate PCRTC non-interlaced scanout scaling; Medium Blending Accuracy.|**1x**: Cortex- A76 / SD 855 **2x**: SD 870 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Forces accurate **PCRTC** **SMODE2/DI** **SPLAY** **scanout** handling for 240p/480i switching and GS fog (FOGCOL).|
|**Gran** **Turismo** **3 & 4** (SCUS-9 7102/ 97328)|**VU1-Bo** **und +** **GS** **Readba** **ck +** **1080i** **PCRTC**|~150 flat hot VU1 blocks in-race; CPU framebuffer readbacks for sun lens flares; GT4 576 × 960𝑖 3 × -stretched|MTVUOn (0.00s EE wait in GT3); async readback caching or lens-flare readback bypass on mobile; Bob/Weave deinterlace.|**1x**: SD 870 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1|Forces **systematic** **VU1** **emitter** **optimizatio** **n** (page-offset folding & NEON shuffles) and **asynchron** **ous** **PBO/stagin**|



|||1080i mode.||**3x**: Ryzen 5 5600 / RTX 3060|**g** **readbacks**.|
|---|---|---|---|---|---|
|**Metal** **Gear** **Solid 2 &** **3** (SLUS-20 062/ 20915)|**GS** **Channel** **Shuffle** **+DATE** **+** **Blendin** **g**|Heavy Channel Shuffles (RG BA) ↔ for DoF, thermal goggles, and post-proces sing; intense DATE& HDR COLCLIP rain/foliage.|Channel Shuffle offset detection; DATEFull/Fast barrier; High Blending Accuracy.|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X / RTX 3060|Forces a **Channel** **Shuffle** **pre-detecto** **r**(shader swizzle instead of RT copy) and **Dual-Sourc** **e Blending** **+** **ROV/DATE** **barriers**.|
|**Jak and** **Daxter I,** **II, 3** (SCUS-9 7124/ 97265)|**EE SMC** **+** **Scratch** **pad** **DMA +** **GS** **Mipmap** **s/CLUT**|GOAL Lisp engine self-modifyi ng code; heavy 16 KB Scratchpad DMA; dynamic CLUT eyes; hardware|Sub-page SMC invalidation; Mipmapping = Basic/Full; accurate CLUT dirty hashing (XXH3-64).|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 3 / Apple M1|Forces **fine-graine** **d SMC** **page** **tracking** (critical on 16 KB Apple/Andro id pages), fast fromSPR/to SPRDMA, and|



|||mipmappin g.||**3x**: Ryzen 5 5600X / RTX 3060|hardware mipmap generation.|
|---|---|---|---|---|---|
|**Ratchet** **& Clank** **1, 2, 3,** **Deadlock** **ed** (SCUS-9 7199+)|**VU1-Bo** **und +** **GS** **Mipmap** **s +** **EE/VU** **Sync**|Massive VU1 instanced geometry; custom mipmap base pointers (MIPTBP1/ 2); tight EE-VU1 cutscene sync in R&C 2.|Mipmapping tuning (#5008/ #11578); EE Cycle Rate / VU sync fix for R&C 2 Jamming Array cutscene.|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 3 / Apple M1 **3x**: Core i7-1170 0 / GTX 1660S|Forces **accurate** **TEX1/MIPT** **BP mipmap** **level** **tracking**in the texture cache and cycle-accur ate VIF1/VU1 sync options.|
|**Snowblin** **d Engine** (_Champio_ _ns of_ _Norrath_, _Baldur's_ _Gate DA_ _I/II_)|**GS** **Texture** **Cache** **Thrashi** **ng +** **8-Bit** **CLUT**|Renders dynamic lightmaps to RT and samples them as thousands of 8-bit PSMT8 textures; FBW=20|FastTextureInvali dation = 1;GPU Palette Conversion (8-bit)On;FBW vs.valid_rect decoupling.|**1x**: SD 8 Gen 1 / Zen 2 **2x**: SD 8 Gen 3 / Apple M2|Forces**GPU** **Compute/S** **hader** **Palette** **Conversion** (R8_UINT+ CLUT texture) and **Fast** **Texture** **Invalidatio** **n**inps2-gs.|



|**Valkyrie** **Profile 2** **& Star** **Ocean 3** (tri-Ace)|**GS** **Depth-a** **s-Color** **+** **Readba** **cks +** **VU** **Clampin** **g**|half-screen stride. Binds 24/32-bit Z-buffer as color texture for DoF/refracti on; renders color into Z-buffer; sensitive VU1 float math.|Depth Emulation On (with GameDb point-sprite/depth tweaks for SO3 #1475); Extra/Full VU clamping.|**3x**: Core i7-6700 K+ / GTX 1080 **1x**: SD 8 Gen 1 / Zen 2 **2x**: SD 8 Gen 3 / Apple M2 **3x**: Ryzen 5 5600X / RTX 3060|Forces **bidirection** **al GPU** **Depth** ↔ **Color** **format** **conversion** **shaders** (PSMZ24 ↔ PSMCT24/ R32F).|
|---|---|---|---|---|---|
|**Zone of** **the** **Enders:** **The 2nd** **Runner** (SLUS-20 554)|**GS** **Fill-Rate** **& Alpha** **Blendin** **g + EE** **Load**|Thousands of overlapping additive/alp ha-blended vector particles and full-screen post-proces|EE Overclock (150%–200%) eliminates native PS2 slowdowns in massive mech battles; Dual-Source Blending.|**1x**: SD 870 / Core i5-4670 K **2x**: SD 8 Gen 2|Forces **Hardware** **Dual-Sourc** **e Blending** (@blend_sr c(0/1)) so particle draws do not trigger render-pass barriers.|



|||sing passes.||/ Apple M1 **3x**: Ryzen 5 5600X / RTX 3060||
|---|---|---|---|---|---|
|**Final** **Fantasy** **X & XII** (SLUS-20 312/ 20963)|**IPU FMV** **Overlay** **+ VU1** **Skinnin** **g +Tex** **in RT**|Real-time 3D characters rendered directly over streaming IPU MPEG-2 FMVs; FFXII depth-of-fiel d and heat haze shuffles.|Threaded/SIMD IPU decoding (IPUWorker); accurateCSC YCbCr-to-RGB conversion.|**1x**: Cortex- A76 / SD 855 **2x**: SD 870 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Forces **SIMD-accel** **erated IPU** **MPEG-2** **IDCT +** **yuv2rgb** and fast streaming texture uploads via PATH3.|
|**Kingdom** **Hearts II** (SLUS-21 005)|**VU1** **Geometr** **y + GS** **Stencil/** **Alpha** **Effects**|High-poly character skinning on VU1, rapid UI sprite batching, and motion/mag ic|MTVUOn; Basic/Medium Blending; GameDb60 FPS patch compatibility.|**1x**: Cortex- A76 / SD 860|Standardize s fast**VIF1** **double-buff** **ered** **TOP/TOPS** **unpacking** and batching of|



|||post-proces||**2x**: SD|2D UI sprite|
|---|---|---|---|---|---|
|||sing.||870 / Apple M1|draws.|
|||||**3x**: Ryzen 5 3600 / GTX 1650||
|**Resident** **Evil 4** (SLUS-21 134)|**EE-Bou** **nd + GS** **Noise/P** **ost-FX**|Port of GameCube engine running heavy AI/collision on EE, letterboxed scanout, and full-screen film grain/blur passes.|MTVUOn; accurate 16-bit dithering (DTHE/DIMX) andTex in RT.|**1x**: SD 865 / Zen 2 **2x**: SD 8 Gen 1 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Requires hardware **GS Dither** **Matrix** **(DIMX)** emulation in the fragment shader to prevent color banding.|



|**Devil** **May Cry** **3:** **Special** **Edition** (SLUS-21 361)|**GS** **Depth/Al** **pha** **Shadow** **s + VU1**|Locked 60 FPS combat engine using stencil-alph a shadows (DATE), motion blur trails, and dynamic lighting.|MTVUOn;DATE barrier support; Half-Pixel Offset alignment.|**1x**: SD 865 / Zen 2 **2x**: SD 8 Gen 1 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Forces low-overhea d**DATE** **destination** **alpha** **testing**(via Stencil or ROV) to keep 60 FPS locked.|
|---|---|---|---|---|---|
|**Burnout** **3:** **Takedow** **n &** **Revenge** (SLUS-21 050)|**EE/VU0** **Physics** **+ GS** **Accumu** **lation** **Blur**|Heavy crash deformation physics on EE/VU0; continuous framebuffer -as-texture accumulatio n motion blur & bloom.|MTVUOn; TextureInsideRT; skybox/reflection software sprite or mipmap fix.|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X / RTX 3060|Forces**fast** **COP2 (VU0** **Macro** **Mode)** **inline JIT** **emission** inside the EE recompiler without context-swit ch overhead.|



|**Black** (SLUS-21 376)|**VU1 +** **GS** **Destruct** **ion** **Particle** **s & Blur**|Criterion's FPS engine; extreme particle overdraw, gun smoke DATE/blend ing, and depth-of-fiel d channel shuffles.|MTVUOn; Channel Shuffle optimization; High Blending Accuracy.|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X / RTX 3060|Demands combined **Channel** **Shuffle +** **Dual-Sourc** **e Blending** throughput under heavy fragment load.|
|---|---|---|---|---|---|
|**Okami** (SLUS-21 410)|**GS** **Multi-Tar** **getTex** **in RT +** **Readba** **cks**|Renders scene, applies multi-pass Sumi-e ink outline and paper canvas overlays, and reads back Celestial Brush strokes.|TextureInsideRT = MergeTargets; async readbacks for brush canvas.|**1x**: SD 870 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600 / RTX 3050|Forces **multi-gener** **ation** **Render** **Target** **caching** (keeping 3+ recent RTs alive in GSTextureC ache).|



|**Tekken 5** (SLUS-21 059)|**GSTex** **in RT +** **60 FPS** **Latency**|60 FPS fighter using custom stage floor reflections, heat distortion, and PSMCT24/ PSMT8H packed textures.|AutoFlush/Tex in RTon specific stages; minimal input latency (<1 frame).|**1x**: SD 865 / Zen 2 **2x**: SD 8 Gen 1 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Forces **accurate** **PSMT8H/4** **HL/4HH** **extraction** from the upper 8 bits of 24-bit color/depth buffers.|
|---|---|---|---|---|---|
|||||**1x**: SD 888 / Zen 2||
|**Ace** **Combat** **04, 5,** **Zero** (SLUS-20 851+)|**GS** **Clouds/** **Terrain** **Mipmap** **s +** **Readba** **cks**|Hardware mipmapped terrain tiles, volumetric cloud alpha sprites, sun glare readbacks, and interlaced HUD overlays.|Mipmapping = Full;AutoFlush/ CpuSpriteRender for clouds and sun glare.|**2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X / RTX 3060|Forces **per-draw** **CpuSpriteR** **ender** **fallback**or compute-as sisted 2D sprite blitting alongside hardware 3D.|



|**GTA: San** **Andreas** (SLUS-20 946)|**EE** **Streami** **ng +** **CDVD +** **GS** **Radiosit** **y**|RenderWar e open-world streaming; heavy EE draw-call submission; multi-pass orange sun radiosity and heat haze trails.|MTVUOn; AutoFlushor Half-Pixel Offset for radiosity ghosting fix.|**1x**: SD 870 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600 / RTX 3050|Forces **asynchron** **ous** **backgroun** **d CDVD** **sector** **prefetching** so disc I/O never stalls the IOP/EE threads.|
|---|---|---|---|---|---|
|**Silent** **Hill 2 & 3** (SLUS-20 228/ 20622)|**GS** **Noise** **Filter +** **Depth-of** **-Field +** **Fog**|Per-pixel dynamic noise grain overlay, complex multi-layer alpha fog, flashlight shadow projection, and FMV cutscenes.|Depth Emulation; accurate GS Fog andPSMCT24 depth reads.|**1x**: SD 865 / Zen 2 **2x**: SD 8 Gen 1 / Apple M1 **3x**: Ryzen 5 3600 / GTX 1650|Forces **exact** **per-vertex** **GSFOG** **interpolatio** **n**in the hardware vertex/frag ment shader pipeline.|



|**Dragon** **Quest** **VIII &** **Rogue** **Galaxy** (Level-5)|**GS** **Cel-Sha** **ding +** **Tex in** **RT +** **VU1**|Cel-shaded outlines, massive draw distances, dynamic clouds, and Menu/Battle transition framebuffer captures.|MTVUOn; TextureInsideRT; PreloadFrameWit hGSDatafor battle swirls.|**1x**: SD 870 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600 / RTX 3050|Forces **Local-to-Lo** **cal** **(TRXDIR=2** **) and** **Host-to-Lo** **cal** **(TRXDIR=0** **) VRAM** **transfer** **synchroniz** **ation**with cached GPU targets.|
|---|---|---|---|---|---|
|**Xenosag** **a** **Episode** **I, II, III** (SLUS-20 469+)|**IPU** **Multi-Str** **eam** **FMV +** **GS** **Shadow** **s**|Heavy 60 FPS MPEG-2 cutscenes with subtitle overlays; stencil/alph a character shadows that glitch if depth/alpha desyncs.|Threaded IPU; DATEand Depth Emulation fixes.|**1x**: SD 870 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600 / RTX 3050|Forces **zero-copy** **IPU-to-GIF** **DMA** **chaining** and accurate IPU_CTRL busy-flag timing.|



|**Need for** **Speed:** **Undergro** **und 2 /** **Most** **Wanted**|**GS** **Channel** **Shuffles** **+ EE** **Physics**|Extreme overbright bloom, motion blur, and color-tint Channel Shuffles; heavy EE traffic AI and reflection passes.|Channel Shuffle detection; EE Cycle Rate 100%–130%for smoother framerate.|**1x**: SD 888 / Zen 2 **2x**: SD 8 Gen 2 / Apple M1 **3x**: Ryzen 5 5600X / RTX 3060|Validates **multi-pass** **Channel** **Shuffle** **chaining** without intermediate GPU-to-CP U transfers.|
|---|---|---|---|---|---|



## **6. CPU and VU Emulation Performance Engineering EE (R5900) and IOP (R3000A) Dynamic Recompiler Architecture** 

Hitting the **4.2 ms EE/DMAC per-frame budget** on mid-range desktop hardware and **5.8 ms on flagship Android SoCs** requires native Dynamic Recompilers (JITs) for both the EE and IOP built on dynasm-rs targeting **x86-64** and **AArch64 (ARM64)** . In the block discovery phase, the JIT decodes guest MIPS instructions linearly starting from the current guest PC until it encounters a conditional branch (BEQ, BNE, BLTZ), jump (J, JAL, JR, JALR), system exception (SYSCALL, BREAK, ERET), or reaches a 64-instruction ceiling. Because the MIPS architecture enforces a **1-instruction branch delay slot** —alongside R5900 "Branch Likely" instructions such as BEQL and BNEL that conditionally nullify the delay-slot instruction if the branch is not taken—every branch compiles its trailing delay-slot instruction directly into the block before updating the guest PC. 

Before emitting host assembly via dynasm!, a fast pre-pass scans the 10 to 64 instructions in the basic block to compute register liveness bitmasks and required lane widths (32/64-bit scalar vs. 128-bit MMI/LQ/SQ). A dedicated callee-saved host register (RBP on x86-64; X29 or X28 on ARM64) is pinned to point directly to the #[repr(C, align(64))] struct EeCpuState. Placing the thirty-two 128-bit GPRs (512 bytes), LO/HI (32 bytes), PC (4 bytes), cycle_countdown (4 bytes), and the thirty-two COP1 FPRs (128 bytes) within the first 1,024 bytes of EeCpuState guarantees that every guest register spill or reload encodes as a single host instruction with a compact 8-bit or 12-bit signed displacement ([rbp + offset]), eliminating multi-instruction ADRP + ADD sequences on ARM64—an optimization proven in parallel-ps2 pass C.72. The register allocator assigns 8 to 10 callee-saved host GPRs (RBX, 

R12–R15, RSI, RDI, R8–R11 on x86-64; X19–X27 on ARM64) to hot 64-bit EE GPRs and 12 SIMD registers (XMM4–XMM15 / V16–V31) to 128-bit MMI registers and COP1 floats. At the end of each block, the generated code subtracts block_cycles from ee.cycle_countdown. When cycle_countdown > 0, static direct jumps within the same 4 KB page patch a direct host jmp rel32 or b imm26 to the target block's native entry address (Direct Block Linking), while indirect jumps (JR $ra) query a 64K-entry flat lookup table (fast_block_lut[(pc >> 2) & 0xFFFF]) inline in assembly without returning to Rust—an inline dispatch technique that reduced EE execution time by 18% in gow-pc-port. 

### **microVU Recompilation and MTVU Threading** 

Emulating VU1 via an interpreter destroys frame rates in every 3D PS2 game. A production VU JIT (microVU) must address three hardware requirements. First, when MSCAL or MSCNT triggers VU1 at micro-address vupc, the compiler traces the microprogram control-flow graph and performs **static pipeline stall and flag analysis** . For each 64-bit VLIW bundle, the analyzer tracks the completion cycle of every VF ( 𝑥, 𝑦, 𝑧, 𝑤 ), VI, Q (FDIV), and P (EFU) register. If bundle  reads a 𝑘 VF component written at cycle , the analyzer 𝑡 statically advances the virtual cycle counter by max (0, 𝑡+ 4 −𝑐𝑢𝑟𝑟𝑒𝑛𝑡_𝑐𝑦𝑐𝑙𝑒) . 

Simultaneously, it models the 4-stage MAC, Status, and Clipping flag shift registers; if subsequent bundles overwrite the flags before any FSAND, FMAND, or FCAND instruction reads an intermediate stage, the compiler elides the intermediate flag computation (mVU Flag Hack). Second, because VU Upper instructions heavily utilize broadcast swizzles (ADDx.xyzw, MULw.xyz) and arbitrary VF component unpacks, the ARM64 NEON emitter must avoid naive MOV + 4x INS lane insertions, emitting indexed NEON instructions (FMLA Vd.4S, Vn.4S, Vm.S[lane]) and single-instruction DUP, REV64, or EXT permutes (parallel-ps2 pass C.73), while the x86-64 emitter utilizes VSHUFPS and BLENDPS (SSE4.1/AVX). Third, when **MTVU** is active, the EE thread never executes VIF1 or VU1 directly. Instead, DMAC Channel 1 (VIF1) and EE writes to VIF1_FIFO enqueue commands (WriteMicroMem, WriteDataMem, VifUnpack, ExecuteMicro(addr), SyncPath1) into a 16 MB power-of-two #[repr(align(64))] SPSC ring buffer (MtvuRingBuffer). When the VU1 JIT executes XGKICK, the MTVU thread packs the geometry kick packet directly into the MTGS GIF PATH1 ring buffer, and the EE thread blocks (wait_for_vu1()) only if guest EE code explicitly polls COP2 VU1 registers (QMFC2/CFC2) or reads VU1 memory at 0x1100C000. 

### **Virtual Memory fastmem and the 16 KB Host Page Solution** 

Memory loads and stores (LW, LD, LQ, SW, SD, SQ) account for roughly 30% of all executed EE instructions. Checking software TLB tables on every access adds 6 to 10 host instructions per load or store. Instead, the emulator must implement **SIGSEGV-Backpatched fastmem** paired with PCSX2's **vtlb architecture** . At initialization, ps2-common reserves a 4 GB virtual memory region (fastmem_base) via mmap(NULL, 0x1_0000_0000, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0) on Linux, macOS, and Android, or VirtualAlloc2 and MapViewOfFile3 on Windows. The 32 MB physical RDRAM shared memory descriptor is mapped into fastmem_base + 0x00000000 (KUSEG), fastmem_base + 0x80000000 (KSEG0), fastmem_base + 0xA0000000 (KSEG1), and fastmem_base + 0x30100000 (UCAB), while the 16 KB Scratchpad is mapped at fastmem_base + 0x70000000. For every guest load or store, the JIT emits a single host memory instruction (mov eax, [r13 + rcx] 

where r13 = fastmem_base and rcx = zero_extended_guest_vaddr), padding the instruction slot with NOPs to a fixed 8-byte (x86-64) or 12-byte (ARM64) patchable window. 

All MMIO regions (0x10000000 EE registers and 0x12000000 GS Privileged registers) remain unmapped (PROT_NONE). When a JIT instruction accesses an MMIO register or writes to a RAM page containing compiled JIT blocks, the host CPU raises SIGSEGV / SIGBUS (POSIX) or EXCEPTION_ACCESS_VIOLATION (Windows). Crucially, as documented in parallel-ps2, Apple Silicon and modern Android kernels enforce **16 KB host pages** , whereas the PS2 TLB and Scratchpad operate on **4 KB pages** . Using mprotect(PROT_READ) on a 16 KB host page to trap self-modifying code (SMC) in a 4 KB guest code page causes writes to adjacent 4 KB data pages inside the same 16 KB host page to trap repeatedly. To solve this, when the fault handler catches a write or MMIO access at host_pc, it decodes the faulting host instruction and **backpatches the 8/12-byte slot in the JIT code cache** to jump to an out-of-line trampoline that indexes a 1𝑀 -entry array of 4 KB vtlb software pages and checks a 4 KB software SMC dirty bitmask. Subsequent executions of that instruction never trigger a kernel signal fault again. 

### **Realistic Hardware Specifications Across Device Classes** 

Based on verified PCSX2 v2.x, ARMSX2, and parallel-ps2 benchmarks on demanding titles ( _Shadow of the Colossus_ , _God of War II_ , _Gran Turismo 4_ , _Metal Gear Solid 3_ ), the table below defines realistic minimum and recommended hardware targets. 

|**Device** **Class**|**Minimum Target** **(1×1\times1× Native** **480p @ 50/60 FPS)**|**Recommended Target** **(2×–3×2\times–3\time** **s2×–3× 720p/1080p @** **Locked 60 FPS on** **Heavy Games)**|**Key Architectural** **Bottleneck on** **Tier**|
|---|---|---|---|
|**Desktop** **(Windows /** **Linux** **x86-64)**|Dual-core / 4-thread CPU with SSE4.1 & PassMark STR (Core ≥1, 600 i3-8100 / Ryzen 3 3100) + Vulkan 1.1 GPU (GTX 750 Ti / UHD 630).|6-core CPU with AVX2 & PassMark STR (Ryzen 5 ≥3, 000 5600X / 7600 or Core i5-12400) + Vulkan 1.3 / DX12 GPU (GTX 1660S / RTX 3060 / RX 6600).|PCIe readback latency on sun flares (GT4) and high-multiplier ( ) accurate 4 ×+ blending barriers.|
|**Apple** **Silicon** **(macOS** **ARM64)**|Apple M1 (8-core, 7-core GPU) easily exceeds native 1 × requirements across the entire library.|Apple M1 / M2 / M3 / M4 (sustains 3 × –6 × 1080p–4K @ 60 FPS using native ARM64|16 KB host virtual page granularity requiring software vtlb+ backpatch SMC handling.|



|||JIT + Metal / MoltenVK).||
|---|---|---|---|
|**Android** **(64-Bit** **ARM64)**|Snapdragon 855 / 865 (Adreno 640/650) with Vulkan 1.2+ at 1 × resolution and Basic Blending.|Snapdragon 8 Gen 2 / 8 Gen 3 / 8 Elite (Adreno 740/750/830 with Turnip or modern Qualcomm Vulkan drivers) at 2 × –3 × upscale.|Tile-based (TBDR) render pass breaks on recursive blending (DATE) and thermal throttling on sustained VU1 load.|
|**Low-Power** **ARM Linux** **SBCs**|Raspberry Pi 5 (BCM2712 4x Cortex-A76 @ 2.4 GHz) or RK3588: runs light/medium games at 60 FPS; heavy games require 75% EE Cycle Rate &MinBlending.|LattePanda Sigma / Ryzen Z1 / Steam Deck APU required for uncompromised 60 FPS at in_SotC_, 2 × _GT4_, and_God of War_ _II_.|Single-thread Cortex-A76 IPC ceiling on EE + microVU1and V3D/Mali Vulkan driver overhead.|



## **7. GS Rendering: The Largest Compatibility and Performance Lever** 

### **Translating 4 MB Unified eDRAM to Modern GPU APIs** 

Modern graphics APIs (Vulkan, Direct3D 12, Metal) enforce strict separation between sampled textures, color attachments, and depth-stencil attachments, storing each in opaque hardware-tiled layouts. The PS2 GS, by contrast, treats its 4 MB eDRAM as a unified array of 512 8 KB pages where a game can upload bytes via PATH3, render 3D polygons into Page 0..63 as PSMCT32, reinterpret Page 32..63 as a 16-bit Z-buffer (PSMZ16), and sample Page 16 as an 8-bit PSMT8 texture within the same millisecond. 

To bridge this impedance mismatch without round-tripping 4 MB of memory between the CPU and GPU on every draw call, the ps2-gs crate must implement a **Dual-State Memory Architecture** modeled on PCSX2's GSTextureCache. First, the renderer maintains a **CPU-side 4 MB swizzled eDRAM mirror (GsLocalMemory)** in aligned host RAM. All HOST_TO_LOCAL (TRXDIR=0) GIF image uploads write into this 4 MB buffer first and mark the affected 8 KB pages (0..511) and 256-byte blocks (0..16383) as **CPU-Dirty** in a BitSet512 page tracker. Second, the renderer maintains a **GPU-side page-indexed Texture Cache (GsTextureCache)** split into a **Source Pool** (sampled textures keyed by (base_block_bp, width_bw, psm, region_rect)) and a **Target Pool** (color and depth render 

targets keyed by (fbp_page, fbw, psm) and allocated at upscale_multiplier * (width, height)). When a draw call binds TEX0, the cache queries the Source Pool; if a matching texture exists and none of its spanned 8 KB pages are marked dirty, it binds immediately. If pages are CPU-dirty, ps2-gs unswizzles those pages from GsLocalMemory using AVX2/NEON swizzle lookup tables (or a GPU compute unswizzle shader), computes an **XXH3-64 hash** across the 256-byte blocks for HD texture replacement lookup, and uploads only the dirty sub-rectangle. Conversely, when a draw renders into a Target Pool attachment, its spanned 8 KB pages are marked **GPU-Dirty** , invalidating or triggering Tex in RT target-to-source blits on overlapping Source Pool textures. 

### **Solving the Four Hardest Hardware Rendering Bottlenecks** 

First, to prevent **Channel Shuffles** from causing catastrophic render-pass thrashing, GsTextureCache must pre-detect when a game binds a render target page (PSMCT32 / PSMCT24 / PSMZ24) as an 8-bit (PSMT8 / PSMT8H) or 16-bit (PSMCT16) texture (TEX0.TBP0 overlapping FRAME.FBP) and issues a sequence of vertical strip SPRITE primitives (typically 8 or 16 pixels wide with matching horizontal U and X step offsets). Instead of unswizzling the 32-bit target to CPU RAM or executing per-strip copies, the renderer batches the strip sprites into a single fullscreen quad draw, classifies the shuffle permutation from the UV coordinates, FRAME.FBMASK, and TEX0.PSM vs. FRAME.PSM, and binds a specialized **Channel Shuffle Fragment Shader** that samples the upscaled 32-bit RGBA8 attachment directly while applying sub-pixel draw offsets in the shader—mirroring PCSX2 v2.8's optimization. 

Second, to emulate **PS2 Alpha Blending (((A - B) * C >> 7) + D), DATE, and COLCLIP** at high speed, ps2-gs splits draws into two paths. For the ~85% of draws where 𝐴, 𝐵, 𝐷 reference at most one instance of destination color 𝐶𝑑 and  is 𝐶 α𝑠 or FIX, the renderer uses **Hardware Dual-Source Blending** (wgpu::Features::DUAL_SOURCE_BLENDING / Vulkan dualSrcBlend), outputting the scaled color term in @location(0) @blend_src(0) and the doubled alpha blend factor ( α𝑠 × 128.0<sup>255.0</sup> ~~)~~ in @location(0) @blend_src(1) with zero pipeline barriers. For the remaining ~15% of draws requiring recursive destination reads—such as (𝐶𝑠 −𝐶𝑑) × α𝑑 + 𝐶𝑑 , bitwise FBMASK masking, modulo-256 wrap-around when COLCLIP=0, or simultaneous destination alpha test-and-write under DATE=1—the renderer utilizes native tile memory reads ( **Metal Programmable Blending** on Apple Silicon and **VK_EXT_rasterization_order_attachment_access** on Android Adreno/Mali) or **VK_EXT_fragment_shader_interlock / D3D12 ROV** on desktop GPUs, emitting a pipeline barrier only when consecutive primitive bounding boxes overlap (SpriteNoGaps hazard tracking). 

Third, to eliminate **upscaling seams and interlacing artifacts** , the vertex shader implements PCSX2's **Half-Pixel Offset** (Normal Vertex, Special Texture, Align to Native) and **Round Sprite Offset** heuristics to bridge 12.4 fixed-point pixel-center (STQ) vs. top-left corner (UV sprite) conventions, while the presentation pass provides GPU **Weave** , **Bob** , and **Motion-Adaptive (MAD)** deinterlacing shaders matched to SMODE2 alongside 

GameIndex.yaml no-interlacing patches. Fourth, to eliminate **shader compilation stutter** across the > 100, 000 combinatorial GS state permutations, ps2-gs implements a **Two-Tier Ubershader and Asynchronous Pipeline Cache** : at startup, the renderer compiles 8 to 16 

Ubershaders that evaluate dynamic GS states via a 64-byte Push Constant block. When the MTGS thread encounters a uncached 64-bit GsPipelineKey, it draws immediately using the matching Ubershader while a background rayon thread pool compiles the specialized branch-free pipeline via VK_EXT_graphics_pipeline_library and saves it to a versioned disk cache (shader_cache.bin). 

### **GS Dump (.gs) Format and Standalone Debugging Tooling** 

To debug rendering bugs in seconds via RenderDoc without booting full games, ps2-gs-runner parses PCSX2's standard .gs (and compressed .gs.xz / .gs.zst) binary stream format, structured as follows: 

|**Binary** **Section /** **Opcode**|**Byte Layout & Payload** **Fields**|**Replay Action in ps2-gs-runner**|
|---|---|---|
|**File Header**|u32 crc,u32 state_size,u32 state_version, serialized internal GS registers, **4,194,304 bytes (4 MB** **eDRAM)**, and**8,192 bytes** **(Privileged Regs)**.|InitializesGsLocalMemory (marking all 512 pages CPU-dirty), restoresContext 1/2registers, and populates0x12000000privileged registers.|
|**Opcode** **0x00: ** **Transfer**|u8 path(0 = PATH1,1 = PATH2,2 = PATH3),u32 size_in_bytes, followed by[u8; size_in_bytes]raw GIF payload.|Routes the payload through the specified GIF path parser, updating GsLocalMemoryonIMAGE transfers or kicking vertices to the HW/SW rasterizer.|
|**Opcode** **0x01: VSync**|u8 field(0 = Even,1 = Odd).|Flushes pending GPU render passes, runs PCRTCDISPFB1/2 circuit composition and deinterlacing, and dumps the output frame PNG for CI diffing.|
|**Opcode** **0x02: ** **ReadFIFO2**|u32 size_in_qwords( ). 16𝑏𝑦𝑡𝑒𝑠× 𝑁|Executes aLOCAL_TO_HOST (TRXDIR=1) framebuffer readback from the activeGsTextureCache target intoGsLocalMemory.|



|**Opcode** **0x03: ** **Registers**|[u8; 8192]raw privileged register snapshot.|UpdatesPMODE,SMODE2, DISPFB1/2,DISPLAY1/2, BGCOLOR, andCSR.|
|---|---|---|



## **8. Audio, Input, UI/UX Teardown, and Feature-Parity Blueprint** 

### **SPU2 Emulation, Time-Stretching Audio Sync, and Low-Latency Input** 

The **SPU2** executes at a fixed sample rate of **48,000 Hz** ( 768 IOP clock cycles per stereo sample). The emulator ticks SPU2 voice decoding, ADSR envelopes, and reverb in batches on the IOP schedule—or catches it up lazily down to single-sample granularity whenever the IOP accesses 0x1F900000 or when Voice Loop/IRQ addresses (IRQINFO) are armed—and pushes 48 kHz float32 stereo frames into a lock-free ring buffer consumed by cpal / SDL3_AudioStream on desktop or oboe (AAudio) on Android. As proven in gow-pc-port v0.4.0, locking a 60 FPS emulator to a fixed 144 Hz monitor refresh timer causes clock drift where only 46 to 54 unique GS frames reach the screen per second. Instead, the main loop must pace directly to the guest PCRTC VBlank cadence (59.94 Hz NTSC / 50.00 Hz PAL) while routing SPU2 audio through a **Dynamic Rate Control (DRC) and Pitch-Preserving Time-Stretcher** (modeled on SoundTouch in PCSX2 and ps2-native-recomp). At 100% emulation speed, DRC adjusts the resampling ratio by $\pm 0.2\%$ to keep the audio buffer at 50% capacity (30–40 ms latency); if a heavy scene dips to 85% speed, the time-stretcher expands the audio smoothly without pitch drop or buffer-underrun clicks. 

For controller input, the PS2 DualShock 2 (SIO2 command 0x42 after config 0x4F) returns **12 analog 8-bit pressure bytes** (0x00–0xFF) for the D-Pad, $\triangle, \bigcirc, \times, \square$, and 𝐿1, 𝑅1, 𝐿2, 𝑅2 buttons alongside the 2-byte digital bitmask and 4 analog stick axes. Because titles such as _Metal Gear Solid 2/3_ (raising vs. firing a rifle) and _Gran Turismo 4_ (throttle modulation) rely on analog face buttons, the input subsystem must map modern analog triggers ( 𝐿2/𝑅2 ) directly to pressure bytes and provide a configurable **"Pressure Modifier" button and ramp curve** for digital gamepads. Finally, while **Run-Ahead** latency reduction is effective on 8/16-bit consoles, it is **infeasible on the PS2** : serializing and rolling back **42 MB of raw state** (32 MB EE RAM + 4 MB GS eDRAM + 2 MB IOP RAM + 2 MB SPU2 RAM + GPU render targets) every frame consumes > 6𝑚𝑠 , breaking the 16.67 ms budget. Instead, input latency is minimized to < 1𝑓𝑟𝑎𝑚𝑒 by **delaying host gamepad polling (SDL_PollEvents) until the exact microsecond the IOP triggers the SIO2 VBlank pad transfer** (typically 80–90% into the frame) rather than polling at frame start. 

### **PCSX2 v2.x UI/UX Teardown and Feature-Parity Blueprint (Deliverable 5)** 

PCSX2 v2.x (and DuckStation, whose Qt/ImGui architecture inspired PCSX2's modern redesign) achieves its polished user experience by separating **Out-of-Game Library Management** (Game Grid/List, Cover Art, Global Settings vs. Per-Game Properties override hierarchy) from an **In-Game Hardware-Accelerated Overlay (Big Picture / OSD)** . Compared against Play! (which offers a minimalist cross-platform UI), AetherSX2/NetherSX2 (which adapt the per-game settings model to Android Material Drawer layouts), and RetroArch (LRPS2, which buries PS2-specific switches inside generic Libretro Quick 

Menus), PCSX2's three-tier configuration cascade (GameIndex.yaml automatic defaults → Global Settings → Per-Game Overrides) is the gold standard to replicate. 

|**Feature** **Category &** **Name**|**What PCSX2 v2.x** **Does**|**User** **Valu** **e**|**Engine** **ering** **Compl** **exity**|**Targ** **et** **Pha** **se**|**Recommende** **d Rust Crates** **&** **Implementatio** **n Blueprint**|
|---|---|---|---|---|---|
|**Game** **Library Grid** **/ List &** **Cover** **Scraper**|Scans.iso/.chd/.cso directories, extracts serial (SLUS-xxxxx) & CRC, displays cover art grid or sortable table with compatibility stars.|**Criti** **cal**|Low–M edium|**MVP** **(Pha** **se** **3)**|walkdir,chd, image, egui_extras::Ta bleBuilder. Async cover downloader fetching by serial from ps2-covers repo.|
|**Hierarchical** **Settings** **(Global vs.** **Per-Game +** **GameIndex.** **yaml)**|Three-tier cascade: GameIndex.yaml automatic fixes → GlobalPCSX2.ini → Per-Game SLUS-xxxxx.iniUI overrides.|**Criti** **cal**|Mediu m|**MVP** **(Pha** **se** **4)**|serde_ymlfor GameIndex.ya ml;tomlfor global & per-game configs; Option<T> fields in PerGameConfi gsoNone inherits Global.|
|**In-Game** **OSD &** **Performanc** **e Graphs**|Renders real-time FPS, GS internal FPS, EE/VU/GS % load counters, frame-time graph, and toast notifications (unsafe_settings_warni ng).|**Criti** **cal**|Low|**MVP** **(Pha** **se** **3)**|egui-wgpu/ Vulkan overlay pass rendered on top of the presented PCRTC texture; ring buffer of|



||||||last 240 frame times ( ). 𝑃50/𝑃99|
|---|---|---|---|---|---|
|**Big Picture** **/ Pause** **Menu** **Overlay**|Controller-navigable fullscreen UI for switching games, toggling cheats/patches, and managing 10 save-state slots with thumbnails.|**High**|Mediu m|**Pha** **se 5**|Fullscreenegui controller-navig able state machine triggered by Guidebutton or Esc.|
|**Save States** **(Versioned** **+** **Compresse** **d +** **Thumbnails** **)**|Saves EE/IOP/VU/GS/SPU2 state + 4 MB eDRAM + screenshot into a.p2s archive (10 slots per game + backup slot).|**Criti** **cal**|Mediu m|**MVP** **(Pha** **se** **4)**|bincode+ lz4_flex (compresses 42 MB raw state to ~6–10 MB in<25 ms) with magic header, schema version, and 320 × 224 WebP/PNG preview.|
|**Memory** **Card** **Manager (8** **MB.ps2 &** **Folder** **Cards)**|Supports raw 8 MB MagicGate.ps2images and Folder Memory Cards (exposing individual game saves as host directories on disk).|**High**|Mediu m|**Raw** **:** **MVP** **(Pha** **se** **3)** **Fold** **er:** **Pha** **se 5**|Raw 8 MB file withmemmap2 + ECC spare calculation for MVP; virtual FAT table synthesizer for Folder Memcards in Phase 5.|



|**Cheats,** **Widescreen** **&** **No-Interlac** **e Patches** **(.pnach)**|Parses.pnachfiles (patch=1,EE,address,e xtended,value) and embedded GameIndex.yaml patches, applying them per VBlank or at ELF boot.|**High**|Low|**MVP** **(Pha** **se** **4)**|Custom 150-line.pnach parser supporting byte,short, word,double, and multi-line pointer/conditio nal codes (0xE, 0xD,0x2).|
|---|---|---|---|---|---|
|**GS Dump** **Recorder &** **Standalone** **Runner** **(.gs.zst)**|Hotkey (Shift+F8) records single-frame or multi-frame GIF/VRAM stream to disk for regression testing and bug reports.|**Criti** **cal** **(Dev** **Velo** **city)**|Low|**MVP** **(Pha** **se** **2)**|Built into ps2-gsand ps2-gs-runner CLI usingzstd compression.|
|**Texture** **Dumping &** **HD Texture** **Replaceme** **nt**|Hashes textures + CLUTs via XXH3-64 and swaps in high-res .dds/.pngpacks asynchronously from textures/<serial>/replac ements/.|**Medi** **um–** **High**|Mediu m|**Pha** **se 5**|xxhash-rust (xxh3_64on 256-byte GS blocks),ddsfile (BC1–BC7 GPU compressed textures), background rayonloader.|
|**Post-Proce** **ssing** **Shader** **Pipeline** **(FXAA,** **CAS, CRT)**|Applies contrast-adaptive sharpening (CAS), FXAA, TV color-space shaders, and Reshade-style CRT scanlines after upscaling.|**Medi** **um**|Low|**Pha** **se 5**|Post-process WGSL/SPIR-V compute/fragm ent pass chain executing betweenMTGS target resolve and swapchain present.|



|**Integrated** **Debugger** **(EE, IOP,** **VU0/1,** **Memory,** **GS)**|Multi-pane debugger with MIPS/MMI/VU VLIW disassemblers, register/memory hex editors, breakpoints, symbol map (.sym) loader, and thread view.|**Criti** **cal** **(Dev** **)**/ **Low** **(End** **User** **)**|Mediu m|**MVP** **(Pha** **se** **1–2)**|Built inegui from Day 1 to debug BIOS/commerci al games; includes live VU1 microcode stepper and GS VRAM page viewer.|
|---|---|---|---|---|---|
|**RetroAchie** **vements** **(rcheevos) ** **Integration**|Logs into RetroAchievements, hashes disc executable, monitors EE RAM conditions every VBlank, and displays unlock popups + Rich Presence.|**Medi** **um–** **High**|Low–M edium|**Pha** **se 5**|FFI bindings to official rcheevosC library + reqwestHTTPS client and OSD achievement badge notifications.|
|**Controller** **Profiles,** **Pressure** **Curves &** **Multitap**|Per-port DualShock 2 / Guitar / GunCon 2 profiles, SDL GameControllerDB auto-mapping, deadzone/pressure curves, and 8-player Multitap.|**High**|Low–M edium|**Pad:** **MVP** **(Pha** **se** **3)** **Mult** **itap/** **USB** **:** **Pha** **se 6**|sdl3-sys (Gamepad API with rumble & gyro) on Desktop; Android InputDevice+ customizable eguitouch overlay on Mobile.|
|**Network** **Adapter** **(DEV9** **SMAP**|Emulates Sony Network Adapter via PCAP / TAP / Sockets (for DNS-revived online servers like_GT4 LAN_,|**Low** **–Me**|High|**Pha** **se 6**|User-mode UDP/TCP NAT translation layer (avoiding raw pcapprivilege|



|**Ethernet &**|_Resident Evil Outbreak_,|**diu**|**(Def**|crashes) +|
|---|---|---|---|---|
|**HDD)**|_MGS3 Subsistence_)|**m**|**er)**|sparse HDD file|
||and 40 GB HDD.|||image.|



## **9. Cross-Platform Strategy, Build Pipeline, and Legal Considerations** 

### **Realistic Platform Targets vs. Technical Dead Ends** 

A disciplined cross-platform strategy divides targets into three tiers based on operating system support for dynamic code generation and virtual memory management. The **first-class shipping targets** are **Windows 10/11 (x86-64 and ARM64)** , **Linux (x86-64 and ARM64, including Steam Deck and ARM SBCs)** , **macOS 13+ (Apple Silicon M1–M4)** , and **Android 10+ (64-bit ARM64 with Vulkan 1.2+)** . All four operating systems permit runtime W^X or dual-mapped JIT compilation, custom POSIX or Win32 page-fault handlers (SIGSEGV / VEH) for 4 GB fastmem, and low-overhead Vulkan, Metal, or Direct3D 12 rendering. 

Conversely, **iOS and iPadOS represent a sideload-only target** . Although Apple updated **App Store Review Guideline 4.7** in 2024 to permit retro game console emulators on the App Store, **Apple continues to prohibit third-party App Store apps from requesting the com.apple.security.cs.allow-jit (MAP_JIT) entitlement** under its code execution security rules. While 8-bit through 32-bit emulators such as Delta run comfortably on AOT interpreters on modern iPhones, **a PS2 emulator cannot run heavy 3D games without a JIT** . Consequently, iOS should be targeted strictly as a sideloaded .ipa package distributed via AltStore, SideStore, or LiveContainer paired with JIT-enabling attach scripts (utmjit.js), matching the deployment model of iPSX2. Finally, **WebAssembly (Browser) must be cut entirely** as unrealistic for heavy PS2 emulation: Wasm's Harvard sandbox requires compiling brand-new Wasm modules via browser APIs whenever VU1 microcode or EE SMC changes, lacks SIGSEGV virtual-memory fault backpatching for 4 GB fastmem, and imposes severe SharedArrayBuffer thread synchronization overhead. 

### **Legal Precedents, BIOS Handling, and Open-Source Licensing** 

Under the landmark Ninth Circuit ruling in **_Sony Computer Entertainment, Inc. v. Connectix Corp._ , 203 F.3d 596 (9th Cir. 2000)** (building upon _Sega Enterprises Ltd. v. Accolade, Inc._ , 977 F.2d 1510), observing, disassembling, and making intermediate copies of Sony's copyrighted BIOS during reverse engineering to understand unprotected functional hardware specifications is **protected Fair Use under 17 U.S.C. § 107** , provided the final distributed emulator contains **zero copyrighted Sony BIOS code** . To maintain strict legal compliance across GitHub, Google Play, Flathub, and Microsoft Store distributions, the repository, CI runners, and installers must never bundle a PS2 BIOS (scph10000.bin, etc.), Sony SDK .IRX modules, or commercial game images. Instead, the application's First-Time Setup Wizard must require the user to supply their own console BIOS dump and game ISOs, validating BIOS dumps via SHA-256 checksums. 

Regarding open-source licensing, **PCSX2, DobieStation, ARMSX2, and parallel-ps2 are licensed under GPLv3 (and LGPL)** , whereas **Play!** (jpd002/Play-) is licensed under the permissive **BSD-2-Clause** license and **ps2autotests** is licensed under the permissive **ISC** license. Licensing the new Rust emulator under **GPLv3 is strongly recommended for maximum engineering velocity** : it allows the developer to legally port algorithms, lookup tables, ps2float.c logic, microVU flag heuristics, and GS texture-cache valid-rect math, as well as bundle PCSX2's GameIndex.yaml database directly inside the application with proper attribution. GPLv3 is fully compatible with Google Play, Flathub, Windows/macOS direct downloads, and sideloaded iOS .ipa distribution. If the developer instead selects a permissive license (MIT, Apache-2.0, or MPL-2.0), strict clean-room rules apply: no code may be translated or derived from PCSX2, ARMSX2, or DobieStation, all subsystems must be implemented solely from hardware documentation (ps2tek, Sony Linux Kit manuals), ps2autotests (ISC), and Play! (BSD-2-Clause), and GameIndex.yaml must be fetched externally at runtime by the user. 

## **10. Phased Engineering Roadmap and Performance Gates** 

Building a PS2 emulator from scratch without stalling across its ten interdependent hardware subsystems requires a sequential 6-phase roadmap over a realistic 36-month solo-developer timeline, where every phase closes with a concrete **Definition of Done (DoD)** and a quantitative **Performance Gate** . 

|**Phase & Solo** **Dev Effort**|**Subsystems &** **Deliverables Built**|**Functional** **Definition of** **Done (DoD)**|**Mandatory** **Performance &** **Quality Gate**|
|---|---|---|---|
|**Phase 1: Core** **Foundation &** **CPU/DMA** **Test Suites** _(Months 1–4)_|ps2-common,ps2-core skeleton: Event-driven Scheduler, physical/virtual memory map (vtlb), R5900 interpreter (base + MMI + COP0 + COP1 with ps2float), R3000A IOP interpreter, 10-channel DMAC, SIF0/1, and ps2-autotestCLI runner.|Boots 4 MB Sony BIOS through hardware initialization; executes standalone.elf and.irxbinaries headlessly.|**Pass** ≥52/55 **ps2autotests** **suites**(EE ALU, FPU, MMI, DMAC, IOP, Timers) with byte-identical stdout in CI < 10𝑠 run.|



**Phase 2:** VU0 (Macro/Micro) & VU1 Renders the **ps2-gs-runner** interpreters, VIF0/VIF1 Sony BIOS **replays 25 Vector Units,** packet decompressors "Towers" intro **reference .gs.gs GIF, SW GS &** (UNPACK), 3-path GIF and OSDSYSOSDSYS **dumps** ( _SotC_ , **Dump Runner** arbiter (PATH3 masking), memory card _GoW2_ , _MGS3_ , 4 MB swizzled eDRAM browser; _GT4_ , _Champions_ ) table, reference CPU renders ps2sdkps2sdk with $>99.9\%$ software rasterizer, 3D cube/teapot pixel match against ps2-gs-runner, and egui homebrew PCSX2 SW golden _(Months 5–9)_ debugger. demos. PNGs. Boots **Phase 3:** ps2-cdvd commercial **60.0 FPS at** 1 × **Peripherals,** (ISO/BIN/CSO/CHD), DVD/CD **Native (SW SPU2, CDVD** SIO2 (PADMAN **& EE/IOP JITs** DualShock 2 + MCMAN 8 games into 3D **Renderer)** on gameplay with Desktop in 2D/light MB cards), ps2-spu2 (48 working 3D games voices + time-stretching), controller input, ( _Disgaea_ , _Capcom_ IPU MPEG-2 decoder, memory card _vs. SNK 2_ , _Final_ and **x86-64 dynasm-rs** saving, SPU2 _Fantasy X (Months_ **JITs for EE and IOP** + audio, and FMV menus/FMVs). _10–15)_ fastmem. cutscenes. **Locked 60.0 FPS ( Phase 4:** x86-64 microVU0/1 JIT 𝑃99 < 16. 67𝑚𝑠 **) at microVU JIT,** (with static stall/flag pass), Heavy **MTVU, MTGS** MTVU & MTGS worker commercial 3D 3 × **(1080p)** on **& HW** threads, wgpu/Vulkan HW library runs Ryzen 5 5600X + **Renderer** renderer, 8 KB Page without major RTX 3060 across **all 10 heaviest** Texture Cache, graphical **games** ( _SotC_ w/ Dual-Source Blending, corruption at Channel Shuffle detector, 1 × –4 × +150% EE, _GoW2_ , _GT4_ , _MGS3_ , _Jak II_ , Depth Emulation, and internal _(Months_ GameIndex.yaml resolution. _R&C 3_ , _Champions of Norrath_ , _VP2_ , _16–24)_ ingestion. _ZOE2_ , _Burnout 3_ ). 

Renders the **ps2-gs-runner** Sony BIOS **replays 25** "Towers" intro **reference .gs.gs** and OSDSYSOSDSYS **dumps** ( _SotC_ , memory card _GoW2_ , _MGS3_ , browser; _GT4_ , _Champions_ ) renders ps2sdkps2sdk with $>99.9\%$ 3D cube/teapot pixel match against homebrew PCSX2 SW golden demos. PNGs. 

|**Phase 5:** **PCSX2-Parity** **UI, Save** **States &** **Polish** _(Months_ _25–30)_|Game Library Grid + Cover Art scraper, Global/Per-Game Settings UI, LZ4 Save States, .pnachcheat/widescreen loader, HD Texture Replacement (XXH3-64), Ubershaders + async pipeline cache, and RetroAchievements.|Feature-comple te desktop release matching PCSX2 UX workflows; zero shader-compila tion hitches on cached/ubersh ader runs.|**Save/Load State** **completes in** ; UI cold < 25𝑚𝑠 launch in ; < 500𝑚𝑠 zero frame drops below 58 FPS during 30-minute heavy-game soak tests.|
|---|---|---|---|
|**Phase 6:** **ARM64 JITs,** **Apple Silicon,** **Android &** **SBCs** _(Months_ _31–36)_|AArch64dynasm-rs backends for EE, IOP, and aVU(microVU), 16 KB host pageSIGSEGV backpatching, Metal/Vulkan TBDR Framebuffer Fetch blending, and Android Kotlin/Compose frontend.|Native ARM64 builds running on macOS Apple Silicon, Android 10+, and Linux ARM64 SBCs.|**60.0 FPS at** 3 × **on Apple M2/M3;** **60.0 FPS at** 2 × **(720p) on** **Snapdragon 8** **Gen 2/3**; playable 50–60 FPS at 1 × (75% EE) on Raspberry Pi 5.|



## **11. Performance Plan, Compatibility Plan, Pitfalls, and Curated Resources** 

### **Performance Plan and Deterministic Benchmarking Methodology (Deliverable 7)** 

Measuring and optimizing emulator throughput requires integrating symbol-aware CPU/JIT profilers, frame-time telemetry, and GPU trace analyzers from Phase 1. Because dynamically generated machine code resides in anonymous mmap pages, the ps2-jit crate must emit a standard /tmp/perf-<pid>.map symbol file mapping [host_code_addr, size, "EE_block_0x001045A0"] and "mVU1_prog_0x12_block_0x0080". This allows Linux perf (perf record -g --call-graph dwarf), cargo-flamegraph, and hotspot to attribute CPU cycles directly to individual guest EE and VU1 basic blocks—a technique proven in parallel-ps2 via LRPS2_PROF and LRPS2_DUMP_HOST_MVU. For real-time thread contention analysis, integrating tracy-client (Tracy profiler) with instrumentation zones around ee_step_batch(), mtvu_wait_sync(), gif_transfer_path1/2/3(), gs_texture_cache_lookup(), and gpu_submit() exposes exact microsecond stalls when Thread 1 waits on MTVU (SyncStats) or when MTGS stalls on a synchronous CPU readback. For GPU bottleneck analysis, ps2-gs-runner embeds the renderdoc API for programmatic single-frame capture alongside **NVIDIA Nsight** 

**Graphics** , **AMD Radeon GPU Profiler (RGP)** , **Xcode Metal System Trace** , and **Qualcomm Snapdragon Profiler** . 

Crucially, **benchmarks must never be recorded from a cold disc boot** . As documented during the ARM64 optimization of parallel-ps2, running _Gran Turismo 3_ headlessly from boot for 9,000 frames (150 seconds of emulated time) with Start auto-pressed still lands inside the opening FMV movie, putting IPUWorker and yuv2rgb at 14% of total CPU time while leaving the MTVU thread idle at 0.2% CPU—sending two optimization efforts down blind alleys before engineers switched to an **in-race save state** . Instead, automated performance regression testing must load deterministic in-game save states (.p2s), replay a 600-frame recorded SIO2 input trace, and report **Wall-Clock Speed %** , 𝑃50 **(Median) Frame Time** 

**(ms)** , 𝑃99 **Frame Time (ms)** , 𝑃99.9 **Worst-Spike Frame Time (ms)** , MTVU wait time (SyncStats), and GPU Render Pass / Barrier counts per frame. 

|**Benchmark Save State ID**|**Target Game** **& Scene**|**Primary** **Subsystem** **Stressed**|**Target Metrics** **on Reference** **Desktop** **(3×3\times3×** **1080p)**|
|---|---|---|---|
|**bench_sotc_gaius**|_Shadow of_ _the Colossus_ — Colossus #3 sword-jump fur & physics|EE R5900 JIT (+150%Cycle Rate),microVU1 IK/fur, GS stencil-alpha shadows|, 𝑃50≤11. 0𝑚𝑠 , 𝑃99≤15. 5𝑚𝑠 MTVUwait < 0. 5𝑚𝑠|
|**bench_gow2_colossus**|_God of War II_ — Colossus of Rhodes opening battle|microVU1block compilation, SIMD VIF1 UNPACK,Tex in RTbloom|, 𝑃50≤10. 2𝑚𝑠 , 𝑃99≤14. 0𝑚𝑠 GPU ≤12 render passes|
|**bench_gt4_nurburgring**|_Gran Turismo_ _4_— 6-car grid facing low sun in 1080i mode|Flat 150-block microVU1 throughput, sun lens-flare readback, PCRTC 1080i|, 𝑃50≤11. 4𝑚𝑠 , 𝑃99≤15. 8𝑚𝑠 SyncStats = 0. 00𝑠|



|**bench_mgs3_rassvet**|_Metal Gear_ _Solid 3_— Sokrovenno rain with thermal goggles|Channel Shuffle pre-detector,DATE barriers, Dual-Source Blending|, 𝑃50≤10. 8𝑚𝑠 , 𝑃99≤15. 0𝑚𝑠 CPU shuffle 0 copies|
|---|---|---|---|
|**bench_con_torch**|_Champions of_ _Norrath_— Multi-torch cave dungeon floor|GsTextureCache Fast Invalidation, GPU 8-bitPSMT8 CLUT conversion|, 𝑃50≤11. 2𝑚𝑠 , 𝑃99≤15. 5𝑚𝑠 CPU palette 0 decodes|



### **Compatibility Plan and Prioritized Accuracy Tiers (Deliverable 8)** 

Validation proceeds across four automated layers in continuous integration. First, **ps2-autotest** executes all 55 .elf and .irx test suites from unknownbrackets/ps2autotests (cpu/ee/alu, cpu/ee/fpu, cpu/ee/mmi, cpu/iop, cpu/gte, dmac, gs, intc, ipu, sif, spu2, timer, vu) on every commit, asserting byte-identical stdout against .expected reference files. Second, headless **GS register and VRAM swizzle unit tests** (modeled on gow-pc-port's ps2x_tests) verify GIF transfer formatting, BitSet512 page invalidation, and XXH3-64 hashing without requiring a GPU context. Third, a **differential lockstep mode (PS2_VU_DIFF=1)** executes every compiled microVU block alongside the reference VU interpreter in debug runs, asserting register-exact VF/VI/VRAM state across 12,000-frame traces (matching parallel-ps2's MVU_DIFF verification methodology). Fourth, **ps2-gs-runner** replays 100+ curated .gs dumps against golden reference PNGs to catch texture-cache or blending regressions immediately. 

|**Priority Tier**|**Hardware Behavior /** **Accuracy Feature**|**Compatibility** **Impact (% of** **Library** **Affected)**|**Implementation** **Strategy**|
|---|---|---|---|



|**P0** **(Mandatory** **for Boot &** **>85%** **Games)**|128-bit EE GPRs/MMI, 10-channel DMAC source/dest chains + TTE, 3-path GIF arbitration +PATH3 mask, VIFUNPACK+ double-buffering (TOP/TOPS), COP1/VU NaN/Infexponent clamping (0xFF), SPU2 ADPCM/IRQ, and LLE BIOS.|**85% – 90%**|Implement unconditionally in core interpreters and JITs.|
|---|---|---|---|
|**P1 (Required** **for 95%–99%** **& Heavy** **Games)**|Static VU FMAC/FDIV stall counting & 4-stage MAC/Status/Clip flag pipeline, GS 8 KB page Texture Cache (Tex in RT,PSMT8H/4HL extraction, Channel Shuffle detection), Dual-Source Blending + DATEbarriers, Depth Emulation, and GameIndex.yaml per-game patches.|**+8% – 12%** (Includes almost all AAA heavy games)|Implement in Phase 4 alongside GameIndex.yaml loader.|
|**P2 (Edge** **Cases for** **99.0%–99.5%** **)**|Bit-exactps2float truncation/FMA flags, MFIFO stall thresholds (D_STADR),CPU Sprite Renderfor interleaved 2D/3D draws, single-column GS invalidation on format mismatches, and exact HBlank DVD video counters.|**+1.0% – 1.5%** (_Star Ocean 3_, _Steambot_ _Chronicles_,_Area_ _51_,_Manhunt 2_)|Activate conditionally viaGameIndex.yaml per-game flags.|



Sub-instruction EE dual-issue pipeline stall simulation, EE Data Cache (8 KB) line **P3** locking (CACHE **(Cycle-Exact** opcode), bus contention **Research —** modeling between **<0.5%)** DMAC and EE cache fills, and exact CDVD physical rotational angular latency. 

**Defer Indefinitely** : **<0.5%** (Handful Emulating the 8 KB of obscure titles EE Data Cache and like _Marvel_ bus contention _Nemesis_ or globally reduces EE _World Series_ JIT speed by _Baseball 2K3_ ) 3 × –5 × for < 5 games. 

### **Pitfalls and Lessons Learned from Emulator Developers (Deliverable 9)** 

Historical post-mortems, GitHub issues, and release notes from PCSX2, DobieStation, parallel-ps2, and gow-pc-port highlight five critical engineering pitfalls to avoid. 

|**Pitfall /** **Failure Mode**|**Origin &** **Documented** **Case Study**|**Underlying** **Hardware** **Mechanism**|**Concrete Engineering** **Solution in Rust**|
|---|---|---|---|
|**HostNaN** **Propagation** **&MIN/MAX** **Bugs**|DobieStation Issue #51 & PCSX2Issue #1110|Guest games store valid PS2 floats with exponent 0xFFor uninitialized bit patterns; passing them to IEEE MINSS/MAXS SorADDSS yields quiet NaNs, causing invisible models or infinite fall-throughs.|Clamp exponent0xFFto 0x7F7FFFFF/0xFF7FFFFF before host float ops, and implementMIN/MAXusing integer signed-magnitude comparison (PMINSD/PMAXSD).|



|**Profiling** **Cold Boots** **Instead of** **In-Game** **States**|parallel-ps2 ARM64 Port Post-Mortem|9,000 frames from boot in _Gran Turismo_ _3_only reaches the opening FMV (IPUWorkerat 14% CPU, MTVUat 0.2%), misdirecting JIT optimization efforts.|Always capture and profile from deterministic in-game save states (bench_gt4_nurburgring).|
|---|---|---|---|
|**High-Refresh** **Monitor** **VSync** **Fighting 60** **FPS Limiter**|gow-pc-port v0.4.0 Release Notes|Running the window at 144 Hz while limiting the game to 60 FPS causes clock drift where only 46–54 unique GS frames reach the display per second despite a "60 FPS" readout.|Lock presentation pacing to guest PCRTC frame delivery (59.94 Hz), use VRR/FreeSync above 120 Hz, and display bothG-FPS (unique GS frames) and V-FPSin the OSD.|



|**Treating** **Channel** **Shuffles as** **Target** **Format Truth**|PCSX2GS/HW Commits by refractionpcsx2|When a game temporarily binds a 32-bit PSMCT32 framebuffer as 8-bitPSMT8 or 16-bit PSMCT16for a channel shuffle, mutating the Texture Cache's canonical format destroys 32-bit scene precision.|Never promote a detected shuffle draw to source-of-truth for target format; execute the 8/16-bit shuffle as an ephemeral view transformation over the 32-bit target.|
|---|---|---|---|
|**Hardcoding 4** **KB Host** **Pages invtlb** **/ SMC**|parallel-ps2& ARMSX2v2.4.9|Calling mprotect assuming 4 KB host pages fails or causes false-positive trap storms on 16 KB page kernels (Apple Silicon & Android 15+).|Query sysconf(_SC_PAGESIZE)at runtime, maintain 4 KB software dirty bitmasks, and useSIGSEGVinstruction backpatching.|



### **Curated Primary Resources and Reference Codebases (Deliverable 10)** 

The table below organizes the essential hardware manuals, community documentation, test suites, and exact source directories in reference emulators that should be studied before writing each subsystem. 

|**Resource /**|**URL / Exact File Paths to Read**|**Primary Engineering**|
|---|---|---|
|**Repository**|**First**|**Value**|



|**ps2tek (by** **PSI-Rockin)**|https://psi-rockin.github.io/ps2tek/|Consolidated hardware reference for EE R5900, COP0/1, VUs, VIF, GIF, GS swizzling, DMAC, IPU, IOP, SPU2, CDVD, and memory maps.|
|---|---|---|
|**DobieStation** **Architecture** **Guide**|https://github.com/PSI-Rockin/Dobi eStation/wiki/Making-a-PS2-Emulat or:-From-Bits-to-Pixels|Step-by-step walkthrough of the minimal hardware path required to go from BIOS reset vector to drawing pixels via DMAC Channel 2 (PATH3) and the GS.|
|**ps2autotests** **Suite**|https://github.com/unknownbracket s/ps2autotests(tests/cpu/, tests/dmac/,tests/gs/,tests/vu/)|55 hardware-verified.elf and.irxtest programs with .expectedstdout outputs for automated CI validation.|
|**PCSX2** **(PCSX2/pcsx2)**|pcsx2/vtlb.cpp, pcsx2/common/Linux/LnxHostSys. cpp, pcsx2/x86/ix86-32/iR5900*.cpp, pcsx2/x86/microVU*.inl, pcsx2/MTVU.cpp, pcsx2/GS/Renderers/HW/GSTextur eCache.cpp, bin/resources/GameIndex.yaml|Reference implementation forvtlbfastmem,microVU static flag analysis,MTVU ring buffers, 8 KB page texture caching, channel shuffles, and GameIndex.yaml.|
|**libretro/parallel-** **ps2**|https://github.com/libretro/parallel-p s2(pcsx2/ps2float.c, pcsx2/ps2float.h, ARM64aVU emitters)|Dependency-free C89 bit-exact model of the PS2 float pipeline (ps2float.c, verified across 31.9M cases) and native AArch64 EE/IOP/aVUJIT patterns.|



|**Play!** **(jpd002/Play-)**|https://github.com/jpd002/Play- (Source/ee/FpAddTruncate.cpp, Source/iop/)|CleanBSD-licensed reference for HLE BIOS/kernel syscalls and .IRXmodule behavior.|
|---|---|---|
|**DobieStation** **(PSI-Rockin/Dob** **ieStation)**|https://github.com/PSI-Rockin/Dobi eStation(src/core/ee/,src/core/gs/, src/core/iop/)|Clean, readable C++ state machines for EE, VU, VIF, GIF, GS, DMAC, IPU, and SPU2—ideal to read before diving into PCSX2's JIT code.|
|**gow-pc-port** **Docs**|https://github.com/danielalves96/go w-pc-port/blob/main/docs/dev/REN DERING.md& Releases|Detailed breakdown of XXH3-64GS 256-byte block hashing, 8 KB page caching, and 144 Hz VSync frame-pacing fixes.|



## **12. Starter Code Guidance: Workspace and Core Loop in Rust** 

### **Cargo Workspace Definition (Cargo.toml)** 

Ini, TOML 

None [workspace] resolver = "2" members = [ "crates/ps2-common", "crates/ps2-jit", "crates/ps2-cdvd", "crates/ps2-gs", "crates/ps2-spu2", "crates/ps2-core", "crates/ps2-gs-runner", "crates/ps2-autotest", "crates/ps2-ui-desktop", "crates/ps2-ui-android", ] 

[workspace.dependencies] bitflags = "2.6" bytemuck = { version = "1.19", features = ["derive", "min_const_generics"] } crossbeam-channel = "0.5" dynasm = "3.0" dynasmrt = "3.0" lz4_flex = "0.11" rustc-hash = "2.0" serde = { version = "1.0", features = ["derive"] } serde_yml = "0.0.12" thiserror = "2.0" tracing = "0.1" wgpu = "25.0" xxhash-rust = { version = "0.8", features = ["xxh3"] } [profile.release] opt-level = 3 lto = "fat" codegen-units = 1 panic = "abort" strip = "symbols" 

### **Core Traits, Hardware Structs, and Minimal EE Fetch-Decode-Execute Loop** 

The following starter implementation in crates/ps2-core/src/lib.rs establishes the cache-line-aligned EeCpuState (designed for direct JIT base-pointer indexing), the Scheduler event queue, the Bus with fastmem pointer access, the non-IEEE 754 ps2_clamp_f32_bits helper, and the event-driven EE execution loop. 

#### Rust 

None use std::collections::BinaryHeap; use std::cmp::Ordering; /// 128-bit Emotion Engine General Purpose / Vector Register. /// Aligned to 16 bytes for zero-cost SSE/AVX2 and ARM64 NEON loads/stores. #[repr(C, align(16))] #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)] 

pub union Reg128 { pub u128_val: u128, pub u64_lane: [u64; 2], pub i64_lane: [i64; 2], pub u32_lane: [u32; 4], pub i32_lane: [i32; 4], pub u16_lane: [u16; 8], pub u8_lane:  [u8; 16], pub f32_lane: [f32; 4], } 

/// Emotion Engine (MIPS R5900) Hot Architectural State. /// Pinned to RBP (x86-64) or X29 (AArch64) inside JIT blocks so all hot fields /// reside within a compact signed 8/12-bit displacement. #[repr(C, align(64))] pub struct EeCpuState { pub gpr: [Reg128; 32],       // Offset 0x000..0x200: 32 x 128-bit GPRs (gpr[0] is hardwired 0) pub hi: Reg128,              // Offset 0x200..0x210: Upper 64 bits = HI1, Lower 64 bits = HI0 pub lo: Reg128,              // Offset 0x210..0x220: Upper 64 bits = LO1, Lower 64 bits = LO0 pub fpr: [u32; 32],          // Offset 0x220..0x2A0: COP1 32-bit raw float registers (non-IEEE) pub fcr31: u32,              // Offset 0x2A0: COP1 Control/Status register (flags: I, U, O, Z, D) pub facc: u32,               // Offset 0x2A4: COP1 32-bit Multiply-Accumulate register (ACC) pub pc: u32,                 // Offset 0x2A8: Current Program Counter pub next_pc: u32,            // Offset 0x2AC: Branch target for delay-slot execution pub in_delay_slot: bool,     // Offset 0x2B0: True if currently executing a branch delay slot pub sa: u32,                 // Offset 0x2B4: Shift Amount register for MMI funnel shifts 

pub cycle_countdown: i32,    // Offset 0x2B8: Cycles remaining until next scheduled hardware event 

pub cop0_regs: [u32; 32],    // Offset 0x2BC..0x33C: COP0 System Control registers (Status, Cause, EPC, Count) } impl Default for EeCpuState { fn default() -> Self { let mut state: Self = unsafe { std::mem::zeroed() }; state.pc = 0xBFC0_0000; // MIPS R5900 Reset Vector in KSEG1 (4 MB BIOS ROM) state.next_pc = 0xBFC0_0004; state.cop0_regs[12] = 0x7040_0004; // Initial COP0 Status (BEV=1, ERL=1) state.cop0_regs[15] = 0x0000_2E20; // PRId: Toshiba/Sony R5900 state } } /// Clamps an IEEE-754 float bit pattern into a valid PS2 COP1/VU float bit pattern /// by converting Exponent 0xFF (NaN / Infinity) into max finite magnitude (0x7F7FFFFF / 0xFF7FFFFF) /// and flushing denormals (Exponent 0x00) to signed zero. #[inline(always)] pub fn ps2_clamp_f32_bits(bits: u32) -> u32 { let exp = (bits >> 23) & 0xFF; if exp == 0xFF { (bits & 0x8000_0000) | 0x7F7F_FFFF } else if exp == 0x00 { bits & 0x8000_0000 } else { bits } } #[derive(Copy, Clone, Debug, Eq, PartialEq)] pub enum HardwareEventKind { DmacTick, Vif1MtvuSync, 

IopTimeslice, HBlank, VBlankStart, VBlankEnd, Spu2Batch, } #[derive(Copy, Clone, Debug, Eq, PartialEq)] pub struct ScheduledEvent { pub trigger_cycle: u64, pub kind: HardwareEventKind, } 

impl Ord for ScheduledEvent { fn cmp(&self, other: &Self) -> Ordering { // Min-heap ordering: earliest trigger_cycle pops first other.trigger_cycle.cmp(&self.trigger_cycle) } } 

impl PartialOrd for ScheduledEvent { 

fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) } } pub struct Scheduler { pub current_ee_cycle: u64, pub next_event_cycle: u64, queue: BinaryHeap<ScheduledEvent>, } impl Scheduler { pub fn new() -> Self { let mut sched = Self { current_ee_cycle: 0, next_event_cycle: u64::MAX, queue: BinaryHeap::with_capacity(32), }; 

// Schedule initial IOP timeslice (every 256 EE cycles = 32 IOP cycles) and HBlank (18,740 EE cycles) sched.schedule(256, HardwareEventKind::IopTimeslice); sched.schedule(18_740, HardwareEventKind::HBlank); sched } #[inline(always)] pub fn schedule(&mut self, delta_ee_cycles: u64, kind: HardwareEventKind) { let trigger_cycle = self.current_ee_cycle.wrapping_add(delta_ee_cycles); self.queue.push(ScheduledEvent { trigger_cycle, kind }); if trigger_cycle < self.next_event_cycle { self.next_event_cycle = trigger_cycle; } } #[inline(always)] pub fn pop_due_event(&mut self) -> Option<ScheduledEvent> { if let Some(top) = self.queue.peek() { if self.current_ee_cycle >= top.trigger_cycle { let ev = self.queue.pop().unwrap(); self.next_event_cycle = self.queue.peek().map_or(u64::MAX, |n| n.trigger_cycle); return Some(ev); } } None } } /// Subsystem Trait Interfaces for Decoupled Unit Testing and Threading pub trait GifInterface { fn transfer_path3(&mut self, data: &[Reg128]); fn write_privileged(&mut self, addr: u32, val: u64); fn read_privileged(&self, addr: u32) -> u64; } 

/// System Bus with Fastmem Arena Pointer and Software Fallback pub struct Bus { pub ram: Box<[u8; 32 * 1024 * 1024]>,    // 32 MB Main RDRAM pub scratchpad: Box<[u8; 16 * 1024]>,    // 16 KB Scratchpad (0x7000_0000) pub bios: Box<[u8; 4 * 1024 * 1024]>,    // 4 MB BIOS ROM (0x1FC0_0000) pub fastmem_base: *mut u8,               // 4 GB OS Virtual Memory Arena base pointer } impl Bus { #[inline(always)] pub fn read32(&self, vaddr: u32) -> u32 { let paddr = vaddr & 0x1FFF_FFFF; match paddr { 0x0000_0000..=0x01FF_FFFF => { let offset = (paddr & 0x01FF_FFFC) as usize; u32::from_le_bytes(self.ram[offset..offset + 4].try_into().unwrap()) } 0x1FC0_0000..=0x1FFF_FFFF => { let offset = (paddr & 0x003F_FFFC) as usize; u32::from_le_bytes(self.bios[offset..offset + 4].try_into().unwrap()) } _ if (vaddr & 0xFFFF_C000) == 0x7000_0000 => { let offset = (vaddr & 0x3FFC) as usize; u32::from_le_bytes(self.scratchpad[offset..offset + 4].try_into().unwrap()) } _ => self.read_mmio32(paddr), } } #[inline(always)] pub fn write32(&mut self, vaddr: u32, val: u32) { let paddr = vaddr & 0x1FFF_FFFF; 

match paddr { 0x0000_0000..=0x01FF_FFFF => { let offset = (paddr & 0x01FF_FFFC) as usize; self.ram[offset..offset + 4].copy_from_slice(&val.to_le_bytes()); } _ if (vaddr & 0xFFFF_C000) == 0x7000_0000 => { let offset = (vaddr & 0x3FFC) as usize; self.scratchpad[offset..offset + 4].copy_from_slice(&val.to_le_bytes()); } _ => self.write_mmio32(paddr, val), } } #[cold] fn read_mmio32(&self, _paddr: u32) -> u32 { 0 } #[cold] fn write_mmio32(&mut self, _paddr: u32, _val: u32) {} } /// Minimal, Zero-Overhead EE Fetch-Decode-Execute and Event-Scheduled Loop pub fn run_ee_timeslice(ee: &mut EeCpuState, bus: &mut Bus, sched: &mut Scheduler) { loop { let batch = (sched.next_event_cycle.saturating_sub(sched.current_ee_cycle)).m in(256) as i32; ee.cycle_countdown = batch.max(1); let initial_countdown = ee.cycle_countdown; while ee.cycle_countdown > 0 { let instr = bus.read32(ee.pc); let current_pc = ee.pc; // Advance PC through delay-slot state machine if ee.in_delay_slot { 

ee.pc = ee.next_pc; ee.next_pc = ee.pc.wrapping_add(4); ee.in_delay_slot = false; } else { ee.pc = ee.next_pc; ee.next_pc = ee.pc.wrapping_add(4); } let op = instr >> 26; let rs = ((instr >> 21) & 0x1F) as usize; let rt = ((instr >> 16) & 0x1F) as usize; let rd = ((instr >> 11) & 0x1F) as usize; let imm_sext64 = ((instr as i16) as i64) as u64; match op { 0x00 => { // SPECIAL (ALU, Shifts, Jumps, EE 3-Operand Multiply) let funct = instr & 0x3F; match funct { 0x21 => { // ADDU (32-bit sign-extended into lower 64-bit GPR) 

if rd != 0 { let res = unsafe { (ee.gpr[rs].i32_lane[0].wrapping_add(ee.gpr[rt].i32_lane[0])) as i64 

}; ee.gpr[rd].i64_lane[0] = res; } } 0x18 => { // MULT rd, rs, rt (R5900 writes low 32 bits directly to rd!) let prod = unsafe { (ee.gpr[rs].i32_lane[0] as i64) * (ee.gpr[rt].i32_lane[0] as i64) }; ee.lo.i64_lane[0] = (prod as i32) as i64; 

ee.hi.i64_lane[0] = ((prod >> 32) as i32) as i64; if rd != 0 { ee.gpr[rd].i64_lane[0] = ee.lo.i64_lane[0]; } } 0x08 => { // JR rs ee.next_pc = unsafe { ee.gpr[rs].u32_lane[0] }; ee.in_delay_slot = true; } _ => {} } } 0x04 => { // BEQ rs, rt, offset let eq = unsafe { ee.gpr[rs].u64_lane[0] == ee.gpr[rt].u64_lane[0] }; if eq { let offset = ((instr as i16) as i32) << 2; ee.next_pc = current_pc.wrapping_add(4).wrapping_add(offset as u32); } ee.in_delay_slot = true; } 0x09 => { // ADDIU rt, rs, imm if rt != 0 { let sum = unsafe { (ee.gpr[rs].i32_lane[0].wrapping_add(instr as i16 as i32)) as i64 }; ee.gpr[rt].i64_lane[0] = sum; } } 0x0F => { // LUI rt, imm if rt != 0 { ee.gpr[rt].i64_lane[0] = (((instr & 0xFFFF) as i32) << 16) as i64; } 

} 0x23 => { // LW rt, offset(rs) let addr = unsafe { ee.gpr[rs].u64_lane[0].wrapping_add(imm_sext64) as u32 }; let val = bus.read32(addr) as i32 as i64; if rt != 0 { ee.gpr[rt].i64_lane[0] = val; } } 0x2B => { // SW rt, offset(rs) let addr = unsafe { ee.gpr[rs].u64_lane[0].wrapping_add(imm_sext64) as u32 }; let val = unsafe { ee.gpr[rt].u32_lane[0] }; bus.write32(addr, val); } _ => {} } // Enforce hardwired r0 == 0 invariant and decrement cycle counter ee.gpr[0].u128_val = 0; ee.cycle_countdown -= 1; } let executed = (initial_countdown - ee.cycle_countdown) as u64; sched.current_ee_cycle = sched.current_ee_cycle.wrapping_add(executed); ee.cop0_regs[9] = ee.cop0_regs[9].wrapping_add(executed as u32); // Update COP0 Count 

while let Some(event) = sched.pop_due_event() { match event.kind { HardwareEventKind::IopTimeslice => { // Run IOP for (256 / 8) = 32 cycles and reschedule sched.schedule(256, HardwareEventKind::IopTimeslice); } 

HardwareEventKind::HBlank => { sched.schedule(18_740, HardwareEventKind::HBlank); return; // Yield control at HBlank/VBlank boundary } _ => {} } } } } 

