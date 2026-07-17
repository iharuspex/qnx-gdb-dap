use std::path::PathBuf;

/// One inferior thread reported by GDB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbThread {
    /// GDB thread identifier.
    pub id: u64,

    /// Human-readable thread name.
    ///
    /// Old QNX GDB does not expose names through `-thread-list-ids`, so the
    /// initial implementation generates a stable fallback name.
    pub name: String,

    /// Whether GDB currently considers this thread active.
    pub current: bool,
}

/// One call-stack frame reported by GDB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GdbStackFrame {
    /// GDB frame level, where zero is the top frame.
    pub level: u64,

    /// Program-counter address, when available.
    pub address: Option<u64>,

    /// Function name.
    pub function: Option<String>,

    /// Source file as reported by GDB.
    pub file: Option<PathBuf>,

    /// Full source path as reported by GDB.
    pub fullname: Option<PathBuf>,

    /// One-based source line.
    pub line: Option<u64>,
}
