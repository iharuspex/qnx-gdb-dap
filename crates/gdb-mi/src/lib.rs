//! GDB Machine Interface client.
//!
//! The implementation targets the MI dialect provided by:
//!
//! `GNU gdb 6.8 qnx-nto (rev. 506)`
//!
//! It must not assume that modern GDB/MI commands are available.

#![forbid(unsafe_code)]

/// GDB version against which the initial implementation is developed.
pub const REFERENCE_GDB_VERSION: &str = "GNU gdb 6.8 qnx-nto (rev. 506)";

mod breakpoint;
mod command;
mod deployment;
mod execution;
mod parser;
mod process;
mod record;
mod session;
mod value;

pub use breakpoint::{GdbBreakpoint, SourceBreakpoint};
pub use command::{MiCommand, MiTokenGenerator, commands};
pub use deployment::{GdbDeployment, GdbDeploymentResult};
pub use execution::{GdbRunStarted, GdbSessionEvent, GdbStopReason, GdbStoppedFrame};
pub use parser::{MiParseError, parse_record};
pub use process::{
    GdbCommandResult, GdbEvent, GdbExecutionEvent, GdbExecutionStart, GdbProcess, GdbProcessConfig,
    GdbProcessError, GdbReaderError, GdbRecordPoll, is_gdb_file,
};
pub use record::{MiAsyncRecord, MiRecord, MiResultRecord};
pub use session::{
    GdbSession, GdbSessionConfig, GdbSessionError, GdbSessionOutput, GdbSessionState,
};
pub use value::{MiListItem, MiResult, MiValue, find_result};

#[cfg(test)]
mod tests {
    use super::REFERENCE_GDB_VERSION;

    #[test]
    fn reference_version_mentions_qnx() {
        assert!(REFERENCE_GDB_VERSION.contains("qnx-nto"));
    }
}
