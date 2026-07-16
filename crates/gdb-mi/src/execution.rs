use std::path::PathBuf;

/// Reason why execution stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbStopReason {
    /// A source or instruction breakpoint was hit.
    Breakpoint {
        /// GDB breakpoint number.
        breakpoint_number: Option<u64>,
    },

    /// Execution stopped after a step operation.
    Step,

    /// The inferior received a signal.
    Signal {
        /// Signal name reported by GDB.
        name: Option<String>,

        /// Human-readable signal description.
        meaning: Option<String>,
    },

    /// The inferior exited normally.
    Exited {
        /// Exit code, if GDB reported one.
        exit_code: Option<i32>,
    },

    /// The inferior exited because of a signal.
    ExitedSignalled {
        /// Signal name reported by GDB.
        signal_name: Option<String>,
    },

    /// GDB reported an unrecognized stop reason.
    Unknown {
        /// Raw MI reason value.
        reason: Option<String>,
    },
}

/// Source frame reported with a stop event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbStoppedFrame {
    /// Instruction address.
    pub address: Option<String>,

    /// Function name.
    pub function: Option<String>,

    /// Source file path.
    pub file: Option<PathBuf>,

    /// One-based source line.
    pub line: Option<u64>,
}

/// High-level event produced while the inferior is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GdbSessionEvent {
    /// Inferior stdout or stderr received through the target stream.
    TargetOutput(String),

    /// GDB console output.
    ConsoleOutput(String),

    /// GDB diagnostic output.
    DiagnosticOutput(String),

    /// Inferior execution stopped.
    Stopped {
        /// Parsed stop reason.
        reason: GdbStopReason,

        /// Thread identifier reported by GDB.
        thread_id: Option<u64>,

        /// Top frame included in the stop notification.
        frame: Option<GdbStoppedFrame>,
    },

    /// GDB produced an asynchronous record not yet mapped to a specialized
    /// event.
    AsyncRecord {
        /// Async record class.
        class: String,
    },

    /// GDB produced a late result record after initially returning `^running`.
    LateResult {
        /// Original command token.
        token: Option<u64>,

        /// Result class.
        class: String,

        /// Error message for a late `^error`, if present.
        message: Option<String>,
    },

    /// GDB closed its output stream.
    EndOfFile,
}

/// Result of starting inferior execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbRunStarted {
    /// Token assigned to `-exec-run`.
    pub token: u64,

    /// Events received before the initial result record.
    pub initial_events: Vec<GdbSessionEvent>,
}
