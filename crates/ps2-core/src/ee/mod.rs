//! Emotion Engine (MIPS R5900) CPU, COP0, and Interpreter Subsystem.

pub mod cop0;
pub mod interpreter;
pub mod state;

pub use cop0::{cause_bits, cop0_reg, status_bits, Level1ExceptionCode};
pub use state::{EeCpuState, EE_COP0_PRID_DEFAULT, EE_RESET_PC};
