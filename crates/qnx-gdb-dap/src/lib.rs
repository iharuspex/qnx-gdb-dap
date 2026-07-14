//! QNX GDB Debug Adapter implementation

#![forbid(unsafe_code)]

mod adapter;
mod config;

pub use adapter::{AdapterState, DebugAdapter};
pub use config::LaunchArguments;
