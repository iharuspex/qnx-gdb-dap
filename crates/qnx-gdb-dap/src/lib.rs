//! QNX GDB Debug Adapter implementation

#![forbid(unsafe_code)]

mod adapter;
mod breakpoints;
mod config;

pub use adapter::{AdapterState, DebugAdapter};
pub use breakpoints::{DapBreakpoint, DapSource, DapSourceBreakpoint, SetBreakpointsArguments};
pub use config::{DisconnectArguments, LaunchArguments};
