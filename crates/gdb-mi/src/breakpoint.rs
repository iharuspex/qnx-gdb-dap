use std::path::PathBuf;

/// A requested source breakpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceBreakpoint {
    /// One-based source line number.
    pub line: u64,
}

impl SourceBreakpoint {
    /// Creates a source breakpoint request.
    #[must_use]
    pub const fn new(line: u64) -> Self {
        Self { line }
    }
}

/// Result of attempting to create one source breakpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbBreakpoint {
    /// GDB breakpoint number, when GDB created the breakpoint.
    pub number: Option<u64>,

    /// Source file associated with the request.
    pub source: PathBuf,

    /// Requested or resolved source line.
    pub line: u64,

    /// Whether GDB verified the breakpoint.
    pub verified: bool,

    /// Function name reported by GDB.
    pub function: Option<String>,

    /// Resolved source file reported by GDB.
    pub resolved_file: Option<PathBuf>,

    /// Address reported by GDB.
    pub address: Option<String>,

    /// Diagnostic message for an unverified breakpoint.
    pub message: Option<String>,
}

impl GdbBreakpoint {
    /// Creates an unverified breakpoint result.
    #[must_use]
    pub fn unverified(source: impl Into<PathBuf>, line: u64, message: impl Into<String>) -> Self {
        Self {
            number: None,
            source: source.into(),
            line,
            verified: false,
            function: None,
            resolved_file: None,
            address: None,
            message: Some(message.into()),
        }
    }
}
