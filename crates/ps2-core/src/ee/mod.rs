//! Emotion Engine (MIPS R5900) CPU, COP0, COP1, MMI, and Interpreter Subsystem.

pub mod cop0;
pub mod cop1;
pub mod cop2;
pub mod interpreter;
pub mod mmi;
pub mod state;
pub mod syscall;

pub use cop0::{cause_bits, cop0_reg, status_bits, Level1ExceptionCode};
pub use cop1::FCR31_WRITABLE_MASK;
pub use interpreter::HleStepResult;
pub use state::{
    EeCpuState, EE_COP0_PRID_DEFAULT, EE_COP1_FCR0_DEFAULT, EE_COP1_FCR31_DEFAULT, EE_RESET_PC,
};
pub use syscall::{EeHleState, SyscallError};
