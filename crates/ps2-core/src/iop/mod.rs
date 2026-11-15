//! R3000A I/O Processor (`IOP`) subsystem (`Task 6a`).
//!
//! Includes:
//! - [`state`]: 64-byte-aligned [`IopCpuState`], [`IopCop0`], and 1-instruction load delay queue.
//! - [`interpreter`]: Full MIPS I (`R3000A`) single-step interpreter driven at the `1:8` EE-cycle ratio.
//! - [`irx`]: Relocatable `.irx` (`ET_SCE_IOPRELEXEC`) loader and MIPS `SHT_REL` relocation engine.
//! - [`hle`]: Minimal `.iop.import` HLE for `stdio` (`printf`) and `sysclib` (`memcpy`).

pub mod hle;
pub mod interpreter;
pub mod irx;
pub mod state;

pub use hle::{try_handle_iop_import_stub, UnimplementedIopImport, IOP_IMPORT_MAGIC, MIPS_JR_RA};
pub use interpreter::IopStepResult;
pub use irx::{
    is_irx_binary, load_irx, load_irx_at, IrxError, LoadedIrx, ET_SCE_IOPRELEXEC,
    IOP_IRX_DEFAULT_BASE,
};
pub use state::{
    IopCop0, IopCpuState, IopExceptionCode, PendingLoad, EE_CYCLES_PER_IOP_CYCLE, IOP_HLE_EXIT_RA,
    IOP_RESET_PC,
};
