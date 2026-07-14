//! QNX GDB Debug Adapter implementation

#![forbid(unsafe_code)]

mod adapter;

pub use adapter::{AdapterState, DebugAdapter};
