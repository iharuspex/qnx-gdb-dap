use std::process::ExitStatus;

use crate::GdbSessionEvent;

/// Action performed with the inferior before closing GDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdbDisconnectMode {
    /// Detach from the inferior and allow it to continue running.
    Detach,

    /// Terminate the inferior before closing GDB.
    Terminate,
}

/// Result of closing a GDB session.
#[derive(Debug)]
pub struct GdbShutdownResult {
    /// Operating-system exit status of the GDB process.
    pub status: ExitStatus,

    /// Events received while detaching, terminating, or closing GDB.
    pub events: Vec<GdbSessionEvent>,
}
