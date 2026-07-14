use crate::MiResult;

/// A complete output record produced by GDB/MI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MiRecord {
    /// Result of a command, prefixed by `^`.
    Result(MiResultRecord),

    /// Asynchronous execution state, prefixed by `*`.
    ExecAsync(MiAsyncRecord),

    /// Asynchronous status information, prefixed by `+`.
    StatusAsync(MiAsyncRecord),

    /// Asynchronous notification, prefixed by `=`.
    NotifyAsync(MiAsyncRecord),

    /// GDB console stream output, prefixed by `~`.
    ConsoleStream(String),

    /// Inferior target output, prefixed by `@`.
    TargetStream(String),

    /// GDB diagnostic/log output, prefixed by `&`.
    LogStream(String),

    /// GDB prompt.
    Prompt,

    /// An empty line.
    Empty,
}

/// A command result record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiResultRecord {
    /// Optional numeric command token.
    pub token: Option<u64>,

    /// Result class, for example `done`, `running`, `connected` or `error`.
    pub class: String,

    /// Named result values.
    pub results: Vec<MiResult>,
}

/// An asynchronous MI record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiAsyncRecord {
    /// Optional numeric token.
    pub token: Option<u64>,

    /// Async class, for example `stopped`, `running` or `thread-created`.
    pub class: String,

    /// Named result values.
    pub results: Vec<MiResult>,
}
