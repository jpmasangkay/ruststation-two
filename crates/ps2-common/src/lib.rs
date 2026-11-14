//! Common foundation types, non-IEEE 754 `ps2float` arithmetic, and virtual memory
//! primitives shared across the PS2 emulator workspace.

pub mod ps2float;
pub mod types;
pub mod vmem;

pub use ps2float::{ps2_clamp_f32_bits, Fcr31Flags, Ps2FloatResult};
pub use types::Reg128;
pub use vmem::VmemConfig;
